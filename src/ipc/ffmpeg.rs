// SPDX-License-Identifier: GPL-3.0-or-later
// FFmpeg wrapper — ported from BiliTools `src-tauri/src/services/ffmpeg.rs`.
//
// The original invokes FFmpeg via the Tauri shell sidecar. The CLI
// port uses `tokio::process::Command` directly. We keep the same
// argument shape so behavior is identical to the GUI version.

use crate::backends::sidecar::{resolve, SidecarKind};
use crate::error::CliError;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Returned by `probe()`. Fields mirror what `ffprobe -v error -show_streams
/// -show_format -of json` produces.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaInfo {
    pub streams: Vec<MediaStream>,
    pub format: MediaFormat,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaStream {
    pub index: i64,
    #[serde(rename = "codec_name", default)]
    pub codec_name: String,
    #[serde(rename = "codec_type", default)]
    pub codec_type: String,
    #[serde(default)]
    pub width: Option<i64>,
    #[serde(default)]
    pub height: Option<i64>,
    #[serde(default)]
    pub duration: Option<String>,
    #[serde(default)]
    pub bit_rate: Option<String>,
    #[serde(default)]
    pub channels: Option<i64>,
    #[serde(default)]
    pub sample_rate: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaFormat {
    pub filename: String,
    #[serde(default)]
    pub duration: Option<String>,
    #[serde(default)]
    pub bit_rate: Option<String>,
    #[serde(default)]
    pub size: Option<String>,
}

/// Run `ffmpeg -version` to confirm the binary is reachable.
pub async fn test(override_path: Option<&Path>) -> Result<String, CliError> {
    let ffmpeg = resolve(SidecarKind::FFmpeg, override_path)?;
    let out = tokio::process::Command::new(&ffmpeg)
        .arg("-version")
        .output()
        .await
        .map_err(|e| CliError::msg(format!("ffmpeg spawn failed: {e}")))?;
    if !out.status.success() {
        return Err(CliError::msg(format!(
            "ffmpeg exited with status {}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        )));
    }
    let s = String::from_utf8_lossy(&out.stdout).to_string();
    // The first line is "ffmpeg version N-...". Trim it.
    Ok(s.lines().next().unwrap_or("").to_string())
}

/// Get the duration of a media file in seconds.
pub async fn get_duration(path: &Path, override_ffmpeg: Option<&Path>) -> Result<u64, CliError> {
    let info = probe(path, override_ffmpeg).await?;
    let dur = info
        .format
        .duration
        .as_deref()
        .or_else(|| info.streams.first().and_then(|s| s.duration.as_deref()))
        .ok_or_else(|| CliError::msg("no duration in ffprobe output"))?;
    let secs: f64 = dur
        .parse()
        .map_err(|e| CliError::msg(format!("bad duration '{dur}': {e}")))?;
    Ok(secs as u64)
}

/// Run `ffprobe` and return a `MediaInfo`.
pub async fn probe(path: &Path, override_ffmpeg: Option<&Path>) -> Result<MediaInfo, CliError> {
    let ffprobe = resolve(SidecarKind::FFmpeg, override_ffmpeg)?
        .parent()
        .map(|p| p.join("ffprobe"))
        .unwrap_or_else(|| PathBuf::from("ffprobe"));
    let out = tokio::process::Command::new(&ffprobe)
        .args([
            "-v", "error",
            "-show_format",
            "-show_streams",
            "-of", "json",
        ])
        .arg(path)
        .output()
        .await
        .map_err(|e| CliError::msg(format!("ffprobe spawn failed: {e}")))?;
    if !out.status.success() {
        return Err(CliError::msg(format!(
            "ffprobe exited with status {}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        )));
    }
    let info: MediaInfo = serde_json::from_slice(&out.stdout)
        .map_err(|e| CliError::msg(format!("bad ffprobe json: {e}")))?;
    Ok(info)
}

/// Run a custom ffmpeg invocation. Returns the combined stdout on
/// success. Use this for one-off transcoding tasks.
pub async fn run(args: &[&str], override_path: Option<&Path>) -> Result<Vec<u8>, CliError> {
    let ffmpeg = resolve(SidecarKind::FFmpeg, override_path)?;
    let out = tokio::process::Command::new(&ffmpeg)
        .args(args)
        .output()
        .await
        .map_err(|e| CliError::msg(format!("ffmpeg spawn failed: {e}")))?;
    if !out.status.success() {
        return Err(CliError::msg(format!(
            "ffmpeg failed: {} (stderr: {})",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        )));
    }
    Ok(out.stdout)
}

/// Merge a video stream and an audio stream into an MP4 using
/// stream-copy. Equivalent of `ffmpeg -i video.mp4 -i audio.m4a -c copy
/// -map 0:v -map 1:a out.mp4`.
pub async fn merge_av(
    video: &Path,
    audio: &Path,
    output: &Path,
    override_path: Option<&Path>,
) -> Result<(), CliError> {
    let ffmpeg = resolve(SidecarKind::FFmpeg, override_path)?;
    let status = tokio::process::Command::new(&ffmpeg)
        .args([
            "-y",
            "-i",
        ])
        .arg(video)
        .args([
            "-i",
        ])
        .arg(audio)
        .args(["-c", "copy", "-map", "0:v", "-map", "1:a"])
        .arg(output)
        .output()
        .await
        .map_err(|e| CliError::msg(format!("ffmpeg spawn failed: {e}")))?;
    if !status.status.success() {
        return Err(CliError::msg(format!(
            "ffmpeg merge_av failed: {} (stderr: {})",
            status.status,
            String::from_utf8_lossy(&status.stderr)
        )));
    }
    Ok(())
}

/// Transcode an audio file to MP3 (CBR 192k).
pub async fn convert_mp3(input: &Path, output: &Path, override_path: Option<&Path>) -> Result<(), CliError> {
    let ffmpeg = resolve(SidecarKind::FFmpeg, override_path)?;
    let status = tokio::process::Command::new(&ffmpeg)
        .args(["-y", "-i"])
        .arg(input)
        .args(["-vn", "-acodec", "libmp3lame", "-b:a", "192k"])
        .arg(output)
        .output()
        .await
        .map_err(|e| CliError::msg(format!("ffmpeg spawn failed: {e}")))?;
    if !status.status.success() {
        return Err(CliError::msg(format!(
            "ffmpeg convert_mp3 failed: {} (stderr: {})",
            status.status,
            String::from_utf8_lossy(&status.stderr)
        )));
    }
    Ok(())
}

/// Concatenate a list of MP4 files in order. Used to splice multi-P
/// episodes into a single file.
pub async fn concat_mp4(inputs: &[PathBuf], output: &Path, override_path: Option<&Path>) -> Result<(), CliError> {
    if inputs.is_empty() {
        return Err(CliError::msg("concat_mp4 called with no inputs"));
    }
    let ffmpeg = resolve(SidecarKind::FFmpeg, override_path)?;
    // Build a temporary concat list file
    let tmp = std::env::temp_dir().join(format!("bilicli-concat-{}.txt", uuid::Uuid::new_v4()));
    let mut f = std::fs::File::create(&tmp)?;
    use std::io::Write;
    for p in inputs {
        writeln!(f, "file '{}'", p.to_string_lossy().replace('\'', "'\\''"))?;
    }
    drop(f);
    let status = tokio::process::Command::new(&ffmpeg)
        .args(["-y", "-f", "concat", "-safe", "0", "-i"])
        .arg(&tmp)
        .args(["-c", "copy"])
        .arg(output)
        .output()
        .await
        .map_err(|e| CliError::msg(format!("ffmpeg spawn failed: {e}")))?;
    let _ = std::fs::remove_file(&tmp);
    if !status.status.success() {
        return Err(CliError::msg(format!(
            "ffmpeg concat_mp4 failed: {} (stderr: {})",
            status.status,
            String::from_utf8_lossy(&status.stderr)
        )));
    }
    Ok(())
}

// =====================  Tests  =====================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_info_deserializes() {
        let s = r#"{
            "streams": [
                {"index": 0, "codec_name": "h264", "codec_type": "video", "width": 1920, "height": 1080, "duration": "120.0"}
            ],
            "format": {"filename": "x.mp4", "duration": "120.0", "bit_rate": "5000000", "size": "75000000"}
        }"#;
        let info: MediaInfo = serde_json::from_str(s).unwrap();
        assert_eq!(info.streams.len(), 1);
        assert_eq!(info.streams[0].width, Some(1920));
        assert_eq!(info.format.filename, "x.mp4");
    }

    #[test]
    fn media_info_handles_missing_optional_fields() {
        let s = r#"{"streams": [], "format": {"filename": "x"}}"#;
        let info: MediaInfo = serde_json::from_str(s).unwrap();
        assert!(info.streams.is_empty());
        assert!(info.format.duration.is_none());
    }
}
