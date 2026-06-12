// SPDX-License-Identifier: GPL-3.0-or-later
//! `bilitools ocr` subcommand — offline OCR via PaddleOCR + MNN.
//!
//! Pipeline: ffmpeg frame extraction (video mode) → ocr-rs (PP-OCRv5
//! mobile) recognition → JSON / human-readable output with confidence
//! and per-detection bounding boxes.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::cli::output::Output;
use crate::cli::root::{Command, SampleMode};
use crate::error::CliError;

pub async fn run(cmd: &Command, out: &Output) -> Result<(), CliError> {
    let Command::Ocr {
        input,
        video,
        interval,
        max_frames,
        min_conf,
        output_dir,
        keep_frames,
        dedup_window,
        dedup_iou,
        sample_mode,
    } = cmd
    else {
        return Err(CliError::Other("internal: not Ocr command".into()));
    };

    let output_dir = output_dir.clone().unwrap_or_else(default_output_dir);
    std::fs::create_dir_all(&output_dir).map_err(|e| CliError::Other(e.to_string()))?;

    let paths = crate::ipc::ocr::model_paths::find_model().map_err(CliError::Other)?;
    out.status(&format!(
        "loading OCR engine (MNN, PP-OCRv5) — det={}",
        paths.det.display()
    ));
    let engine = crate::ipc::ocr::engine::OcrEngine::load(&paths).map_err(CliError::Other)?;

    if *video {
        run_video(
            input,
            *interval,
            *max_frames,
            *min_conf,
            &output_dir,
            *keep_frames,
            *dedup_window,
            *dedup_iou,
            *sample_mode,
            &engine,
            out,
        )
        .await
    } else {
        run_image(input, &output_dir, *min_conf, &engine, out).await
    }
}

async fn run_image(
    input: &str,
    output_dir: &PathBuf,
    min_conf: f32,
    engine: &crate::ipc::ocr::engine::OcrEngine,
    out: &Output,
) -> Result<(), CliError> {
    let img = image::open(input).map_err(|e| CliError::Other(format!("open image: {e}")))?;
    out.status(&format!("OCR {} ({}x{}) ...", input, img.width(), img.height()));
    let results = engine.recognize(&img).map_err(CliError::Other)?;
    let kept: Vec<_> = results
        .into_iter()
        .filter(|r| r.confidence >= min_conf)
        .collect();

    let result = serde_json::json!({
        "mode": "image",
        "input": input,
        "image_size": [img.width(), img.height()],
        "detections": kept.iter().map(|r| serde_json::json!({
            "text": r.text,
            "confidence": r.confidence,
            "bbox": r.bbox,
        })).collect::<Vec<_>>(),
    });

    let json_path = output_dir.join("ocr.json");
    std::fs::write(&json_path, serde_json::to_string_pretty(&result).unwrap())
        .map_err(|e| CliError::Other(e.to_string()))?;

    if out.is_json() {
        out.ok(result)?;
    } else {
        out.status(&format!(
            "OCR done: {} detections (kept at min_conf={})",
            kept.len(),
            min_conf
        ));
        for r in &kept {
            out.status(&format!("  ({:.2}) {}", r.confidence, r.text));
        }
        out.status(&format!("wrote {}", json_path.display()));
    }
    Ok(())
}

async fn run_video(
    input: &str,
    interval: f32,
    max_frames: u32,
    min_conf: f32,
    output_dir: &PathBuf,
    keep_frames: bool,
    dedup_window: f32,
    dedup_iou: f32,
    sample_mode: SampleMode,
    engine: &crate::ipc::ocr::engine::OcrEngine,
    out: &Output,
) -> Result<(), CliError> {
    crate::ipc::ocr::frames::ensure_ffmpeg().map_err(CliError::Other)?;

    let video_path = resolve_video_path(input, output_dir)?;

    // Probe duration for adaptive mode (linear mode uses --max-frames cap).
    let duration_sec = crate::ipc::ocr::frames::probe_duration(&video_path)
        .await
        .map_err(CliError::Other)?;
    out.status(&format!(
        "video duration: {:.1}s, sampling mode: {:?}",
        duration_sec, sample_mode
    ));

    let frames_dir = output_dir.join("frames");

    // -------- Branch 1: adaptive (v3 two-pointer binary search) --------
    if sample_mode == SampleMode::Adaptive {
        // The v3 algorithm is supposed to be a full traversal of
        // [0, duration_sec] — the user spec is "二分递归 + 退出条件
        // lo<=right or ocr(lo)=ocr(hi)", which only makes sense as a
        // complete sweep. The user explicitly said "v3 正确的话它自
        // 动就是一个遍历, 完全不需要花里胡哨的参数". So the budget
        // is purely a safety cap, not a feature knob — default to
        // u32::MAX (effectively unlimited). Power users who want to
        // truncate very long videos can still pass --max-frames.
        let max_ocr = if max_frames == 0 {
            u32::MAX
        } else {
            max_frames
        };
        let (samples, ocr_calls) = crate::ipc::ocr::adaptive::run(
            engine,
            &video_path,
            &frames_dir,
            duration_sec,
            &crate::ipc::ocr::adaptive::AdaptiveConfig {
                min_segment_sec: interval.max(3.0), // reuse --interval as min_segment floor
                max_ocr_calls: max_ocr,
                iou_thresh: dedup_iou,
                text_sim_thresh: 0.5,
                min_conf,
            },
        )
        .await;

        if !keep_frames {
            let _ = std::fs::remove_dir_all(&frames_dir);
        }

        out.status(&format!(
            "adaptive sampling: {} samples ({} OCR calls, budget {})",
            samples.len(),
            ocr_calls,
            max_ocr
        ));

        // Flatten samples into raws + capture frame_size from the
        // first sample.
        let mut raws: Vec<crate::ipc::ocr::dedup::RawDetection> = Vec::new();
        let mut frame_size: (f32, f32) = (1920.0, 1080.0);
        for s in &samples {
            if let Ok(img) = image::open(&s.frame) {
                frame_size = (img.width() as f32, img.height() as f32);
            }
            raws.extend(s.raws.iter().cloned());
        }
        // The adaptive pass already did the dedup-stop walk, but we
        // also run the post-hoc merge so the user gets the same
        // first_t / last_t / n_frames / category fields as the
        // linear path.
        let n_raw = raws.len();
        let merged = if dedup_window > 0.0 {
            let cfg = crate::ipc::ocr::dedup::DedupConfig {
                window_sec: dedup_window,
                iou_thresh: dedup_iou,
                text_sim_thresh: 0.5,
                frame_size,
                video_duration_sec: duration_sec,
            };
            crate::ipc::ocr::dedup::merge(&raws, &cfg)
        } else {
            raws.iter()
                .map(|r| crate::ipc::ocr::dedup::MergedDetection {
                    text: r.text.clone(),
                    first_t: r.t_sec,
                    last_t: r.t_sec,
                    n_frames: 1,
                    best_conf: r.confidence,
                    avg_conf: r.confidence,
                    bbox: r.bbox,
                    category: "raw",
                })
                .collect()
        };
        let n_merged = merged.len();

        let result = serde_json::json!({
            "mode": "video",
            "sample_mode": "adaptive",
            "input": input,
            "video_path": video_path.to_string_lossy(),
            "duration_sec": duration_sec,
            "ocr_calls": ocr_calls,
            "dedup": {
                "enabled": dedup_window > 0.0,
                "raw_count": n_raw,
                "merged_count": n_merged,
                "window_sec": dedup_window,
                "iou_thresh": dedup_iou,
            },
            "samples": samples.iter().map(|s| serde_json::json!({
                "t_sec": s.t_sec,
                "n_raw": s.raws.len(),
            })).collect::<Vec<_>>(),
            "detections": merged,
        });

        let json_path = output_dir.join("ocr.json");
        std::fs::write(&json_path, serde_json::to_string_pretty(&result).unwrap())
            .map_err(|e| CliError::Other(e.to_string()))?;

        if out.is_json() {
            out.ok(result)?;
        } else {
            out.status(&format!(
                "OCR done: adaptive {} OCRs, {} raw → {} merged",
                samples.len(),
                n_raw,
                n_merged
            ));
            for d in &merged {
                let span = if (d.first_t - d.last_t).abs() < 0.01 {
                    format!("{:>6.1}s", d.first_t)
                } else {
                    format!("{:>6.1}-{:>6.1}s", d.first_t, d.last_t)
                };
                out.status(&format!(
                    "  [{}] ({:.2}, ×{}) {:>10}  {}",
                    span, d.best_conf, d.n_frames, d.category, d.text
                ));
            }
            out.status(&format!("wrote {}", json_path.display()));
        }
        return Ok(());
    }

    // -------- Branch 2: linear (fixed --interval) --------
    // max_frames=0 means "unlimited", i.e. cover the whole video at
    // the given --interval. Cap = ceil(duration / interval).
    let linear_cap = if max_frames == 0 {
        (duration_sec / interval).ceil() as u32
    } else {
        max_frames
    };
    let extract = crate::ipc::ocr::frames::extract_frames(&video_path, &frames_dir, interval, linear_cap)
        .await
        .map_err(CliError::Other)?;
    out.status(&format!(
        "extracted {} frames from {} (interval {}s, max {})",
        extract.frames.len(),
        video_path.display(),
        interval,
        linear_cap
    ));

    // Collect raw detections, capturing the first frame's dimensions so
    // the dedup classifier can decide which corner / band a bbox lives in.
    let mut raws: Vec<crate::ipc::ocr::dedup::RawDetection> = Vec::new();
    let mut frame_size: (f32, f32) = (1920.0, 1080.0);
    for (i, frame) in extract.frames.iter().enumerate() {
        // Frame index `i` was extracted at `i * interval_sec` seconds
        // because ffmpeg's `fps=1/interval` filter produces a fixed
        // cadence. (See `extract_frames` for the pattern.)
        let ts = (i as f32) * interval;
        let img = image::open(frame)
            .map_err(|e| CliError::Other(format!("open {}: {e}", frame.display())))?;
        if i == 0 {
            frame_size = (img.width() as f32, img.height() as f32);
        }
        let detections = engine.recognize(&img).map_err(CliError::Other)?;
        for d in detections {
            if d.confidence >= min_conf {
                raws.push(crate::ipc::ocr::dedup::RawDetection {
                    t_sec: ts,
                    text: d.text,
                    confidence: d.confidence,
                    bbox: d.bbox,
                });
            }
        }
        if i % 10 == 0 {
            out.status(&format!(
                "  OCR frame {}/{} (t={:.1}s)",
                i + 1,
                extract.frames.len(),
                ts
            ));
        }
    }

    if !keep_frames {
        let _ = std::fs::remove_dir_all(&frames_dir);
    }

    // Spatial-temporal dedup. window_sec=0 disables dedup.
    let n_raw = raws.len();
    let merged = if dedup_window > 0.0 {
        let cfg = crate::ipc::ocr::dedup::DedupConfig {
            window_sec: dedup_window,
            iou_thresh: dedup_iou,
            text_sim_thresh: 0.5,
            frame_size,
            video_duration_sec: extract.frames.len() as f32 * interval,
        };
        crate::ipc::ocr::dedup::merge(&raws, &cfg)
    } else {
        // No dedup — wrap each raw as a single-frame MergedDetection
        // so the output shape stays consistent.
        raws.iter()
            .map(|r| crate::ipc::ocr::dedup::MergedDetection {
                text: r.text.clone(),
                first_t: r.t_sec,
                last_t: r.t_sec,
                n_frames: 1,
                best_conf: r.confidence,
                avg_conf: r.confidence,
                bbox: r.bbox,
                category: "raw",
            })
            .collect()
    };
    let n_merged = merged.len();

    let result = serde_json::json!({
        "mode": "video",
        "sample_mode": "linear",
        "input": input,
        "video_path": video_path.to_string_lossy(),
        "frames_processed": extract.frames.len(),
        "interval_sec": interval,
        "dedup": {
            "enabled": dedup_window > 0.0,
            "raw_count": n_raw,
            "merged_count": n_merged,
            "window_sec": dedup_window,
            "iou_thresh": dedup_iou,
        },
        "detections": merged,
    });

    let json_path = output_dir.join("ocr.json");
    std::fs::write(&json_path, serde_json::to_string_pretty(&result).unwrap())
        .map_err(|e| CliError::Other(e.to_string()))?;

    if out.is_json() {
        out.ok(result)?;
    } else {
        out.status(&format!(
            "OCR done: linear {} frames, {} raw → {} merged (window {}s, iou {})",
            extract.frames.len(),
            n_raw,
            n_merged,
            dedup_window,
            dedup_iou
        ));
        for d in &merged {
            let span = if d.first_t == d.last_t {
                format!("{:>6.1}s", d.first_t)
            } else {
                format!("{:>6.1}-{:>6.1}s", d.first_t, d.last_t)
            };
            out.status(&format!(
                "  [{}] ({:.2}, ×{}) {:>10}  {}",
                span,
                d.best_conf,
                d.n_frames,
                d.category,
                d.text
            ));
        }
        out.status(&format!("wrote {}", json_path.display()));
    }
    Ok(())
}

/// Resolve a video path. Today we only accept local files; B 站 BV/AV
/// support requires the user to download first (we suggest the exact
/// `bilitools download` command in the error message).
fn resolve_video_path(input: &str, output_dir: &PathBuf) -> Result<PathBuf, CliError> {
    let p = PathBuf::from(input);
    if p.is_file() {
        return p
            .canonicalize()
            .map_err(|e| CliError::Other(format!("canonicalize {input}: {e}")));
    }
    Err(CliError::Other(format!(
        "video not found at local path: {input}\n\
         \n\
         If this is a B 站 BV / AV id or URL, first download it:\n  \
           bilitools download {input} -o {}\n\
         \n\
         Then re-run:\n  \
           bilitools ocr {input} --video -o {}",
        output_dir.display(),
        output_dir.display()
    )))
}

fn parse_frame_ts(p: &std::path::Path) -> Option<f32> {
    let stem = p.file_stem()?.to_str()?;
    let t = stem.strip_prefix("frame_")?;
    t.parse().ok()
}

fn default_output_dir() -> PathBuf {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    PathBuf::from(format!("ocr_out/{ts}"))
}
