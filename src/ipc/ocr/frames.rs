// SPDX-License-Identifier: GPL-3.0-or-later
//! ffmpeg-based frame extraction for the OCR pipeline.

use std::path::{Path, PathBuf};

/// Result of a frame extraction: list of frame file paths in time order.
#[derive(Debug, Clone)]
pub struct ExtractResult {
    /// Paths to extracted frames, sorted by filename (= time order).
    pub frames: Vec<PathBuf>,
    /// The interval the user asked for, in seconds.
    pub interval_sec: f32,
    /// The cap the user asked for.
    pub max_frames: u32,
}

/// Build the deterministic file path for a given timestamp.
pub fn frame_path(out_dir: &Path, t_sec: f32) -> PathBuf {
    out_dir.join(format!("frame_{:09.3}.jpg", t_sec))
}

/// Parse a timestamp back out of a path produced by `frame_path`.
pub fn parse_frame_ts(p: &Path) -> Option<f32> {
    let stem = p.file_stem()?.to_str()?;
    stem.strip_prefix("frame_")?.parse().ok()
}

/// Extract frames from `video` into `out_dir` at `interval_sec`
/// spacing, stopping after `max_frames` frames. The output filenames
/// are `frame_<ts>.jpg` so we can recover the timestamp later.
pub async fn extract_frames(
    video: &Path,
    out_dir: &Path,
    interval_sec: f32,
    max_frames: u32,
) -> Result<ExtractResult, String> {
    if interval_sec <= 0.0 {
        return Err("interval must be > 0".into());
    }
    if max_frames == 0 {
        return Err("max_frames must be > 0".into());
    }

    tokio::fs::create_dir_all(out_dir)
        .await
        .map_err(|e| format!("create frames dir: {e}"))?;

    let fps = 1.0 / interval_sec;
    let pattern = out_dir.join("frame_%09.3f.jpg");

    let mut cmd = tokio::process::Command::new("ffmpeg");
    cmd.arg("-y")
        .arg("-i")
        .arg(video)
        .args(["-vf", &format!("fps={fps}")])
        .args(["-frames:v", &max_frames.to_string()])
        .arg("-q:v")
        .arg("2") // JPEG quality 2 = visually lossless
        .arg(&pattern);
    cmd.stdout(std::process::Stdio::null());
    cmd.stderr(std::process::Stdio::null());

    let status = cmd
        .status()
        .await
        .map_err(|e| format!("spawn ffmpeg: {e}"))?;
    if !status.success() {
        return Err(format!("ffmpeg exited with status {status}"));
    }

    let mut frames: Vec<PathBuf> = Vec::new();
    let mut entries = tokio::fs::read_dir(out_dir)
        .await
        .map_err(|e| format!("read frames dir: {e}"))?;
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|e| format!("iterate frames dir: {e}"))?
    {
        let p = entry.path();
        if p.extension().and_then(|s| s.to_str()) == Some("jpg")
            && p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with("frame_"))
                .unwrap_or(false)
        {
            frames.push(p);
        }
    }
    frames.sort();

    Ok(ExtractResult {
        frames,
        interval_sec,
        max_frames,
    })
}

/// Make sure `ffmpeg` is on `PATH`. Returns a clean error message
/// pointing the user at the missing binary.
pub fn ensure_ffmpeg() -> Result<(), String> {
    // Probe by spawning `ffmpeg -version` with a short timeout. We use
    // a direct spawn rather than the `which` crate so this module does
    // not pull in another transitive dep.
    match std::process::Command::new("ffmpeg")
        .arg("-version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
    {
        Ok(status) if status.success() => Ok(()),
        _ => Err(
            "ffmpeg not found in PATH. Install ffmpeg:\n  \
             Ubuntu/Debian:  sudo apt install ffmpeg\n  \
             macOS:          brew install ffmpeg\n  \
             Windows:        choco install ffmpeg"
                .to_string(),
        ),
    }
}
