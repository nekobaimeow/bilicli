// SPDX-License-Identifier: GPL-3.0-or-later
//! Thin Rust wrapper around the `ocr-rs` high-level engine.
//!
//! The configuration mirrors the one used in the rpic project (which we
//! validated works for B 站 videos): fast detection options, no rayon
//! parallel mode (which would compete with MNN's own threads),
//! min-result-confidence 0.45, and the standard PP-OCRv5 mobile
//! detection / recognition options.

use image::DynamicImage;
use imageproc::point::Point;
use ocr_rs::{DetOptions, OcrEngine as PaddleOcrEngine, OcrEngineConfig, RecOptions};

use super::model_paths::ModelPaths;

/// A single OCR detection result, flattened for serialization.
#[derive(Debug, Clone, serde::Serialize)]
pub struct DetectedText {
    /// Recognized text.
    pub text: String,
    /// Confidence in [0, 1].
    pub confidence: f32,
    /// Four corner points of the (rotated) bounding box, in image
    /// coordinates, top-left → top-right → bottom-right → bottom-left.
    /// Falls back to the axis-aligned rectangle when the engine
    /// didn't return rotated corners.
    pub bbox: [[f32; 2]; 4],
}

/// Offline OCR engine backed by PaddleOCR + MNN.
pub struct OcrEngine {
    inner: PaddleOcrEngine,
}

impl OcrEngine {
    /// Build the engine from a resolved model path group.
    pub fn load(paths: &ModelPaths) -> Result<Self, String> {
        let config = OcrEngineConfig::fast()
            .with_threads(default_threads())
            .with_parallel(false)
            .with_min_result_confidence(0.45)
            .with_det_options(
                DetOptions::fast()
                    .with_max_side_len(960)
                    .with_merge_boxes(true)
                    .with_merge_threshold(8),
            )
            .with_rec_options(
                RecOptions::new()
                    .with_min_score(0.25)
                    .with_batch_size(8),
            );

        let inner = PaddleOcrEngine::new(&paths.det, &paths.rec, &paths.charset, Some(config))
            .map_err(|e| format!("load OCR model: {e}"))?;

        Ok(Self { inner })
    }

    /// Recognize all text regions in `image`. Results are sorted
    /// top-down then left-to-right (rpic-style: `(top / 12, left)`).
    pub fn recognize(&self, image: &DynamicImage) -> Result<Vec<DetectedText>, String> {
        let mut results = self
            .inner
            .recognize(image)
            .map_err(|e| format!("OCR: {e}"))?;

        // Top-down, left-right sort, matching rpic's `clean_ocr_text` line
        // ordering so multi-line text comes out in reading order.
        results.sort_by_key(|r| {
            let rect = &r.bbox.rect;
            (rect.top() / 12, rect.left())
        });

        Ok(results
            .into_iter()
            .map(|r| DetectedText {
                text: r.text,
                confidence: r.confidence,
                bbox: bbox_to_array(&r.bbox),
            })
            .collect())
    }
}

fn default_threads() -> i32 {
    if let Ok(value) = std::env::var("BILITOOLS_OCR_THREADS") {
        if let Ok(n) = value.parse::<i32>() {
            return n.clamp(1, 8);
        }
    }
    std::thread::available_parallelism()
        .map(|n| n.get().min(3) as i32)
        .unwrap_or(2)
}

/// Convert a `TextBox` to a flat `[[f32; 2]; 4]` array. If the engine
/// returned rotated corners (DB-postprocess style), use them; otherwise
/// project the axis-aligned `Rect` to its four corners.
fn bbox_to_array(b: &ocr_rs::TextBox) -> [[f32; 2]; 4] {
    if let Some(pts) = &b.points {
        [
            [pts[0].x, pts[0].y],
            [pts[1].x, pts[1].y],
            [pts[2].x, pts[2].y],
            [pts[3].x, pts[3].y],
        ]
    } else {
        let r = &b.rect;
        let x0 = r.left() as f32;
        let y0 = r.top() as f32;
        let x1 = (r.left() + r.width() as i32) as f32;
        let y1 = (r.top() + r.height() as i32) as f32;
        [[x0, y0], [x1, y0], [x1, y1], [x0, y1]]
    }
}

// Suppress an "unused import" warning if the consumer of this module
// happens to also bring `Point` in — we re-export it for downstream.
#[allow(dead_code)]
type _PointF32 = Point<f32>;
