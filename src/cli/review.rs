// SPDX-License-Identifier: GPL-3.0-or-later
// `review` subcommand — fetch B 站 video comments.

use crate::cli::output::Output;
use crate::cli::root::Command;
use crate::error::CliError;
use crate::ipc::review::{self, Reply, ReviewSort};
use std::path::PathBuf;

pub async fn run(cmd: &Command, out: &Output) -> Result<(), CliError> {
    let Command::Review {
        input,
        sort,
        page,
        ps,
        sub,
        no_login_warn,
    } = cmd
    else {
        return Err(CliError::Other("internal: not a Review command".into()));
    };
    let sort: ReviewSort = sort.parse().map_err(|e: CliError| e)?;
    let page = *page;
    let ps = *ps;

    if !*no_login_warn
        && !crate::ipc::shared::HEADERS.cookie().await.contains("SESSDATA=")
    {
        out.status(
            "[warn] not logged in; only first-page hot reviews will be visible. run `bilicli auth qrcode` first.",
        );
    }

    if let Some(root_rpid) = sub {
        // 子评模式：拉 root rpid 下的所有 sub
        let root_rpid = root_rpid.parse::<i64>().map_err(|e| {
            CliError::Parse(format!("invalid --sub rpid '{root_rpid}': {e}"))
        })?;
        let result = review::fetch_sub(input, root_rpid, page, ps).await?;
        if out.is_json() {
            out.ok(serde_json::json!({
                "bv": input,
                "root_rpid": root_rpid,
                "sub_replies_count": result.sub_replies_count,
                "sub_replies": result.sub_replies,
            }))?;
        } else {
            out.status(&format!("bv:           {}", input));
            out.status(&format!("root rpid:    {}", root_rpid));
            out.status(&format!("sub replies:  {} (loaded {})", result.sub_replies_count, result.sub_replies.len()));
            print_reply_table(&result.sub_replies, &out);
        }
    } else {
        // 主评模式
        let result = review::fetch_main(input, sort, page, ps).await?;
        if out.is_json() {
            out.ok(serde_json::json!({
                "bv": result.bv,
                "aid": result.aid,
                "title": result.title,
                "sort": result.sort,
                "page": result.page,
                "page_size": result.page_size,
                "total": result.total,
                "replies": result.replies,
                "degraded": result.degraded,
            }))?;
        } else {
            out.status(&format!("title:  {}", result.title));
            out.status(&format!("bv:     {}", result.bv));
            out.status(&format!("aid:    {}", result.aid));
            out.status(&format!("total:  {} (loaded {} on page {})", result.total, result.replies.len(), result.page));
            for d in &result.degraded {
                out.status(&format!("[degraded] {d}"));
            }
            print_reply_table(&result.replies, &out);
        }
    }
    Ok(())
}

fn print_reply_table(replies: &[Reply], out: &Output) {
    if replies.is_empty() {
        out.status("(no comments loaded)");
        return;
    }
    out.status(&format!(
        "{:<12} {:<16} {:<5} {:<11} {}",
        "RPID", "UNAME", "LIKE", "CTIME", "MESSAGE"
    ));
    for r in replies {
        let ctime = format_ctime(r.ctime);
        let marker = if r.is_top { "📌" } else { "  " };
        out.status(&format!(
            "{}{:<12} {:<16} {:<5} {:<11} {}",
            marker,
            r.rpid,
            truncate(&r.uname, 15),
            r.like,
            ctime,
            truncate(&r.message, 60)
        ));
        // 缩进子评
        for s in &r.sub_replies {
            out.status(&format!(
                "  ↳ {:<12} {:<16} {:<5} {:<11} {}",
                s.rpid,
                truncate(&s.uname, 15),
                s.like,
                format_ctime(s.ctime),
                truncate(&s.message, 60)
            ));
        }
    }
}

fn format_ctime(sec: i64) -> String {
    if sec <= 0 {
        return "-".into();
    }
    // YYYYMMDD 简化格式（不要时区，时间戳当作 UTC+8）
    use chrono::{DateTime, Utc};
    DateTime::<Utc>::from_timestamp(sec, 0)
        .map(|dt| {
            let cst = dt + chrono::Duration::hours(8);
            cst.format("%Y%m%d").to_string()
        })
        .unwrap_or_else(|| sec.to_string())
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
    fn format_ctime_handles_zero() {
        assert_eq!(format_ctime(0), "-");
    }

    #[test]
    fn truncate_ascii_short() {
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn truncate_chinese_under_limit() {
        assert_eq!(truncate("原神演示", 5), "原神演示");
    }

    #[test]
    fn truncate_chinese_over_limit() {
        assert_eq!(truncate("原神演示", 3), "原神…");
    }

    // chrono 重新出现 — 验证它已经在 Cargo.toml 里。
    // (如果不在，build 会报 dep 错。)
    #[test]
    fn chrono_available() {
        use chrono::Utc;
        let now = Utc::now().timestamp();
        assert!(now > 0);
    }
}
