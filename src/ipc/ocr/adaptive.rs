// SPDX-License-Identifier: GPL-3.0-or-later
//! Adaptive-sampling OCR driver.
//!
//! Algorithm (the C-variant the user described):
//!
//!   1. Start with one range: `[0, duration]`
//!   2. Pop a range, OCR its midpoint frame
//!   3. If the OCR result is empty / noise → drop the range, don't
//!      recurse (the whole sub-range is text-free)
//!   4. Otherwise split the range into two halves and push them back
//!   5. Continue until the range is shorter than `min_segment_sec` or
//!      we hit `max_ocr_calls`
//!   6. After all OCR is done, sort samples by time, then walk adjacent
//!      pairs and **drop** any whose detections are "basically the same"
//!      as the previous one (this is the "stop if left ≈ right" part —
//!      implemented as a post-pass rather than inline, because the
//!      sampling order isn't strictly time-sequential)
//!
//! Total OCRs: O(log2(duration / min_segment)) × 2 per leaf → roughly
//! 12-20 for a 5-min video. A static video with no text → 1 OCR
//! (root frame is empty → bail out). A frame-by-frame title sequence
//! → log2(N) per title.

use image::DynamicImage;
use std::path::{Path, PathBuf};

use super::dedup::{bbox_iou, text_distance_ratio, RawDetection};
use super::engine::OcrEngine;
use super::frames;

/// Configuration for the adaptive sampler.
#[derive(Debug, Clone)]
pub struct AdaptiveConfig {
    /// Stop splitting once a time range is shorter than this many
    /// seconds. Default 3.0 — finer than this is rarely useful since
    /// the smallest readable title is ~2s.
    pub min_segment_sec: f32,
    /// Hard cap on total OCR calls across the whole tree. Default
    /// 200 — same as the legacy `max_frames` ceiling.
    pub max_ocr_calls: u32,
    /// Bbox IoU threshold for "same region" during the dedup-stop
    /// short-circuit. Default 0.6.
    pub iou_thresh: f32,
    /// Text similarity threshold for "basically the same text"
    /// (char-bag distance). Default 0.5.
    pub text_sim_thresh: f32,
    /// Minimum detection confidence. Sub-threshold detections are
    /// filtered out before the noise check, so a noisy frame
    /// doesn't pollute the recursion decisions.
    pub min_conf: f32,
}

impl Default for AdaptiveConfig {
    fn default() -> Self {
        Self {
            min_segment_sec: 3.0,
            max_ocr_calls: 200,
            iou_thresh: 0.6,
            text_sim_thresh: 0.5,
            min_conf: 0.45,
        }
    }
}

/// One OCR sample: a frame + the detections extracted from it.
#[derive(Debug, Clone)]
pub struct AdaptiveSample {
    /// Path to the extracted jpg.
    pub frame: PathBuf,
    /// Timestamp in seconds.
    pub t_sec: f32,
    /// OCR detections (already filtered for `min_conf`).
    pub raws: Vec<RawDetection>,
}

/// Run the adaptive sampler. Returns samples in time order with the
/// dedup-stop pass already applied.
pub async fn run(
    engine: &OcrEngine,
    video: &Path,
    frames_dir: &Path,
    duration_sec: f32,
    cfg: &AdaptiveConfig,
) -> Vec<AdaptiveSample> {
    let mut samples: Vec<AdaptiveSample> = Vec::new();
    let mut budget = cfg.max_ocr_calls;

    // BFS work queue: (t_start, t_end). Using VecDeque as a FIFO
    // queue so we process ranges in BREADTH-FIRST order. This
    // matters: with a plain Vec used as a stack (LIFO), a video
    // that has text in the right half (e.g. chapter titles spread
    // out evenly) would keep splitting the right half recursively
    // and starve the left half entirely. BFS guarantees both halves
    // get explored at each depth before we recurse deeper.
    use std::collections::VecDeque;
    let mut queue: VecDeque<(f32, f32)> = VecDeque::new();
    queue.push_back((0.0, duration_sec));

    while let Some((t_start, t_end)) = queue.pop_front() {
        tracing::info!("queue pop [{}, {}], budget={}, samples={}", t_start, t_end, budget, samples.len());
        if budget == 0 {
            tracing::warn!("OCR budget exhausted after {} samples", samples.len());
            break;
        }
        let span = t_end - t_start;
        if span < cfg.min_segment_sec {
            continue;
        }
        let t_mid = (t_start + t_end) * 0.5;

        // ---- OCR the midpoint frame ----
        let frame = match frames::extract_single_frame(video, frames_dir, t_mid).await {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!("extract_single_frame({t_mid:.2}s) failed: {e}");
                continue;
            }
        };
        let img: DynamicImage = match image::open(&frame) {
            Ok(i) => i,
            Err(e) => {
                tracing::warn!("open {} failed: {e}", frame.display());
                continue;
            }
        };
        let dets = match engine.recognize(&img) {
            Ok(d) => d,
            Err(e) => {
                tracing::warn!("ocr at {t_mid:.2}s failed: {e}");
                continue;
            }
        };
        budget = budget.saturating_sub(1);

        let raws: Vec<RawDetection> = dets
            .into_iter()
            .filter(|d| d.confidence >= cfg.min_conf)
            .map(|d| RawDetection {
                t_sec: t_mid,
                text: d.text,
                confidence: d.confidence,
                bbox: d.bbox,
            })
            .collect();

        // Filter OCR noise: single Latin glyphs, empty strings.
        let valid: Vec<RawDetection> = raws
            .iter()
            .filter(|r| is_meaningful_text(&r.text))
            .cloned()
            .collect();

        if valid.is_empty() {
            // No readable text in this frame → assume the whole range
            // [t_start, t_end] is text-free. Don't split further.
            continue;
        }

        samples.push(AdaptiveSample {
            frame,
            t_sec: t_mid,
            raws: valid,
        });

        tracing::info!("OCR @ {:.2}s had valid text → split (span {:.1}s)", t_mid, span);

        // Split the range and enqueue both halves. The half-span must
        // be >= min_segment_sec for the recursion to do anything
        // useful. We push LEFT first, then RIGHT, so the FIFO queue
        // pops LEFT before RIGHT (matches the user's spec: "take
        // the midpoint, then recurse into [left, mid] and [mid,
        // right] — if [left, mid] is the same, drop it").
        if span * 0.5 >= cfg.min_segment_sec {
            queue.push_back((t_start, t_mid));
            queue.push_back((t_mid, t_end));
            tracing::info!("  enqueued [{}, {}] + [{}, {}]; queue.len={}", t_start, t_mid, t_mid, t_end, queue.len());
        } else {
            tracing::info!("  half-span {:.2}s < min_segment {:.2}s → no split", span * 0.5, cfg.min_segment_sec);
        }
    }

    // ---- Sort by time ----
    samples.sort_by(|a, b| a.t_sec.partial_cmp(&b.t_sec).unwrap_or(std::cmp::Ordering::Equal));

    // ---- Dedup-stop pass ----
    //
    // Walk the sorted samples; if sample[i+1] is "basically the same
    // content" as sample[i] (matching the user's spec: "if left image
    // ≈ right image, stop"), drop sample[i+1].
    let mut kept: Vec<AdaptiveSample> = Vec::with_capacity(samples.len());
    for s in samples {
        if let Some(prev) = kept.last() {
            if clusters_match(&prev.raws, &s.raws, cfg.iou_thresh, cfg.text_sim_thresh) {
                // Skip — it's a duplicate of the previous sample.
                continue;
            }
        }
        kept.push(s);
    }

    kept
}

/// A "raw detection" is meaningful (worth recursing on) if:
///  - non-empty after trim
///  - has at least one CJK character OR is at least 4 ASCII chars
///
/// This filters out the common OCR noise pattern on B 站: a
/// watermark's tiny "bilibili" suffix (or 1-2 Latin glyphs) being
/// mistakenly recognized from a different part of the frame.
pub fn is_meaningful_text(s: &str) -> bool {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return false;
    }
    let n_chars = trimmed.chars().count();
    let n_cjk = trimmed
        .chars()
        .filter(|c| {
            matches!(c,
                '\u{4E00}'..='\u{9FFF}'  // CJK Unified
                | '\u{3040}'..='\u{309F}'  // Hiragana
                | '\u{30A0}'..='\u{30FF}'  // Katakana
                | '\u{AC00}'..='\u{D7AF}'  // Hangul
            )
        })
        .count();
    n_cjk >= 1 || n_chars >= 4
}

/// Are two detection clusters "basically the same"?
pub fn clusters_match(
    a: &[RawDetection],
    b: &[RawDetection],
    iou_thresh: f32,
    text_thresh: f32,
) -> bool {
    for da in a {
        for db in b {
            if bbox_iou(&da.bbox, &db.bbox) >= iou_thresh
                && text_distance_ratio(&da.text, &db.text) <= text_thresh
            {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn det(text: &str, bbox: [[f32; 2]; 4]) -> RawDetection {
        RawDetection {
            t_sec: 0.0,
            text: text.into(),
            confidence: 0.9,
            bbox,
        }
    }

    #[test]
    fn is_meaningful_filters_latin_glyphs() {
        assert!(!is_meaningful_text(""));
        assert!(!is_meaningful_text("   "));
        assert!(!is_meaningful_text("li"));
        assert!(!is_meaningful_text("bl"));
    }

    #[test]
    fn is_meaningful_keeps_cjk() {
        assert!(is_meaningful_text("出"));
        assert!(is_meaningful_text("bilibili"));  // 8 ASCII chars
        assert!(is_meaningful_text("风景旅行收藏家"));
    }

    #[test]
    fn clusters_match_same_region_similar_text() {
        // Both have the watermark bbox (right-top) and overlapping
        // character bag.
        let bbox = [[1445.0, 21.0], [1903.0, 21.0], [1903.0, 109.0], [1445.0, 109.0]];
        let a = vec![det("风景旅行收藏家bilbi", bbox)];
        let b = vec![det("行收藏家bi", bbox)];
        assert!(clusters_match(&a, &b, 0.6, 0.5));
    }

    #[test]
    fn clusters_match_different_region() {
        let top = [[1445.0, 21.0], [1903.0, 21.0], [1903.0, 109.0], [1445.0, 109.0]];
        let bot = [[775.0, 877.0], [1143.0, 877.0], [1143.0, 959.0], [775.0, 959.0]];
        let a = vec![det("风景旅行收藏家", top)];
        let b = vec![det("桂林雨中游湖", bot)];
        assert!(!clusters_match(&a, &b, 0.6, 0.5));
    }
}
