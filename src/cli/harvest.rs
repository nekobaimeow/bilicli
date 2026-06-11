// SPDX-License-Identifier: GPL-3.0-or-later
// `harvest` subcommand — batch-fetches danmaku + reviews + subtitles
// for the top-N search results of a keyword.
//
// 工作流（与 GUI 原版一致）：
//   1. search_videos(keyword) 拿 top N
//   2. 对每条 video 串行调 danmaku::fetch_and_convert + review::fetch_main
//      + subtitle::fetch_all
//   3. 每个视频一个子目录，文件名稳定
//
// 降级策略：
//   - 单个视频失败（danmaku/review/subtitle 任一）→ 继续下一个，
//     不中断整体流程；degraded 列表带上原因
//   - 整个 batch 全部失败 → exit 1
//   - 至少一个成功 → exit 0 + 警告

use crate::cli::output::Output;
use crate::cli::root::Command;
use crate::error::CliError;
use crate::ipc::danmaku::{self, DanmakuFormat, DanmakuSource};
use crate::ipc::review::{self, ReviewSort};
use crate::ipc::search::{self, VideoResult};
use crate::ipc::subtitle;
use serde::Serialize;
use std::path::{Path, PathBuf};
use tokio::fs;
use tokio::io::AsyncWriteExt;

/// 单个视频的 harvest 结果
#[derive(Debug, Clone, Serialize)]
pub struct HarvestEntry {
    pub bvid: String,
    pub aid: i64,
    pub cid: i64,
    pub title: String,
    pub dir: PathBuf,
    pub danmaku: Option<DanmakuSummary>,
    pub review: Option<ReviewSummary>,
    pub subtitle: Vec<SubtitleSummary>,
    pub degraded: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DanmakuSummary {
    pub live_count: i64,
    pub xml_path: Option<PathBuf>,
    pub ass_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReviewSummary {
    pub total: i64,
    pub loaded: usize,
    pub json_path: PathBuf,
}

#[derive(Debug, Clone, Serialize)]
pub struct SubtitleSummary {
    pub lan: String,
    pub lan_doc: String,
    pub path: PathBuf,
    pub body_len: usize,
}

/// 整体 harvest 结果
#[derive(Debug, Clone, Serialize)]
pub struct HarvestResult {
    pub keyword: String,
    pub output_dir: PathBuf,
    pub entries: Vec<HarvestEntry>,
    pub degraded: Vec<String>,
}

/// 把标题变成安全目录名（slug）。
///
/// - 去掉 <em> 标签、特殊字符
/// - 截断到 80 字符
/// - 全 ASCII 控制字符和路径分隔符替换为 `_`
pub fn slugify_title(s: &str) -> String {
    let s = s.replace("<em class=\"keyword\">", "");
    let s = s.replace("</em>", "");
    let s = s.trim().to_string();
    let mut out = String::with_capacity(s.len().min(80));
    for c in s.chars() {
        if c.is_control() || c == '/' || c == '\\' || c == ':' {
            out.push('_');
        } else if c == ' ' || c == '\t' {
            out.push('_');
        } else {
            out.push(c);
        }
        if out.chars().count() >= 80 {
            break;
        }
    }
    if out.is_empty() {
        "untitled".to_string()
    } else {
        out
    }
}

/// 抓单个视频（串起 search -> danmaku + review + subtitle）
pub async fn harvest_one_video(
    bv: &str,
    title: &str,
    output_dir: &Path,
    opts: &HarvestOptions,
) -> HarvestEntry {
    let mut entry = HarvestEntry {
        bvid: bv.to_string(),
        aid: 0,
        cid: 0,
        title: title.to_string(),
        dir: output_dir.to_path_buf(),
        danmaku: None,
        review: None,
        subtitle: Vec::new(),
        degraded: Vec::new(),
    };

    // 1. 拿 aid + cid（用于后面 subtitle 命名 + review oid）
    match danmaku::resolve_cid(bv).await {
        Ok((t, aid, cid)) => {
            entry.title = t;
            entry.aid = aid;
            entry.cid = cid;
        }
        Err(e) => {
            entry.degraded.push(format!("resolve_cid failed: {e}"));
            return entry;
        }
    }

    // 2. 子目录：{output_dir}/{slug-title}/
    let slug = slugify_title(&entry.title);
    let sub_dir = output_dir.join(&slug);
    if let Err(e) = fs::create_dir_all(&sub_dir).await {
        entry
            .degraded
            .push(format!("create_dir_all {} failed: {e}", sub_dir.display()));
        return entry;
    }
    entry.dir = sub_dir.clone();

    // 3. 弹幕
    if opts.with_danmaku {
        match danmaku::fetch_and_convert(bv, &sub_dir, DanmakuSource::Live, DanmakuFormat::Both)
            .await
        {
            Ok(d) => {
                entry.danmaku = Some(DanmakuSummary {
                    live_count: d.live_count as i64,
                    xml_path: d.xml_path,
                    ass_path: d.ass_path,
                });
            }
            Err(e) => entry.degraded.push(format!("danmaku: {e}")),
        }
    }

    // 4. 评论
    if opts.with_review {
        // 匿名自动降级 ps
        let ps = if danmaku::anonymous_mode().await {
            3u32.min(opts.review_ps)
        } else {
            opts.review_ps
        };
        match review::fetch_main(bv, ReviewSort::Hot, 1, ps).await {
            Ok(r) => {
                let json_path = sub_dir.join("review.json");
                let summary = ReviewSummary {
                    total: r.total,
                    loaded: r.replies.len(),
                    json_path: json_path.clone(),
                };
                if let Ok(json_bytes) = serde_json::to_vec_pretty(&r) {
                    if let Ok(mut f) = fs::File::create(&json_path).await {
                        let _ = f.write_all(&json_bytes).await;
                    }
                }
                entry.review = Some(summary);
                for d in r.degraded {
                    entry.degraded.push(format!("review: {d}"));
                }
            }
            Err(e) => entry.degraded.push(format!("review: {e}")),
        }
    }

    // 5. 字幕
    if opts.with_subtitle {
        match subtitle::fetch_all(bv, &sub_dir).await {
            Ok(s) => {
                for f in s.fetched {
                    entry.subtitle.push(SubtitleSummary {
                        lan: f.entry.lan,
                        lan_doc: f.entry.lan_doc,
                        path: f.path,
                        body_len: f.body_len,
                    });
                }
                for d in s.degraded {
                    entry.degraded.push(format!("subtitle: {d}"));
                }
            }
            Err(e) => entry.degraded.push(format!("subtitle: {e}")),
        }
    }

    // 6. meta.json
    let meta = serde_json::json!({
        "bv": entry.bvid,
        "aid": entry.aid,
        "cid": entry.cid,
        "title": entry.title,
        "slug": slug,
        "harvested_at": chrono::Utc::now().to_rfc3339(),
    });
    if let Ok(b) = serde_json::to_vec_pretty(&meta) {
        let meta_path = sub_dir.join("meta.json");
        if let Ok(mut f) = fs::File::create(&meta_path).await {
            let _ = f.write_all(&b).await;
        }
    }

    entry
}

/// 选项
#[derive(Debug, Clone)]
pub struct HarvestOptions {
    pub with_danmaku: bool,
    pub with_review: bool,
    pub with_subtitle: bool,
    pub review_ps: u32,
}

impl Default for HarvestOptions {
    fn default() -> Self {
        HarvestOptions {
            with_danmaku: true,
            with_review: true,
            with_subtitle: true,
            review_ps: 20,
        }
    }
}

pub async fn run(cmd: &Command, out: &Output) -> Result<(), CliError> {
    let Command::Harvest {
        keyword,
        limit,
        output_dir,
        no_danmaku,
        no_review,
        no_subtitle,
        review_ps,
    } = cmd
    else {
        return Err(CliError::Other("internal: not a Harvest command".into()));
    };
    let output_dir: PathBuf = output_dir.clone();
    fs::create_dir_all(&output_dir)
        .await
        .map_err(CliError::from)?;

    let opts = HarvestOptions {
        with_danmaku: !*no_danmaku,
        with_review: !*no_review,
        with_subtitle: !*no_subtitle,
        review_ps: *review_ps,
    };

    // 1. search
    let search = search::search_videos(keyword, 1, *limit)
        .await?;
    let total_collected = search.results.len();
    if total_collected == 0 {
        if out.is_json() {
            out.ok(serde_json::json!({
                "keyword": keyword,
                "output_dir": output_dir,
                "entries": [],
                "degraded": ["no search results"],
            }))?;
        } else {
            out.status("[info] no search results");
        }
        return Ok(());
    }

    out.status(&format!(
        "[harvest] keyword={}  found={}  output={}  opts=d:{} r:{} s:{}",
        keyword,
        total_collected,
        output_dir.display(),
        opts.with_danmaku,
        opts.with_review,
        opts.with_subtitle,
    ));

    // 2. 串行抓取
    let mut entries = Vec::new();
    let mut overall_degraded = Vec::new();
    for (i, r) in search.results.iter().enumerate() {
        let bv = match r.bvid.as_ref() {
            Some(b) => b,
            None => {
                // 跳过 cheese 等非视频类型
                out.status(&format!(
                    "[{}/{}] (skip non-video) kind={} ssid={:?}",
                    i + 1,
                    total_collected,
                    r.kind,
                    r.ssid
                ));
                continue;
            }
        };
        out.status(&format!(
            "[{}/{}] {bv}  {}",
            i + 1,
            total_collected,
            truncate(&r.title, 60)
        ));
        let entry = harvest_one_video(bv, &r.title, &output_dir, &opts).await;
        let summary = format!(
            "[{}/{}] {bv} → danmaku:{} review:{} subtitle:{} degraded:{}",
            i + 1,
            total_collected,
            entry.danmaku.as_ref().map(|d| d.live_count).unwrap_or(-1),
            entry
                .review
                .as_ref()
                .map(|r| format!("{} (total={})", r.loaded, r.total))
                .unwrap_or_else(|| "skip".into()),
            entry.subtitle.len(),
            entry.degraded.len(),
        );
        out.status(&summary);
        for d in &entry.degraded {
            overall_degraded.push(format!("{bv}: {d}"));
        }
        entries.push(entry);
    }

    // 3. 汇总
    let result = HarvestResult {
        keyword: keyword.clone(),
        output_dir,
        entries,
        degraded: overall_degraded,
    };

    if out.is_json() {
        out.ok(serde_json::to_value(&result).unwrap_or(serde_json::json!({})))?;
    } else {
        out.status(&format!(
            "[done] {} videos processed  degraded: {}",
            result.entries.len(),
            result.degraded.len()
        ));
    }

    // 至少一个成功
    if result.entries.is_empty() {
        return Err(CliError::Other("harvest: 0 videos processed".into()));
    }
    Ok(())
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_chinese() {
        // <em> 标签 + 中文 + 特殊符号
        let s = "<em class=\"keyword\">原神</em> PC版 12分钟 实机演示";
        let slug = slugify_title(s);
        assert!(slug.contains("原神"));
        assert!(slug.contains("PC版"));
        assert!(!slug.contains("<em"));
        assert!(!slug.contains(' '));
    }

    #[test]
    fn slugify_short_input() {
        assert_eq!(slugify_title("hi"), "hi");
    }

    #[test]
    fn slugify_truncates() {
        let s = "a".repeat(200);
        let slug = slugify_title(&s);
        assert!(slug.chars().count() <= 80);
    }

    #[test]
    fn slugify_empty_becomes_untitled() {
        assert_eq!(slugify_title(""), "untitled");
        assert_eq!(slugify_title("   "), "untitled");
    }

    #[test]
    fn slugify_strips_path_separators() {
        let s = "title/with\\slashes:and:colons";
        let slug = slugify_title(s);
        assert!(!slug.contains('/'));
        assert!(!slug.contains('\\'));
        assert!(!slug.contains(':'));
    }

    #[test]
    fn slugify_removes_em_tags() {
        // B 站搜索结果里 <em class="keyword"> 是真实形态
        assert_eq!(slugify_title("<em class=\"keyword\">foo</em>bar"), "foobar");
    }
}
