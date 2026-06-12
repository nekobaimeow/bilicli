// SPDX-License-Identifier: GPL-3.0-or-later
//! Pure-algorithm benchmark for the v3 explicit-stack loop.
//!
//! Runs the v3 two-pointer binary search on a fake 215-idx workload
//! WITHOUT invoking the OCR engine or reading jpg files. `ocr_frame`
//! is replaced by a closure that returns pre-built raws from a
//! HashMap, mimicking cache hit/miss behavior. This isolates the
//! pure-algorithm overhead — `primary_content_text` string building,
//! `Vec` allocation for the work stack, etc.
//!
//! Run with: `cargo test --release --lib -- --ignored v3_pure_algo --nocapture`

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use super::adaptive::AdaptiveSample;
use super::dedup::RawDetection;

/// Build fake raws for an idx. The text differs per-idx so the
/// algorithm can't exit early on lo_text==hi_text.
fn fake_raws(idx: i32) -> Vec<RawDetection> {
    // Body in the lower band (subtitle position: cy > 0.75 * 720 = 540).
    // primary_content_text builds fingerprint from non-watermark
    // detections sorted by (cy, cx), text `\n` join. We want
    // every idx to have a different text so the algorithm has to
    // recurse all the way down.
    vec![RawDetection {
        t_sec: idx as f32,
        text: format!("subtitle_at_{}_unique_text_token_xxxxx", idx),
        confidence: 0.95,
        bbox: [[100.0, 600.0], [1500.0, 600.0], [1500.0, 700.0], [100.0, 700.0]],
    }]
}

/// A no-op `ocr_frame` stand-in: returns pre-built raws for `idx`,
/// tracks hit/miss so we can assert we hit the right number of OCRs.
struct FakeCache {
    inner: HashMap<i32, (PathBuf, Vec<RawDetection>)>,
    calls: u32,
    cache_hits: u32,
}

impl FakeCache {
    fn new() -> Self {
        Self {
            inner: HashMap::new(),
            calls: 0,
            cache_hits: 0,
        }
    }
    fn get_or_fake(&mut self, idx: i32) -> Option<(PathBuf, Vec<RawDetection>, bool)> {
        if let Some(cached) = self.inner.get(&idx) {
            self.cache_hits += 1;
            return Some((cached.0.clone(), cached.1.clone(), true));
        }
        self.calls += 1;
        let raws = fake_raws(idx);
        let path = PathBuf::from(format!("/tmp/fake_frame_{:05}.jpg", idx + 1));
        let result = (path.clone(), raws);
        self.inner.insert(idx, result.clone());
        Some((result.0, result.1, false))
    }
}

/// Replicate the v3 explicit-stack loop from `adaptive::run` but
/// with `FakeCache::get_or_fake` instead of `ocr_frame`. This is
/// the exact same control flow — same 9 conditions, same work-stack
/// dispatch — just without the OCR engine or jpg decode.
fn v3_pure_algo(last_frame: i32) -> (Vec<AdaptiveSample>, u32) {
    let mut samples: Vec<AdaptiveSample> = Vec::new();
    let mut budget_remaining: u32 = u32::MAX;
    let mut ocr_calls: u32 = 0;
    let mut cache = FakeCache::new();

    let mut work: Vec<(
        i32, PathBuf, Vec<RawDetection>,
        i32, PathBuf, Vec<RawDetection>,
    )> = Vec::with_capacity(64);

    // Root: cache idx=0 and idx=last_frame
    let (lo_path, lo_raws, lo_cached) = cache.get_or_fake(0).unwrap();
    if !lo_cached { budget_remaining -= 1; ocr_calls += 1; }
    let (hi_path, hi_raws, hi_cached) = cache.get_or_fake(last_frame).unwrap();
    if !hi_cached { budget_remaining -= 1; ocr_calls += 1; }
    work.push((
        0, lo_path, lo_raws,
        last_frame, hi_path, hi_raws,
    ));

    // Main loop — copy of adaptive.rs while-let-some block.
    while let Some((
        lo_idx, lo_path, lo_raws,
        hi_idx, hi_path, hi_raws,
    )) = work.pop() {
        if budget_remaining == 0 { continue; }
        let lo_text_empty =
            lo_raws.is_empty() || super::adaptive::primary_content_text(&lo_raws).is_empty();
        if lo_text_empty {
            if lo_idx + 1 > hi_idx { continue; }
            let (new_hi_path, new_hi_raws, _) = cache.get_or_fake(hi_idx).unwrap();
            let (new_lo_path, new_lo_raws, _) = cache.get_or_fake(lo_idx + 1).unwrap();
            work.push((
                lo_idx + 1, new_lo_path, new_lo_raws,
                hi_idx, new_hi_path, new_hi_raws,
            ));
            continue;
        }
        let hi_text_empty =
            hi_raws.is_empty() || super::adaptive::primary_content_text(&hi_raws).is_empty();
        if hi_text_empty {
            if lo_idx > hi_idx - 1 { continue; }
            let (new_lo_path, new_lo_raws, _) = cache.get_or_fake(lo_idx).unwrap();
            let (new_hi_path, new_hi_raws, _) = cache.get_or_fake(hi_idx - 1).unwrap();
            work.push((
                lo_idx, new_lo_path, new_lo_raws,
                hi_idx - 1, new_hi_path, new_hi_raws,
            ));
            continue;
        }
        let lo_text = super::adaptive::primary_content_text(&lo_raws);
        let hi_text = super::adaptive::primary_content_text(&hi_raws);
        if lo_idx == hi_idx {
            samples.push(AdaptiveSample {
                frame: lo_path, t_sec: lo_idx as f32, raws: lo_raws,
            });
            continue;
        }
        if lo_text == hi_text {
            samples.push(AdaptiveSample {
                frame: lo_path, t_sec: lo_idx as f32, raws: lo_raws,
            });
            continue;
        }
        if budget_remaining == 0 { continue; }
        let mid_idx = (lo_idx + hi_idx) / 2;
        if mid_idx == lo_idx || mid_idx == hi_idx {
            samples.push(AdaptiveSample {
                frame: lo_path, t_sec: lo_idx as f32, raws: lo_raws,
            });
            samples.push(AdaptiveSample {
                frame: hi_path, t_sec: hi_idx as f32, raws: hi_raws,
            });
            continue;
        }
        let (mid_path, mid_raws, mid_cached) = cache.get_or_fake(mid_idx).unwrap();
        if !mid_cached {
            budget_remaining -= 1;
            ocr_calls += 1;
        }
        if mid_raws.is_empty() || super::adaptive::primary_content_text(&mid_raws).is_empty() {
            if lo_idx <= mid_idx - 1 {
                let (new_hi_path, new_hi_raws, _) = cache.get_or_fake(mid_idx - 1).unwrap();
                work.push((
                    lo_idx, lo_path.clone(), lo_raws.clone(),
                    mid_idx - 1, new_hi_path, new_hi_raws,
                ));
            }
            if mid_idx + 1 <= hi_idx {
                let (new_lo_path, new_lo_raws, _) = cache.get_or_fake(mid_idx + 1).unwrap();
                work.push((
                    mid_idx + 1, new_lo_path, new_lo_raws,
                    hi_idx, hi_path.clone(), hi_raws.clone(),
                ));
            }
            continue;
        }
        let mid_text = super::adaptive::primary_content_text(&mid_raws);
        if mid_text == lo_text {
            work.push((
                lo_idx, lo_path, lo_raws,
                mid_idx, mid_path, mid_raws,
            ));
            if mid_idx + 1 <= hi_idx {
                let (new_lo_path, new_lo_raws, _) = cache.get_or_fake(mid_idx + 1).unwrap();
                work.push((
                    mid_idx + 1, new_lo_path, new_lo_raws,
                    hi_idx, hi_path, hi_raws,
                ));
            }
        } else if mid_text == hi_text {
            if lo_idx <= mid_idx - 1 {
                let (new_hi_path, new_hi_raws, _) = cache.get_or_fake(mid_idx - 1).unwrap();
                work.push((
                    lo_idx, lo_path, lo_raws,
                    mid_idx - 1, new_hi_path, new_hi_raws,
                ));
            }
            work.push((
                mid_idx, mid_path, mid_raws,
                hi_idx, hi_path, hi_raws,
            ));
        } else {
            samples.push(AdaptiveSample {
                frame: mid_path, t_sec: mid_idx as f32, raws: mid_raws,
            });
            if lo_idx <= mid_idx - 1 {
                let (new_hi_path, new_hi_raws, _) = cache.get_or_fake(mid_idx - 1).unwrap();
                work.push((
                    lo_idx, lo_path, lo_raws,
                    mid_idx - 1, new_hi_path, new_hi_raws,
                ));
            }
            if mid_idx + 1 <= hi_idx {
                let (new_lo_path, new_lo_raws, _) = cache.get_or_fake(mid_idx + 1).unwrap();
                work.push((
                    mid_idx + 1, new_lo_path, new_lo_raws,
                    hi_idx, hi_path, hi_raws,
                ));
            }
        }
    }
    samples.sort_by(|a, b| a.t_sec.partial_cmp(&b.t_sec).unwrap_or(std::cmp::Ordering::Equal));
    (samples, ocr_calls)
}

#[test]
#[cfg_attr(not(test), ignore = "benchmark; run with `cargo test -- --ignored`")]
#[ignore]
fn v3_pure_algo_under_3s_for_215s_workload() {
    let last_frame = 214; // 215s video, 0..=214 = 215 frames
    let t0 = Instant::now();
    let (samples, ocr_calls) = v3_pure_algo(last_frame);
    let elapsed = t0.elapsed();

    eprintln!(
        "v3_pure_algo(last_frame={}): {} samples, {} ocr_calls, {:?} elapsed (limit 3s)",
        last_frame, samples.len(), ocr_calls, elapsed
    );

    assert!(
        ocr_calls > 0 && ocr_calls <= last_frame as u32,
        "ocr_calls should be in (0, {}], got {}",
        last_frame + 1,
        ocr_calls
    );
    assert!(
        elapsed < Duration::from_secs(3),
        "v3 pure algorithm took {:?} (>3s limit) for 215-idx workload",
        elapsed
    );
}
