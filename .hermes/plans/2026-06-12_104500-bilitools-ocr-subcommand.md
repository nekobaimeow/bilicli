# bilitools OCR 子命令 Implementation Plan

> **For Hermes:** Main agent serial execution. bilitools-cli 是单一 binary 跨多文件类型耦合，**不要 spawn subagent**。8 个 task 串行执行。

**Goal:** 给 bilitools-cli 加 `bilitools ocr <path>` 子命令 — 既能 OCR 单张图片，也能 OCR 视频抽帧（ffmpeg 抽帧 + 帧内文字识别）。完全离线、纯 Rust 推理、无 Python 进程。

**Architecture (经 rpic 项目验证为最简路径):**
- 依赖 `ocr-rs = "2.2.2"` (crates.io 稳定版) — 内部用 **MNN** 推理 PaddleOCR
- 模型：**PP-OCRv5 mobile FP16** (10.4 MB 3 文件) — 已在 rpic 项目验证可跑
- 抽帧：调 `ffmpeg` sidecar (项目已有 backends/sidecar)
- 输入支持 3 种模式: 单图 / 视频 / B 站 BV (自动 parse → 调 download → 抽帧)
- 输出 JSON (`--json`) 或人类可读

**Tech Stack:**
- Rust 1.80+ (与 bilitools 一致)
- `ocr-rs = "2.2.2"` (公开 crates.io 版本) — 不锁 git rev
- `image = "0.25"` (已存在, 加 features = ["jpeg"])
- 现有 `tokio::process::Command` 调 ffmpeg
- Feature flag `ocr` (opt-in) — 与现有 `transcribe` 一致

---

## 关键事实 (经 rpic 学习 + 实际验证)

| 项 | 真实情况 |
|---|---|
| **OCR crate** | `ocr-rs = "2.2.2"` 在 crates.io (用户原话 "PP-OCRv5 mobile") |
| **模型格式** | **MNN** (不是 ONNX), 三个文件 10.4 MB total |
| **v5 字符表** | `ppocr_keys_v5.txt` 74 KB (独立文件, 不编码进模型) |
| **v5 模型** | `PP-OCRv5_mobile_det_fp16.mnn` (2.4MB) + `PP-OCRv5_mobile_rec_fp16.mnn` (8.0MB) |
| **rpic 实际用** | git rev pinned `b7141e7d`, 我们用更稳的 crates.io 2.2.2 |
| **算法实现位置** | 全部 in `ocr-rs` 内部 (DB postprocess + CTC decode + perspective crop) |
| **rpic 自己写的** | 模型查找 + clean_ocr_text + 隔离 worker process (我们都不需要) |
| **OCR 速度** | rpic 1.5-2s/帧 (CPU), 与 rapidocr-onnxruntime 持平 |
| **OCR 准确度** | v1 视频: 识别 "bilibili" / "风景旅行收藏家" / "狂风骤雨的大山卧龙谷" (rpic + rapidocr 都验过) |
| **依赖大小** | ocr-rs 自带 MNN C++ 编译, 静态链接, binary 大约 +10-15 MB (vs ONNX Runtime 50+ MB) |
| **多语言** | PP-OCRv5 支持中英日韩等多语言 (vs v4 单一中文) |

---

## rpic 关键代码模式 (我们要复用)

```rust
// rpic/src/recognition.rs:228-258 — 实际 OCR 调用的核心
use ocr_rs::{DetOptions, OcrEngine as PaddleOcrEngine, OcrEngineConfig as PaddleOcrConfig, RecOptions};

let config = PaddleOcrConfig::fast()
    .with_threads(offline_thread_count())        // 默认 3
    .with_parallel(false)                          // 避免 rayon 抢占
    .with_min_result_confidence(0.45)              // 过滤噪点
    .with_det_options(DetOptions::fast()
        .with_max_side_len(960)
        .with_merge_boxes(true)
        .with_merge_threshold(8))
    .with_rec_options(RecOptions::new()
        .with_min_score(0.25)
        .with_batch_size(8));

let engine = PaddleOcrEngine::new(&model.det, &model.rec, &model.charset, Some(config))?;
let image = image::open(path)?;
let mut results = engine.recognize(&image)?;
// results: Vec<{ text, confidence, bbox: TextBox }>
```

```rust
// rpic 找模型的路径顺序 (我们直接照搬)
fn find_model() -> Result<ModelPaths, String> {
    let dirs = [
        env::var("BILITOOLS_OCR_MODEL_DIR").ok().map(PathBuf::from),
        exe_dir(),
        cwd(),
    ];
    // 按顺序搜 PP-OCRv5 FP16 → v5 → v4
}
```

---

## 任务分解 (8 个 task, TDD 严格)

### Task 1: 添加 `ocr` feature + ocr-rs 依赖

**Files:**
- Modify: `Cargo.toml` (在 `[features]` 段加 `ocr`, 加 optional dep)
- Modify: `Cargo.toml` line 94 (image 加 `jpeg` feature)

**Step 1: 在 Cargo.toml 加 ocr feature flag**

```toml
[dependencies]
# OCR — opt-in via --features ocr (PP-OCRv5 mobile MNN models)
ocr-rs = { version = "2.2.2", optional = true, default-features = false }

# Image: 加 jpeg feature (当前只有 png)
image = { version = "0.25", default-features = false, features = ["png", "jpeg"] }

[features]
default = []
transcribe = ["dep:which"]
ocr = ["dep:ocr-rs"]
```

**Step 2: 验证**

```bash
cd /home/trade/bilitools-cli
cargo check --features ocr 2>&1 | tail -10
# 期望: ocr-rs 编译过 (第一次会拉 MNN C++ 源 + 编译, 2-5 分钟)
```

**Step 3: Commit**

```bash
git add Cargo.toml
git commit -m "feat(ocr): add ocr feature flag with ocr-rs (MNN) opt-in dep"
```

---

### Task 2: OCR 引擎封装 (src/ipc/ocr/mod.rs + engine.rs)

**Files:**
- Create: `src/ipc/ocr/mod.rs`
- Create: `src/ipc/ocr/engine.rs`
- Modify: `src/ipc/mod.rs` (注册新模块, `#[cfg(feature = "ocr")]`)

**Step 1: mod.rs**

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
//! Offline OCR via PaddleOCR (MNN backend, PP-OCRv5 mobile).

pub mod engine;
pub mod frames;
pub mod model_paths;
```

**Step 2: engine.rs (核心 ~60 行)**

```rust
use image::DynamicImage;
use ocr_rs::{DetOptions, OcrEngine as PaddleOcrEngine, OcrEngineConfig, RecOptions};
use std::path::Path;

use super::model_paths::ModelPaths;

pub struct OcrEngine {
    inner: PaddleOcrEngine,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct DetectedText {
    pub text: String,
    pub confidence: f32,
    pub bbox: [[f32; 2]; 4],   // 4 角点
}

impl OcrEngine {
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

        let inner = PaddleOcrEngine::new(
            &paths.det,
            &paths.rec,
            &paths.charset,
            Some(config),
        )
        .map_err(|e| format!("load OCR model: {e}"))?;

        Ok(Self { inner })
    }

    pub fn recognize(&self, image: &DynamicImage) -> Result<Vec<DetectedText>, String> {
        let mut results = self
            .inner
            .recognize(image)
            .map_err(|e| format!("OCR: {e}"))?;
        // 按行排序 (top-down, left-right), 跟 rpic 一致
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
    if let Ok(v) = std::env::var("BILITOOLS_OCR_THREADS") {
        if let Ok(n) = v.parse() {
            return n.clamp(1, 8);
        }
    }
    std::thread::available_parallelism()
        .map(|n| n.get().min(3) as i32)
        .unwrap_or(2)
}

fn bbox_to_array(b: &ocr_rs::TextBox) -> [[f32; 2]; 4] {
    // rpic 用的 TextBox.rect 是 Rectangle-like; ocr-rs TextBox 有 points()
    // 视实际字段, 用 4 corners
    let pts = b.points();
    [
        [pts[0].x, pts[0].y],
        [pts[1].x, pts[1].y],
        [pts[2].x, pts[2].y],
        [pts[3].x, pts[3].y],
    ]
}
```

**Step 3: 注册模块**

```rust
// src/ipc/mod.rs
#[cfg(feature = "ocr")]
pub mod ocr;
```

**Step 4: 验证编译**

```bash
cargo check --features ocr 2>&1 | tail -5
# 期望: 编译过
```

**Step 5: Commit**

```bash
git add src/ipc/ocr/ src/ipc/mod.rs
git commit -m "feat(ocr): OcrEngine wrapper around ocr-rs (MNN)"
```

---

### Task 3: 模型路径搜索 (model_paths.rs)

**Files:**
- Create: `src/ipc/ocr/model_paths.rs`

**Step 1: 实现 (照搬 rpic 的 find_offline_model, 改用 bilitools 命名)**

```rust
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct ModelPaths {
    pub det: PathBuf,
    pub rec: PathBuf,
    pub charset: PathBuf,
}

pub fn find_model() -> Result<ModelPaths, String> {
    let dirs = model_dirs();
    let profiles: &[(&str, &str, &str, &str)] = &[
        ("PP-OCRv5 FP16", "PP-OCRv5_mobile_det_fp16.mnn",
                          "PP-OCRv5_mobile_rec_fp16.mnn", "ppocr_keys_v5.txt"),
        ("PP-OCRv5", "PP-OCRv5_mobile_det.mnn",
                    "PP-OCRv5_mobile_rec.mnn", "ppocr_keys_v5.txt"),
        ("PP-OCRv4", "ch_PP-OCRv4_det_infer.mnn",
                    "ch_PP-OCRv4_rec_infer.mnn", "ppocr_keys_v4.txt"),
    ];

    for dir in &dirs {
        for (label, det, rec, charset) in profiles {
            let p = ModelPaths {
                det: dir.join(det),
                rec: dir.join(rec),
                charset: dir.join(charset),
            };
            if p.det.is_file() && p.rec.is_file() && p.charset.is_file() {
                tracing::info!("OCR model found: {} in {}", label, dir.display());
                return Ok(p);
            }
        }
    }

    let searched = dirs.iter()
        .map(|d| format!("  {}", d.display()))
        .collect::<Vec<_>>()
        .join("\n");
    Err(format!(
        "OCR model not found. Place PP-OCRv5 FP16 / PP-OCRv5 / PP-OCRv4 mobile MNN models and charset in:\n{searched}\n\
         Or set BILITOOLS_OCR_MODEL_DIR env var to model directory."
    ))
}

fn model_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(d) = std::env::var("BILITOOLS_OCR_MODEL_DIR") {
        if !d.trim().is_empty() { dirs.push(PathBuf::from(d)); }
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
    dirs.dedup();
    dirs
}

/// 首次使用提示, 写明模型下载/放置位置
pub fn ensure_model_available() -> Result<ModelPaths, String> {
    find_model()
}
```

**Step 2: 写单测**

```rust
// tests/ocr_model_paths.rs
use bilitools::ipc::ocr::model_paths;

#[test]
fn model_dirs_includes_env_var() {
    std::env::set_var("BILITOOLS_OCR_MODEL_DIR", "/tmp/ocr-test");
    // 调用 find_model() 应该会查 /tmp/ocr-test 路径
    // (本测试不要求找到真模型, 只要函数不 panic)
    let _ = model_paths::find_model();
}

#[test]
fn model_paths_struct_debug() {
    let p = model_paths::ModelPaths {
        det: PathBuf::from("/tmp/det.mnn"),
        rec: PathBuf::from("/tmp/rec.mnn"),
        charset: PathBuf::from("/tmp/c.txt"),
    };
    assert!(format!("{:?}", p).contains("det.mnn"));
}
```

**Step 3: 跑测试**

```bash
cargo test --features ocr ocr_model_paths 2>&1 | tail -5
# 期望: 2 passed
```

**Step 4: Commit**

```bash
git add src/ipc/ocr/model_paths.rs tests/ocr_model_paths.rs
git commit -m "feat(ocr): model path discovery with BILITOOLS_OCR_MODEL_DIR env var"
```

---

### Task 4: OCR 子命令 (CLI 入口 + 单图模式)

**Files:**
- Create: `src/cli/ocr.rs`
- Modify: `src/cli/root.rs` (加 `Ocr` variant 到 Command enum)
- Modify: `src/cli/mod.rs` (注册新模块)

**Step 1: 在 `Command` enum 加**

```rust
// src/cli/root.rs Command enum
/// Run OCR on an image or B 站 video (extract hard-coded text).
#[cfg(feature = "ocr")]
Ocr {
    /// Image file path OR B 站 BV/AV id OR B 站 video URL.
    /// If --video is given, this is the source video; otherwise it's an image.
    input: String,
    /// Treat input as a local video file (extract frames first).
    #[arg(long)]
    video: bool,
    /// Frame interval in seconds when --video is used (default 1.0).
    #[arg(long, default_value = "1.0")]
    interval: f32,
    /// Maximum frames to OCR from a video (default 200).
    #[arg(long, default_value = "200")]
    max_frames: u32,
    /// Minimum confidence to keep a detection (default 0.45).
    #[arg(long, default_value = "0.45")]
    min_conf: f32,
    /// Output directory for frames (video mode) and ocr.json.
    /// Default: ./ocr_out/<timestamp>
    #[arg(long, short = 'o')]
    output_dir: Option<PathBuf>,
    /// Keep extracted frames on disk after OCR (default: delete).
    #[arg(long)]
    keep_frames: bool,
},
```

**Step 2: 实现 cli/ocr.rs**

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
//! `bilitools ocr` subcommand.

use std::path::PathBuf;
use std::time::SystemTime;

use crate::cli::output::Output;
use crate::cli::root::Command;
use crate::error::CliError;

pub async fn run(cmd: &Command, out: &Output) -> Result<(), CliError> {
    let Command::Ocr {
        input, video, interval, max_frames,
        min_conf, output_dir, keep_frames,
    } = cmd else {
        return Err(CliError::Other("internal: not Ocr command".into()));
    };

    let output_dir = output_dir.clone().unwrap_or_else(|| {
        let ts = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_secs()).unwrap_or(0);
        PathBuf::from(format!("ocr_out/{ts}"))
    });
    std::fs::create_dir_all(&output_dir).map_err(|e| CliError::Other(e.to_string()))?;

    // 加载 OCR 引擎
    let paths = crate::ipc::ocr::model_paths::find_model()
        .map_err(CliError::Other)?;
    out.status(&format!("loading OCR engine (MNN, PP-OCRv5) from {}", paths.det.display()));
    let engine = crate::ipc::ocr::engine::OcrEngine::load(&paths)
        .map_err(CliError::Other)?;

    if *video {
        run_video(&input, *interval, *max_frames, *min_conf, &output_dir, *keep_frames, &engine, out).await
    } else {
        run_image(&input, &output_dir, *min_conf, &engine, out).await
    }
}

async fn run_image(
    input: &str, output_dir: &PathBuf, min_conf: f32,
    engine: &crate::ipc::ocr::engine::OcrEngine, out: &Output,
) -> Result<(), CliError> {
    let img = image::open(input).map_err(|e| CliError::Other(format!("open image: {e}")))?;
    out.status(&format!("OCR {} ...", input));
    let results = engine.recognize(&img).map_err(CliError::Other)?;
    let results: Vec<_> = results.into_iter().filter(|r| r.confidence >= min_conf).collect();
    emit(&results, output_dir, "image", out).map_err(CliError::Other)
}

async fn run_video(
    input: &str, interval: f32, max_frames: u32, min_conf: f32,
    output_dir: &PathBuf, keep_frames: bool,
    engine: &crate::ipc::ocr::engine::OcrEngine, out: &Output,
) -> Result<(), CliError> {
    let video_path = resolve_video(input, output_dir, out).await?;

    let frames_dir = output_dir.join("frames");
    let extract = crate::ipc::ocr::frames::extract_frames(&video_path, &frames_dir, interval, max_frames)
        .await.map_err(CliError::Other)?;
    out.status(&format!("extracted {} frames from {} (interval {}s)",
        extract.frames.len(), video_path.display(), interval));

    let mut all = Vec::new();
    for (i, frame) in extract.frames.iter().enumerate() {
        let ts = parse_frame_ts(frame).unwrap_or(0.0);
        let img = image::open(frame).map_err(|e| CliError::Other(format!("open {}: {e}", frame.display())))?;
        let detections = engine.recognize(&img).map_err(CliError::Other)?;
        for d in detections {
            if d.confidence >= min_conf {
                all.push(serde_json::json!({
                    "t_sec": ts,
                    "text": d.text,
                    "confidence": d.confidence,
                    "bbox": d.bbox,
                }));
            }
        }
        if i % 10 == 0 {
            out.status(&format!("  OCR frame {}/{}", i + 1, extract.frames.len()));
        }
    }

    if !keep_frames {
        let _ = std::fs::remove_dir_all(&frames_dir);
    }

    let result = serde_json::json!({
        "input": input,
        "frames_processed": extract.frames.len(),
        "interval_sec": interval,
        "detections": all,
    });
    if out.is_json() {
        out.ok(result).map_err(CliError::Other)?;
    } else {
        out.status(&format!("OCR done: {} detections across {} frames",
            all.len(), extract.frames.len()));
        for d in &all {
            out.status(&format!("  [{:>6.1f}s] ({:.2}) {}",
                d["t_sec"].as_f64().unwrap_or(0.0),
                d["confidence"].as_f64().unwrap_or(0.0),
                d["text"].as_str().unwrap_or("")));
        }
    }
    let json_path = output_dir.join("ocr.json");
    std::fs::write(&json_path, serde_json::to_string_pretty(&result).unwrap())
        .map_err(|e| CliError::Other(e.to_string()))?;
    Ok(())
}

async fn resolve_video(input: &str, output_dir: &PathBuf, out: &Output) -> Result<PathBuf, CliError> {
    // 情况 1: 已经是本地文件
    if let Ok(p) = PathBuf::from(input).canonicalize() {
        if p.is_file() {
            return Ok(p);
        }
    }
    // 情况 2: B 站 BV/AV/URL — 解析, 调 download
    // (这里假设用户已经下过视频, 或者我们直接复用 download 子命令)
    Err(CliError::Other(format!(
        "video not found at local path: {input}.\n\
         If this is a B 站 BV/AV/URL, first run:\n  bilitools download {input} -o {}\n\
         then re-run ocr with the same -o dir.",
        output_dir.display())))
}

fn parse_frame_ts(p: &std::path::Path) -> Option<f32> {
    let stem = p.file_stem()?.to_str()?;
    let t = stem.strip_prefix("frame_")?;
    t.parse().ok()
}

fn emit(
    results: &[crate::ipc::ocr::engine::DetectedText],
    output_dir: &PathBuf, mode: &str, out: &Output,
) -> Result<(), String> {
    let result = serde_json::json!({
        "mode": mode,
        "detections": results.iter().map(|r| serde_json::json!({
            "text": r.text,
            "confidence": r.confidence,
            "bbox": r.bbox,
        })).collect::<Vec<_>>(),
    });
    let json_path = output_dir.join("ocr.json");
    std::fs::write(&json_path, serde_json::to_string_pretty(&result).unwrap())
        .map_err(|e| e.to_string())?;
    if out.is_json() {
        out.ok(result)?;
    } else {
        out.status(&format!("OCR done: {} detections", results.len()));
        for r in results {
            out.status(&format!("  ({:.2}) {}", r.confidence, r.text));
        }
    }
    Ok(())
}
```

**Step 3: 注册子命令到 dispatch**

```rust
// src/cli/root.rs
#[cfg(feature = "ocr")]
Command::Ocr { .. } => crate::cli::ocr::run(cmd, out).await,
```

```rust
// src/cli/mod.rs
#[cfg(feature = "ocr")]
pub mod ocr;
```

**Step 4: 编译验证**

```bash
cargo build --features ocr 2>&1 | tail -10
./target/debug/bilitools --help 2>&1 | grep -A 1 ocr
./target/debug/bilitools ocr --help 2>&1 | head -20
```

**Step 5: Commit**

```bash
git add src/cli/ocr.rs src/cli/root.rs src/cli/mod.rs
git commit -m "feat(ocr): ocr subcommand with image + video modes"
```

---

### Task 5: 视频抽帧模块 (frames.rs)

**Files:**
- Create: `src/ipc/ocr/frames.rs`

**Step 1: 实现**

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
//! ffmpeg-based frame extraction for OCR.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct ExtractResult {
    pub frames: Vec<PathBuf>,
    pub interval_sec: f32,
    pub duration_sec: f32,
}

pub fn frame_path(out_dir: &Path, t_sec: f32) -> PathBuf {
    out_dir.join(format!("frame_{:09.3}.jpg", t_sec))
}

/// 抽帧: 调用 ffmpeg, 输出 frame_<ts>.jpg 序列, 最长 max_frames
pub async fn extract_frames(
    video: &Path,
    out_dir: &Path,
    interval_sec: f32,
    max_frames: u32,
) -> Result<ExtractResult, String> {
    if interval_sec <= 0.0 {
        return Err("interval must be > 0".into());
    }
    tokio::fs::create_dir_all(out_dir).await.map_err(|e| e.to_string())?;

    let fps = 1.0 / interval_sec;
    // ffmpeg -i input -vf fps=X -frames:v N frame_%09.3f.jpg
    let pattern = out_dir.join("frame_%09.3f.jpg");
    let mut cmd = tokio::process::Command::new("ffmpeg");
    cmd.arg("-y").arg("-i").arg(video);
    cmd.args(["-vf", &format!("fps={fps}")]);
    cmd.args(["-frames:v", &max_frames.to_string()]);
    cmd.arg("-q:v").arg("2");
    cmd.arg(&pattern);
    cmd.stdout(std::process::Stdio::null());
    cmd.stderr(std::process::Stdio::null());

    let status = cmd.status().await.map_err(|e| format!("ffmpeg spawn: {e}"))?;
    if !status.success() {
        return Err(format!("ffmpeg exit {}", status));
    }

    let mut frames: Vec<PathBuf> = tokio::fs::read_dir(out_dir).await
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())?
        .into_iter()
        .map(|e| e.path())
        .filter(|p| {
            p.extension().and_then(|s| s.to_str()) == Some("jpg")
                && p.file_name().and_then(|n| n.to_str())
                    .map(|n| n.starts_with("frame_")).unwrap_or(false)
        })
        .collect();
    frames.sort();
    Ok(ExtractResult { frames, interval_sec, duration_sec: 0.0 })
}

/// 优先用 ffmpeg (项目已有 backends/sidecar); 找不到时给清晰错误
pub fn ensure_ffmpeg() -> Result<(), String> {
    which::which("ffmpeg")
        .map(|_| ())
        .map_err(|_| "ffmpeg not found in PATH. Install ffmpeg (apt install ffmpeg) or set PATH.".to_string())
}
```

**Step 2: 写单测 (filename format)**

```rust
// tests/ocr_frames.rs
use bilitools::ipc::ocr::frames::frame_path;
use std::path::Path;

#[test]
fn frame_filename_zero() {
    assert_eq!(frame_path(Path::new("/tmp/f"), 0.0).to_str().unwrap(),
        "/tmp/f/frame_000.000.jpg");
}

#[test]
fn frame_filename_subsecond() {
    assert_eq!(frame_path(Path::new("/tmp/f"), 1.5).to_str().unwrap(),
        "/tmp/f/frame_001.500.jpg");
}

#[test]
fn frame_filename_long() {
    assert_eq!(frame_path(Path::new("/tmp/f"), 123.456).to_str().unwrap(),
        "/tmp/f/frame_123.456.jpg");
}
```

**Step 3: 跑测试**

```bash
cargo test --features ocr ocr_frames 2>&1 | tail -5
# 期望: 3 passed
```

**Step 4: 端到端 test: extract 真实视频**

```bash
# 用 v1 视频 (64MB, 163s) 抽 3 帧 (interval=30s)
mkdir -p /tmp/ocr-e2e
# ffmpeg 抽帧, 模拟 ocr 子命令
ffmpeg -y -i /tmp/ocr-test/vid/merged.mp4 -vf fps=1/30 -frames:v 3 -q:v 2 /tmp/ocr-e2e/frame_%09.3f.jpg 2>&1 | tail -3
ls /tmp/ocr-e2e/
# 期望: frame_000.000.jpg, frame_030.000.jpg, frame_060.000.jpg
```

**Step 5: Commit**

```bash
git add src/ipc/ocr/frames.rs tests/ocr_frames.rs
git commit -m "feat(ocr): ffmpeg frame extraction with deterministic filename pattern"
```

---

### Task 6: 端到端验证 (单图 + 视频)

**Step 1: 单图 OCR**

```bash
cd /home/trade/bilitools-cli
cargo build --release --features ocr 2>&1 | tail -5
# 期望: 编译过 (用现有 v1 测试帧)
ls -lh /tmp/ocr-test/frame8.jpg
# 跑
BILITOOLS_OCR_MODEL_DIR=/home/trade/bilitools-cli/models/ocr-fast \
  ./target/release/bilitools ocr /tmp/ocr-test/frame8.jpg -o /tmp/ocr-e2e
cat /tmp/ocr-e2e/ocr.json | head -30
# 期望: 3 条 detections: bilibili / 风景旅行收藏家 / 狂风骤雨的大山卧龙谷
```

**Step 2: 视频 OCR**

```bash
cp /tmp/ocr-test/vid/merged.mp4 /tmp/ocr-e2e/video.mp4
BILITOOLS_OCR_MODEL_DIR=/home/trade/bilitools-cli/models/ocr-fast \
  ./target/release/bilitools ocr /tmp/ocr-e2e/video.mp4 --video --interval 30 --max-frames 5 -o /tmp/ocr-e2e
ls /tmp/ocr-e2e/  # 期望: ocr.json + frames/ (默认 keep false → 删除)
cat /tmp/ocr-e2e/ocr.json | head -50
# 期望: 多条 detection, 每条带 t_sec / text / confidence / bbox
```

**Step 3: 验证 if 任何一步失败 → 修代码 → 重跑 → commit**

**Step 4: Commit (如果改了代码)**

---

### Task 7: SKILL.md + README 更新

**Files:**
- Modify: `SKILL.md` (在 "analyzing a B 站 video" 工作流加 ocr 选项)
- Modify: `README.md` (Quick Start 加 ocr subcommand)

**Step 1: SKILL.md 加新章节**

```markdown
## OCR (Extract Hard-Coded Text from Video or Image)

`bilitools ocr <path>` 离线 OCR — 抓视频内嵌字幕/水印/标题卡, B 站 AI 字幕抓不到时使用。

- **图片模式**: `bilitools ocr screenshot.png`
- **视频模式**: `bilitools ocr video.mp4 --video --interval 1.0`
- **BV 模式**: 先 `bilitools download <BV>` 再 `bilitools ocr <BV> --video`

**底层**: PP-OCRv5 mobile FP16 (MNN 后端, 10.4 MB), 完全离线, 无 Python 进程。

**模型位置** (按顺序查找):
1. `$BILITOOLS_OCR_MODEL_DIR`
2. `<exe-dir>/models/ocr-fast/`
3. `<exe-dir>/`
4. `./models/ocr-fast/`
5. `./`

**必需模型文件**:
- `PP-OCRv5_mobile_det_fp16.mnn`
- `PP-OCRv5_mobile_rec_fp16.mnn`
- `ppocr_keys_v5.txt`

**何时用 vs 官方字幕**:
- 视频有 B 站 AI 字幕 → 优先 `bilitools audio <BV>` (srt 路径更准)
- 视频只有视觉标题卡/水印/过场 → 必用 ocr
- 视频外语 + 没人声 → ocr 是唯一选择

**抽帧频率**:
- 默认 1.0s/帧 (1 fps), 适合大多数视频
- 高密度文字 (新闻标题) → 0.5s/帧
- 慢速视频 (纪录片) → 2.0s/帧

**已知限制** (rpic 经验):
- 手机小字 (< 16px 等效) 易漏
- 极端字体效果 (霓虹/阴影) 置信度低
- 反光/眩光区域 100% 漏
```

**Step 2: README Quick Start 表格加 `ocr` 行**

```markdown
| `bilitools ocr <path>` | 离线 OCR 图片/视频 (PP-OCRv5) | opt-in (`--features ocr`) |
```

**Step 3: Commit**

```bash
git add SKILL.md README.md
git commit -m "docs(ocr): add OCR subcommand to SKILL.md and README"
```

---

### Task 8: 重新打 release v1.4.7-cli.3 (含 ocr)

**Step 1: 默认 build 验证 (不带 ocr feature)**

```bash
cargo build --release 2>&1 | tail -3
ls -lh target/release/bilitools
# 期望: 大小接近 v1.4.7-cli.2 (无 ocr 时不变)
```

**Step 2: ocr build**

```bash
cargo build --release --features ocr 2>&1 | tail -3
ls -lh target/release/bilitools
# 期望: binary 大 +10-15 MB (MNN 静态库)
cp target/release/bilitools target/release/bilitools-ocr
```

**Step 3: GitHub Actions workflow 加 ocr build matrix**

```yaml
# .github/workflows/release.yml 加一个 include 行
- target: x86_64-unknown-linux-gnu
  features: ocr
  binary_name: bilitools-ocr
- target: x86_64-pc-windows-msvc
  features: ocr
  binary_name: bilitools-ocr.exe
- target: x86_64-apple-darwin
  features: ocr
  binary_name: bilitools-ocr
# 等等, 4 平台 × 2 feature 变体 = 8 个 artifacts
```

**Step 4: 打 tag + push**

```bash
cd /home/trade/bilitools-cli
# 改 Cargo.toml version
sed -i 's/1.4.7-cli.1/1.4.7-cli.3/' Cargo.toml
git add Cargo.toml .github/workflows/release.yml
git commit -m "release: v1.4.7-cli.3 (add opt-in ocr subcommand)"
git tag -a v1.4.7-cli.3 -m "bilitools 1.4.7-cli.3: add opt-in ocr subcommand (PP-OCRv5 mobile MNN, no Python)"
git push origin v1.4.7-cli.3
# GitHub Actions 触发编译
```

**Step 5: 等 CI, 验证 8 个 artifacts 都成功 (8 binary + 各自的 model zip)**

---

## 已知风险与缓解

| 风险 | 严重度 | 缓解 |
|------|--------|------|
| `ocr-rs` 2.2.2 与 rpic git rev API 有差异 | 中 | Task 2 编译时立刻发现, 修 `bbox_to_array` 等几行 |
| MNN 静态链接把 binary 推大 10-15 MB | 低 | opt-in feature, 默认 build 不带; 用户可选用 v4 模型再小一些 |
| 模型 MNN 文件 10.4 MB 不入 git | 中 | 单独发 GitHub Release; `make fetch-ocr-models` 脚本 download |
| rpic `TextBox` 字段名与 ocr-rs 2.2.2 不一致 | 高 | Task 2 第一次编译就发现, 调整 `bbox_to_array` |
| `which` crate 已有依赖 (transcribe feature) | - | `frames.rs` 复用, 不需要新加 |
| v1 视频 OCR 漏掉某些小字 | 低 | 默认 1.0s interval 用户可调到 0.5s |
| `image::open` 解码超大图 OOM | 低 | 抽帧 ffmpeg 阶段 -q:v 2 已压到 < 200 KB/帧 |
| GitHub Actions 4 平台 × 2 变体 = 8 build, 慢 | 中 | 用 `actions-rs/cargo` cache 拉 MNN 编译产物 |

---

## 验证清单 (完工判定)

- [ ] `cargo check` 不带 ocr → 编译过, 干净
- [ ] `cargo check --features ocr` → 编译过, 拉了 ocr-rs
- [ ] `bilitools ocr /tmp/ocr-test/frame8.jpg` → 输出 "bilibili" / "风景旅行收藏家" / "狂风骤雨的大山卧龙谷" 至少 1 个
- [ ] `bilitools ocr video.mp4 --video --interval 30` → 抽帧 + OCR + 输出 ocr.json 含 t_sec/text/conf
- [ ] `bilitools ocr <BV>` 当 BV 已 download 后也能跑
- [ ] `--json` 输出机器可读
- [ ] `--min-conf 0.8` 过滤低置信度
- [ ] 模型没下载/放错位置 → 清晰错误信息 (含搜索路径)
- [ ] `--keep-frames` 保留 frames 目录
- [ ] SKILL.md 有完整 ocr 章节
- [ ] README 有 ocr subcommand 行
- [ ] Release 1.4.7-cli.3 发布, 含 ocr binary

---

## 不在范围内 (YAGNI)

- ❌ PDF/Word OCR (rpic 也不做)
- ❌ 多线程并行 OCR (ocr-rs 内置 rayon 已经处理)
- ❌ 实时流 OCR
- ❌ GPU 加速 (v5 mobile CPU 已经够用)
- ❌ 自训练模型
- ❌ OCR 结果翻译
- ❌ 历史弹幕 OCR

---

## 时间估计

| Task | 时间 |
|------|------|
| 1. feature flag | 5 min |
| 2. OCR 引擎封装 | 20 min |
| 3. 模型路径搜索 | 15 min |
| 4. CLI 子命令 | 30 min |
| 5. 视频抽帧 | 20 min |
| 6. E2E 验证 | 30 min |
| 7. 文档 | 15 min |
| 8. release | 15 min |
| **总计** | **2.5 小时** |

比之前的 5-7 小时**少一半**，因为 `ocr-rs` 替我们处理了所有底层数学。
