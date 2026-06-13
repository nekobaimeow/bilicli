// SPDX-License-Identifier: GPL-3.0-or-later
// `hot` subcommand — fetch B站 popular/trending videos.

use crate::cli::output::Output;
use crate::cli::root::Command;
use crate::cli::search::{format_count, format_duration, render_id_column, truncate};
use crate::error::CliError;
use crate::ipc::search::VideoResult;
use crate::ipc::shared;
use serde::Deserialize;
use std::collections::BTreeMap;

/// B站热门 API 返回的单条视频
#[derive(Debug, Deserialize)]
struct HotVideo {
    aid: Option<i64>,
    bvid: Option<String>,
    title: String,
    #[serde(default)]
    desc: Option<String>,
    #[serde(default)]
    pic: Option<String>,
    #[serde(default)]
    pubdate: Option<i64>,
    #[serde(default)]
    duration: Option<i64>,
    #[serde(default)]
    owner: Option<Owner>,
    #[serde(default)]
    stat: Option<Stat>,
    #[serde(default)]
    tname: Option<String>,
    #[serde(default)]
    rcmd_reason: Option<RcmdReason>,
    #[serde(default)]
    short_link_v2: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Owner {
    mid: i64,
    name: String,
}

#[derive(Debug, Deserialize)]
struct Stat {
    #[serde(default)]
    view: Option<i64>,
    #[serde(default)]
    danmaku: Option<i64>,
    #[serde(default)]
    reply: Option<i64>,
    #[serde(default)]
    favorite: Option<i64>,
    #[serde(default)]
    coin: Option<i64>,
    #[serde(default)]
    share: Option<i64>,
    #[serde(default)]
    like: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct RcmdReason {
    content: String,
}

#[derive(Debug, Deserialize)]
struct HotData {
    #[serde(default)]
    list: Vec<HotVideo>,
    #[serde(default)]
    no_more: bool,
}

#[derive(Debug, Deserialize)]
struct HotApiResponse {
    code: i64,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    data: Option<HotData>,
}

fn hot_to_video_result(h: &HotVideo) -> VideoResult {
    let dur_sec = h.duration.unwrap_or(0);
    VideoResult {
        kind: "video".to_string(),
        bvid: h.bvid.clone(),
        ssid: None,
        title: h.title.clone(),
        author: h.owner.as_ref().map(|o| o.name.clone()).unwrap_or_default(),
        mid: h.owner.as_ref().map(|o| o.mid).unwrap_or(0),
        duration: format_duration(dur_sec),
        duration_sec: dur_sec,
        play: h.stat.as_ref().and_then(|s| s.view).unwrap_or(0),
        pubdate: h.pubdate.unwrap_or(0),
        description: h.desc.clone().unwrap_or_default(),
        pic: h.pic.clone().unwrap_or_default(),
        typename: h.tname.clone().unwrap_or_default(),
        tid: None,
        arcurl: h
            .short_link_v2
            .clone()
            .unwrap_or_else(|| format!("https://www.bilibili.com/video/{}", h.bvid.as_deref().unwrap_or(""))),
    }
}

pub async fn run(cmd: &Command, out: &Output) -> Result<(), CliError> {
    let Command::Hot { page, page_size } = cmd else {
        return Err(CliError::Other("internal: not a Hot command".into()));
    };
    let page = *page;
    let page_size = *page_size;

    let url_base = "https://api.bilibili.com/x/web-interface/popular";
    let mut params: BTreeMap<String, String> = BTreeMap::new();
    params.insert("pn".into(), page.to_string());
    params.insert("ps".into(), page_size.to_string());

    // WBI sign — fallback to unsigned on failure
    let url = match shared::wbi_sign(&params).await {
        Ok((q, w_rid)) => format!("{url_base}?{q}&w_rid={w_rid}"),
        Err(_) => {
            let fallback: String = params
                .iter()
                .map(|(k, v)| format!("{}={}", encode(k), encode(v)))
                .collect::<Vec<_>>()
                .join("&");
            format!("{url_base}?{fallback}")
        }
    };

    let client = shared::init_client()
        .await
        .map_err(|e| CliError::Other(e.to_string()))?;

    let resp = client
        .get(&url)
        .header(reqwest::header::ACCEPT_ENCODING, "identity")
        .send()
        .await?;

    if !resp.status().is_success() {
        return Err(CliError::Http {
            status: resp.status().as_u16(),
            message: "popular api failed".into(),
        });
    }

    let body_bytes = resp.bytes().await.map_err(CliError::from)?;
    let raw: HotApiResponse = serde_json::from_slice(&body_bytes).map_err(|e| {
        CliError::Parse(format!(
            "json decode failed: {e}; body[0..200]={:?}",
            String::from_utf8_lossy(&body_bytes[..body_bytes.len().min(200)])
        ))
    })?;

    if raw.code != 0 {
        return Err(CliError::Api {
            code: raw.code,
            message: raw.message.unwrap_or_default(),
        });
    }

    let data = raw.data.unwrap_or(HotData {
        list: vec![],
        no_more: true,
    });

    let results: Vec<VideoResult> = data.list.iter().map(hot_to_video_result).collect();

    if out.is_json() {
        out.ok(serde_json::json!({
            "page": page,
            "page_size": page_size,
            "no_more": data.no_more,
            "count": results.len(),
            "results": results,
        }))?;
    } else {
        if results.is_empty() {
            out.status("(no hot videos)");
            return Ok(());
        }
        out.status(&format!(
            "{:<14} {:<7} {:<8} {:<14} {}",
            "BVID", "DUR", "PLAY", "AUTHOR", "TITLE"
        ));
        for r in &results {
            let id = render_id_column(r);
            out.status(&format!(
                "{:<14} {:<7} {:<8} {:<14} {}",
                id,
                format_duration(r.duration_sec),
                format_count(r.play),
                truncate(&r.author, 13),
                truncate(&r.title, 60),
            ));
        }
        if data.no_more {
            out.status(&format!("(page {} — end)", page));
        } else {
            out.status(&format!(
                "(page {} of {} results — use --page {} for more)",
                page,
                results.len(),
                page + 1
            ));
        }
    }

    Ok(())
}

fn encode(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}
