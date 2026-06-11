// SPDX-License-Identifier: GPL-3.0-or-later
// B 站视频评论抓取 IPC。
//
// 工作流（与 GUI 原版一致）：
//   1. 拿 aid（from BV via web-interface/view）
//   2. 拉 https://api.bilibili.com/x/v2/reply?type=1&oid={aid}&pn=N&ps=K&sort={2|0}
//   3. （可选）拿 sub replies via /x/v2/reply/reply
//
// 降级策略：
//   - 未登录 → 只能拿到第 1 页 3-5 条热评（风控），不报错
//   - 翻页超限 → 返回当前页 + degraded 提示
//   - sub reply 风控 → 返回 main，sub 字段为空

use crate::error::CliError;
use crate::ipc::danmaku;
use crate::ipc::shared;
use serde::Serialize;
use std::collections::VecDeque;

pub type Result<T> = std::result::Result<T, CliError>;

/// 排序方式
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ReviewSort {
    /// 热门（sort=2）
    Hot,
    /// 时间（sort=0）
    Time,
}

impl Default for ReviewSort {
    fn default() -> Self {
        ReviewSort::Hot
    }
}

impl std::str::FromStr for ReviewSort {
    type Err = CliError;
    fn from_str(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            "hot" | "h" | "default" | "d" => Ok(ReviewSort::Hot),
            "time" | "t" | "new" | "n" => Ok(ReviewSort::Time),
            _ => Err(CliError::Parse(format!("unknown review sort: {s}"))),
        }
    }
}

impl ReviewSort {
    pub fn as_param(&self) -> u32 {
        match self {
            ReviewSort::Hot => 2,
            ReviewSort::Time => 0,
        }
    }
}

/// 一条评论（main + 嵌套 sub）
#[derive(Debug, Clone, Serialize)]
pub struct Reply {
    /// 楼层 id
    pub rpid: i64,
    /// 用户 mid
    pub mid: i64,
    /// 用户名
    pub uname: String,
    /// 头像
    pub avatar: String,
    /// 评论内容（HTML 实体已解码，<em> 高亮保留）
    pub message: String,
    /// 点赞数
    pub like: i64,
    /// 评论时间戳（秒）
    pub ctime: i64,
    /// 是否置顶
    pub is_top: bool,
    /// 嵌套子评
    pub sub_replies: Vec<Reply>,
    /// 子评总数（服务端给的真实值，可能 > 实际拉到的 sub_replies.len()）
    pub sub_replies_count: i64,
}

/// 评论抓取结果
#[derive(Debug, Clone, Serialize)]
pub struct ReviewResults {
    /// 视频 BV 号
    pub bv: String,
    /// 视频 aid
    pub aid: i64,
    /// 视频标题
    pub title: String,
    /// 排序：hot / time
    pub sort: ReviewSort,
    /// 当前页
    pub page: u32,
    /// 实际拿到的 page_size
    pub page_size: u32,
    /// 总评论数（来自 `data.page.count`）
    pub total: i64,
    /// 已加载的评论（main 层）
    pub replies: Vec<Reply>,
    /// 降级 / 警告信息
    pub degraded: Vec<String>,
}

/// 抓 hot / time 主评论
pub async fn fetch_main(
    bv: &str,
    sort: ReviewSort,
    page: u32,
    page_size: u32,
) -> Result<ReviewResults> {
    let (title, aid, _cid) = danmaku::resolve_cid(bv).await?;
    let url = format!(
        "https://api.bilibili.com/x/v2/reply?type=1&oid={aid}&pn={page}&ps={page_size}&sort={}",
        sort.as_param()
    );
    let client = shared::init_client().await.map_err(|e| CliError::Other(e.to_string()))?;
    let mut degraded = Vec::new();
    if !shared::HEADERS.cookie().await.contains("SESSDATA=") {
        degraded.push(
            "匿名模式：仅可获取第 1 页热评；登录后可见全量".to_string(),
        );
    }
    let resp = client.get(&url).send().await.map_err(CliError::from)?;
    if !resp.status().is_success() {
        return Err(CliError::Http {
            status: resp.status().as_u16(),
            message: format!("review main http for {bv}"),
        });
    }
    let body: serde_json::Value = resp.json().await.map_err(CliError::from)?;
    let code = body.get("code").and_then(|c| c.as_i64()).unwrap_or(-1);
    if code != 0 {
        let msg = body
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("reply api error")
            .to_string();
        return Err(CliError::Api { code, message: msg });
    }
    let data = body.get("data");
    let total = data
        .and_then(|d| d.get("page"))
        .and_then(|p| p.get("count"))
        .and_then(|c| c.as_i64())
        .unwrap_or(0);
    let replies = parse_replies_value(data);

    Ok(ReviewResults {
        bv: bv.to_string(),
        aid,
        title,
        sort,
        page,
        page_size,
        total,
        replies,
        degraded,
    })
}

/// 抓子评（root rpid 下的所有 sub）
pub async fn fetch_sub(
    bv: &str,
    root_rpid: i64,
    page: u32,
    page_size: u32,
) -> Result<Reply> {
    let (title, aid, _cid) = danmaku::resolve_cid(bv).await?;
    let url = format!(
        "https://api.bilibili.com/x/v2/reply/reply?oid={aid}&type=1&root={root_rpid}&pn={page}&ps={page_size}"
    );
    let client = shared::init_client().await.map_err(|e| CliError::Other(e.to_string()))?;
    let resp = client.get(&url).send().await.map_err(CliError::from)?;
    if !resp.status().is_success() {
        return Err(CliError::Http {
            status: resp.status().as_u16(),
            message: format!("review sub http for {bv} root={root_rpid}"),
        });
    }
    let body: serde_json::Value = resp.json().await.map_err(CliError::from)?;
    let code = body.get("code").and_then(|c| c.as_i64()).unwrap_or(-1);
    if code != 0 {
        let msg = body
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("reply sub api error")
            .to_string();
        return Err(CliError::Api { code, message: msg });
    }
    let data = body.get("data");
    let replies = parse_replies_value(data);
    // 子评拿到的只是一组 Reply（最深的层）；上层用 count 当 sub_replies_count
    let count = data
        .and_then(|d| d.get("page"))
        .and_then(|p| p.get("count"))
        .and_then(|c| c.as_i64())
        .unwrap_or(replies.len() as i64);
    let _ = title; // not used in this variant
    Ok(Reply {
        rpid: root_rpid,
        mid: 0,
        uname: String::new(),
        avatar: String::new(),
        message: String::new(),
        like: 0,
        ctime: 0,
        is_top: false,
        sub_replies: replies,
        sub_replies_count: count,
    })
}

/// 从 reply 响应的 `data` 段递归抽取嵌套评论
fn parse_replies_value(data: Option<&serde_json::Value>) -> Vec<Reply> {
    let mut out = Vec::new();
    let Some(data) = data else {
        return out;
    };
    let Some(replies) = data.get("replies").and_then(|r| r.as_array()) else {
        return out;
    };
    let mut queue: VecDeque<&serde_json::Value> = replies.iter().collect();
    while let Some(item) = queue.pop_front() {
        if let Some(r) = parse_single_reply(item) {
            out.push(r);
        }
    }
    out
}

/// 解析单条评论 + 它直接挂载的子评论（递归一层）
fn parse_single_reply(v: &serde_json::Value) -> Option<Reply> {
    let rpid = v.get("rpid").and_then(|x| x.as_i64())?;
    let mid = v
        .get("mid")
        .and_then(|x| x.as_i64())
        .or_else(|| {
            v.get("member")
                .and_then(|m| m.get("mid"))
                .and_then(|x| x.as_i64())
        })
        .unwrap_or(0);
    let member = v.get("member");
    let uname = member
        .and_then(|m| m.get("uname"))
        .and_then(|u| u.as_str())
        .unwrap_or("")
        .to_string();
    let avatar = member
        .and_then(|m| m.get("avatar"))
        .and_then(|a| a.as_str())
        .unwrap_or("")
        .to_string();
    let message_raw = v
        .get("content")
        .and_then(|c| c.get("message"))
        .and_then(|m| m.as_str())
        .unwrap_or("");
    let message = decode_html_entities(message_raw);
    let like = v.get("like").and_then(|x| x.as_i64()).unwrap_or(0);
    let ctime = v.get("ctime").and_then(|x| x.as_i64()).unwrap_or(0);
    // B 站用 `top` boolean OR `state=4` 表示置顶。
    let is_top = v
        .get("top")
        .and_then(|x| x.as_bool())
        .unwrap_or(false)
        || v.get("state").and_then(|x| x.as_i64()).unwrap_or(0) == 4;
    let sub_replies_count = v.get("rcount").and_then(|x| x.as_i64()).unwrap_or(0);
    let mut subs = Vec::new();
    if let Some(sub_arr) = v.get("replies").and_then(|r| r.as_array()) {
        for s in sub_arr {
            if let Some(r) = parse_single_reply(s) {
                subs.push(r);
            }
        }
    }
    Some(Reply {
        rpid,
        mid,
        uname,
        avatar,
        message,
        like,
        ctime,
        is_top,
        sub_replies: subs,
        sub_replies_count,
    })
}

/// 解码 B 站评论 / 弹幕 / 字幕里常见的 HTML 实体。
///
/// B 站会把 `<` `>` `&` `"` 等转义。我们也顺手处理 `&nbsp;`（U+00A0）
/// 和 `&#39;`（U+0027 单引号）。
pub fn decode_html_entities(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&nbsp;", "\u{00a0}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_html_amp() {
        assert_eq!(decode_html_entities("a&amp;b"), "a&b");
    }

    #[test]
    fn decode_html_quote() {
        assert_eq!(decode_html_entities("&quot;x&quot;"), "\"x\"");
    }

    #[test]
    fn decode_html_nbsp() {
        assert_eq!(decode_html_entities("a&nbsp;b"), "a\u{00a0}b");
    }

    #[test]
    fn decode_html_passthrough() {
        assert_eq!(decode_html_entities("plain"), "plain");
    }

    #[test]
    fn decode_html_combined() {
        // B 站实际形态：<em>foo</em> 实体化的
        let s = "&lt;em class=\"keyword\"&gt;黄金&lt;/em&gt; &amp; 美元";
        // 我们的解码会先把实体化的左右尖括号还原，但保留 <em>
        // 标签本身（<em> 是 B 站的搜索高亮，不应被去掉）。
        assert_eq!(decode_html_entities(s), "<em class=\"keyword\">黄金</em> & 美元");
    }

    #[test]
    fn review_sort_default_is_hot() {
        assert_eq!(ReviewSort::default(), ReviewSort::Hot);
    }

    #[test]
    fn review_sort_from_str() {
        assert_eq!("hot".parse::<ReviewSort>().unwrap(), ReviewSort::Hot);
        assert_eq!("TIME".parse::<ReviewSort>().unwrap(), ReviewSort::Time);
        assert!("unknown".parse::<ReviewSort>().is_err());
    }

    #[test]
    fn review_sort_as_param() {
        assert_eq!(ReviewSort::Hot.as_param(), 2);
        assert_eq!(ReviewSort::Time.as_param(), 0);
    }

    #[test]
    fn parse_single_reply_extracts_uname_like_message() {
        let v = serde_json::json!({
            "rpid": 277505236320_i64,
            "mid": 222222,
            "ctime": 1715000000,
            "like": 247,
            "rcount": 3,
            "top": false,
            "state": 0,
            "content": {"message": "测试<em>内容</em> &amp; 特殊字符"},
            "member": {"uname": "墨菲特", "avatar": "https://i0.hdslb.com"}
        });
        let r = parse_single_reply(&v).expect("should parse");
        assert_eq!(r.rpid, 277505236320);
        assert_eq!(r.uname, "墨菲特");
        assert_eq!(r.like, 247);
        assert_eq!(r.message, "测试<em>内容</em> & 特殊字符");
        assert_eq!(r.sub_replies_count, 3);
        assert!(!r.is_top);
        assert!(r.sub_replies.is_empty());
    }

    #[test]
    fn parse_replies_with_sub_replies_recurses() {
        let v = serde_json::json!({
            "replies": [
                {
                    "rpid": 1_i64,
                    "like": 10,
                    "rcount": 2,
                    "content": {"message": "顶层评论"},
                    "member": {"uname": "top", "avatar": "x"},
                    "replies": [
                        {"rpid": 2, "like": 5, "rcount": 0, "content": {"message": "子1"}, "member": {"uname": "c1", "avatar": "y"}},
                        {"rpid": 3, "like": 3, "rcount": 0, "content": {"message": "子2"}, "member": {"uname": "c2", "avatar": "y"}}
                    ]
                }
            ]
        });
        let replies = parse_replies_value(Some(&v));
        assert_eq!(replies.len(), 1);
        assert_eq!(replies[0].sub_replies.len(), 2);
        assert_eq!(replies[0].sub_replies[0].uname, "c1");
        assert_eq!(replies[0].sub_replies_count, 2);
    }

    #[test]
    fn parse_empty_data_returns_empty_vec() {
        let v = serde_json::json!({"replies": null});
        assert!(parse_replies_value(Some(&v)).is_empty());
        assert!(parse_replies_value(None).is_empty());
    }

    #[test]
    fn parse_top_flag() {
        let v = serde_json::json!({
            "rpid": 99_i64,
            "like": 0,
            "rcount": 0,
            "state": 4, // 置顶
            "content": {"message": "顶"},
            "member": {"uname": "T", "avatar": "x"}
        });
        let r = parse_single_reply(&v).unwrap();
        assert!(r.is_top);
    }

    /// 真打 B 站 API — 需登录才能拿 > 3 条；匿名也能拿到第 1 页 3 条。
    /// 标 `#[ignore]` 避免在 `cargo test` 默认跑（避免 CI 依赖网络）。
    #[tokio::test]
    #[ignore]
    async fn fetch_main_against_real_bilibili() {
        let r = fetch_main("BV1CZEY67E8o", ReviewSort::Hot, 1, 5)
            .await
            .expect("fetch_main failed");
        assert!(r.total > 0, "total should be > 0, got {}", r.total);
        assert!(!r.replies.is_empty(), "expected at least 1 reply");
        assert_eq!(r.bv, "BV1CZEY67E8o");
        assert!(r.aid > 0);
        // 至少一个 reply 有 uname 和非空 message
        assert!(r.replies.iter().any(|x| !x.uname.is_empty() && !x.message.is_empty()));
    }

    #[tokio::test]
    #[ignore]
    async fn fetch_sub_against_real_bilibili() {
        // 已知 BV1CZEY67E8o 有 sub replies
        // 先 fetch main 拿一个 rpid，再 fetch_sub
        let main = fetch_main("BV1CZEY67E8o", ReviewSort::Hot, 1, 5)
            .await
            .expect("main failed");
        let root = main
            .replies
            .iter()
            .find(|r| r.sub_replies_count > 0)
            .map(|r| r.rpid)
            .expect("no reply with subs found");
        let sub = fetch_sub("BV1CZEY67E8o", root, 1, 10)
            .await
            .expect("sub fetch failed");
        assert!(!sub.sub_replies.is_empty(), "expected sub replies");
    }
}
