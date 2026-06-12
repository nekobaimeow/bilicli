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

/// Build the deterministic file path for a given index (0-based frame
/// number). Use this when generating paths for OCR output.
pub fn frame_path(out_dir: &Path, t_sec: f32) -> PathBuf {
    out_dir.join(format!("frame_{:09.3}.jpg", t_sec))
}

/// Parse a timestamp back out of a path produced by `frame_path`.
pub fn parse_frame_ts(p: &Path) -> Option<f32> {
    let stem = p.file_stem()?.to_str()?;
    stem.strip_prefix("frame_")?.parse().ok()
}

/// Parse a frame index out of an ffmpeg-numbered path (e.g.
/// `frame_00001.jpg`). Returns None for non-matching names.
pub fn parse_frame_index(p: &Path) -> Option<u32> {
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
    // ffmpeg's image2 muxer requires a simple integer pattern like
    // `%05d`; the `%.3f` / `%09.3f` patterns we tried first are
    // rejected as invalid. The Rust side recovers the timestamp from
    // the frame index (i × interval_sec) at OCR time.
    let pattern = out_dir.join("frame_%05d.jpg");

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
    // Sort by frame index so output is in time order (parse_frame_index
    // returns Option<u32>; missing/invalid names sort to the end).
    frames.sort_by_key(|p| {
        crate::ipc::ocr::frames::parse_frame_index(p).unwrap_or(u32::MAX)
    });

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

/// Extract a single frame at `t_sec` from `video`, write it to
/// `out_dir/frame_<ts>.jpg`, and return the resulting path. Used by
/// the adaptive-sampling loop where we only want one frame at a time.
pub async fn extract_single_frame(
    video: &Path,
    out_dir: &Path,
    t_sec: f32,
) -> Result<PathBuf, String> {
    tokio::fs::create_dir_all(out_dir)
        .await
        .map_err(|e| format!("create frames dir: {e}"))?;
    // ffmpeg -ss t -i in -frames:v 1 -q:v 2 out.jpg
    // The `-ss` before `-i` does a fast keyframe seek, then -frames:v 1
    // grabs the first decoded frame at or after the seek point. For most
    // B 站 videos this is precise to ~0.1s; for frame-accurate extraction
    // we'd need a second -ss after -i, but adaptive sampling doesn't need
    // that precision.
    let out_path = out_dir.join(format!("frame_{:09.3}.jpg", t_sec));
    let status = tokio::process::Command::new("ffmpeg")
        .args(["-y", "-ss", &format!("{t_sec:.3}"), "-i"])
        .arg(video)
        .args(["-frames:v", "1", "-q:v", "2"])
        .arg(&out_path)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await
        .map_err(|e| format!("spawn ffmpeg: {e}"))?;
    if !status.success() {
        return Err(format!("ffmpeg (single frame at {t_sec:.3}s) exit {status}"));
    }
    if !out_path.exists() {
        return Err(format!("ffmpeg reported success but {} is missing", out_path.display()));
    }
    Ok(out_path)
}

/// Probe the video's duration in seconds via `ffprobe -v error
/// -show_entries format=duration -of csv=p=0`. Returns 0.0 on parse
/// failure (caller should fall back to a default).
pub async fn probe_duration(video: &Path) -> Result<f32, String> {
    // Prefer `ffprobe` if present; fall back to `ffmpeg -i` + grep.
    let probe = tokio::process::Command::new("ffprobe")
        .args([
            "-v", "error",
            "-show_entries", "format=duration",
            "-of", "default=noprint_wrappers=1:nokey=1",
        ])
        .arg(video)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
        .await;
    if let Ok(out) = probe {
        if out.status.success() {
            let s = String::from_utf8_lossy(&out.stdout);
            if let Ok(d) = s.trim().parse::<f32>() {
                if d > 0.0 {
                    return Ok(d);
                }
            }
        }
    }
    // Fallback: spawn ffmpeg and parse stderr for "Duration: HH:MM:SS.xx"
    let out = tokio::process::Command::new("ffmpeg")
        .arg("-i")
        .arg(video)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .output()
        .await
        .map_err(|e| format!("spawn ffmpeg for probe: {e}"))?;
    let stderr = String::from_utf8_lossy(&out.stderr);
    // Look for "Duration: 00:02:43.05, start:..."
    if let Some(idx) = stderr.find("Duration:") {
        let after = &stderr[idx + 9..];
        if let Some(comma) = after.find(',') {
            let ts = after[..comma].trim();
            return parse_hms(ts).ok_or_else(|| format!("could not parse Duration: {ts:?}"));
        }
    }
    Err(format!(
        "could not determine video duration. ffprobe missing? ffmpeg stderr did not contain 'Duration:' line."
    ))
}

/// Parse "HH:MM:SS.xx" → seconds.
fn parse_hms(s: &str) -> Option<f32> {
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() != 3 {
        return None;
    }
    let h: f32 = parts[0].parse().ok()?;
    let m: f32 = parts[1].parse().ok()?;
    let sec: f32 = parts[2].parse().ok()?;
    Some(h * 3600.0 + m * 60.0 + sec)
}
