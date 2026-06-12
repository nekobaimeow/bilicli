// SPDX-License-Identifier: GPL-3.0-or-later
//! Locate the PaddleOCR (MNN) model files on disk.
//!
//! Search order (mirrors rpic's `offline_model_dirs`):
//!
//! 1. `$BILITOOLS_OCR_MODEL_DIR`
//! 2. `<executable-dir>/models/ocr-fast/`
//! 3. `<executable-dir>/`
//! 4. `<cwd>/models/ocr-fast/`
//! 5. `<cwd>/`
//!
//! Within each directory we try three model profiles in order of preference:
//! PP-OCRv5 FP16 (fastest, ~10 MB) → PP-OCRv5 (full precision) → PP-OCRv4
//! (fallback for users who already have legacy models).

use std::path::{Path, PathBuf};

/// All three model files for one PaddleOCR profile.
#[derive(Debug, Clone)]
pub struct ModelPaths {
    /// Text-detection MNN model.
    pub det: PathBuf,
    /// Text-recognition MNN model.
    pub rec: PathBuf,
    /// Character-set file (one token per line, with the special blank
    /// token at index 0 if the model was exported with one).
    pub charset: PathBuf,
}

/// Find an OCR model group on disk. Returns the first matching profile
/// in the first matching directory.
///
/// The error message lists every directory we searched so the user can
/// drop the models into one of the canonical locations.
pub fn find_model() -> Result<ModelPaths, String> {
    let dirs = model_dirs();

    // (label, det file, rec file, charset file) — tried in this order.
    let profiles: &[(&str, &str, &str, &str)] = &[
        (
            "PP-OCRv5 FP16 (recommended, ~10 MB)",
            "PP-OCRv5_mobile_det_fp16.mnn",
            "PP-OCRv5_mobile_rec_fp16.mnn",
            "ppocr_keys_v5.txt",
        ),
        (
            "PP-OCRv5 (full precision, ~25 MB)",
            "PP-OCRv5_mobile_det.mnn",
            "PP-OCRv5_mobile_rec.mnn",
            "ppocr_keys_v5.txt",
        ),
        (
            "PP-OCRv4 (legacy)",
            "ch_PP-OCRv4_det_infer.mnn",
            "ch_PP-OCRv4_rec_infer.mnn",
            "ppocr_keys_v4.txt",
        ),
    ];

    for dir in &dirs {
        for (label, det, rec, charset) in profiles {
            let candidate = ModelPaths {
                det: dir.join(det),
                rec: dir.join(rec),
                charset: dir.join(charset),
            };
            if candidate.det.is_file() && candidate.rec.is_file() && candidate.charset.is_file() {
                tracing::info!("OCR model resolved: {} in {}", label, dir.display());
                return Ok(candidate);
            }
        }
    }

    let searched = dirs
        .iter()
        .map(|d| format!("  {}", d.display()))
        .collect::<Vec<_>>()
        .join("\n");
    Err(format!(
        "OCR model files not found. Place a PaddleOCR/MNN model group in one of:\n{searched}\n\n\
         Or set BILITOOLS_OCR_MODEL_DIR to your model directory. Recommended group:\n\
           PP-OCRv5_mobile_det_fp16.mnn\n\
           PP-OCRv5_mobile_rec_fp16.mnn\n\
           ppocr_keys_v5.txt"
    ))
}

/// Collect the candidate model directories in search order.
fn model_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();

    if let Ok(raw) = std::env::var("BILITOOLS_OCR_MODEL_DIR") {
        if !raw.trim().is_empty() {
            dirs.push(PathBuf::from(raw));
        }
    }

    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            dirs.push(parent.to_path_buf());
            dirs.push(parent.join("models").join("ocr-fast"));
        }
    }

    if let Ok(cwd) = std::env::current_dir() {
        dirs.push(cwd.clone());
        dirs.push(cwd.join("models").join("ocr-fast"));
    }

    dirs
}

/// True if `path` looks like a local file we can read (image or video).
/// Used by the CLI dispatcher to decide between image-OCR and
/// video-OCR modes when the input string is a path rather than a BV/AV.
pub fn is_local_path(input: &str) -> bool {
    Path::new(input).is_file()
}
