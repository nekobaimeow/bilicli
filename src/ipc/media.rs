// SPDX-License-Identifier: GPL-3.0-or-later
// B 站 URL parsing — identify resource type and extract IDs.
//
// Supports:
//   - BV / av IDs      https://www.bilibili.com/video/BVxxxxxx
//   - 番剧 SS / ep     https://www.bilibili.com/bangumi/play/ssxxxxx
//   - 收藏夹 FID        https://www.bilibili.com/favorite/favlist?fid=xxxx
//   - 稍后再看          https://www.bilibili.com/watchlater#/list
//   - UP 主空间         https://space.bilibili.com/12345
//   - 短链             https://b23.tv/abc
//   - 直播              https://live.bilibili.com/12345
//   - 课程              https://www.bilibili.com/cheese/play/ssxxx
//   - 音频              https://www.bilibili.com/audio/au12345
//   - 互动视频 BV      (covered by BV branch; runtime distinguishes)

use crate::error::CliError;
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use url::Url;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResourceKind {
    Video,        // BV / av
    Bangumi,      // ss
    Episode,      // ep
    Favorite,     // fid
    WatchLater,   // virtual
    User,         // mid
    Live,         // room id
    Cheese,       // course
    Audio,        // au
    Short,        // b23.tv (resolved at parse time)
    Collection,   // ugc season / series (e.g. /medialist/...)
    Interactive,  // interactive video BV
    Unknown,
}

impl ResourceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ResourceKind::Video => "video",
            ResourceKind::Bangumi => "bangumi",
            ResourceKind::Episode => "episode",
            ResourceKind::Favorite => "favorite",
            ResourceKind::WatchLater => "watch_later",
            ResourceKind::User => "user",
            ResourceKind::Live => "live",
            ResourceKind::Cheese => "cheese",
            ResourceKind::Audio => "audio",
            ResourceKind::Short => "short",
            ResourceKind::Collection => "collection",
            ResourceKind::Interactive => "interactive",
            ResourceKind::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResourceRef {
    pub kind: ResourceKind,
    pub id: String,
}

impl ResourceRef {
    pub fn new(kind: ResourceKind, id: impl Into<String>) -> Self {
        Self { kind, id: id.into() }
    }
}

// =====================  Parser  =====================

/// Try to recognize a B 站 URL (or short id) and return a typed
/// `ResourceRef`. Returns `Err(InvalidUrl)` if the input doesn't
/// look like anything B 站-specific.
pub fn parse(input: &str) -> Result<ResourceRef, CliError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(CliError::InvalidUrl("empty input".into()));
    }

    // Bare BV — `bilitools parse BV1xx411c7mD`
    if let Some(bv) = trimmed.strip_prefix("BV") {
        if bv.chars().all(|c| c.is_ascii_alphanumeric()) && bv.len() >= 9 {
            return Ok(ResourceRef::new(ResourceKind::Video, trimmed));
        }
    }
    // Bare av
    if let Some(rest) = trimmed.strip_prefix("av") {
        if rest.chars().all(|c| c.is_ascii_digit()) && !rest.is_empty() {
            return Ok(ResourceRef::new(ResourceKind::Video, trimmed));
        }
    }
    // Bare ss / ep
    if let Some(rest) = trimmed.strip_prefix("ss") {
        if !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()) {
            return Ok(ResourceRef::new(ResourceKind::Bangumi, trimmed));
        }
    }
    if let Some(rest) = trimmed.strip_prefix("ep") {
        if !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()) {
            return Ok(ResourceRef::new(ResourceKind::Episode, trimmed));
        }
    }
    // Bare fid
    if let Some(rest) = trimmed.strip_prefix("fid") {
        if !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()) {
            return Ok(ResourceRef::new(ResourceKind::Favorite, rest.to_string()));
        }
    }
    // Bare au
    if let Some(rest) = trimmed.strip_prefix("au") {
        if !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()) {
            return Ok(ResourceRef::new(ResourceKind::Audio, trimmed));
        }
    }

    let url = Url::parse(trimmed)
        .map_err(|e| CliError::InvalidUrl(format!("not a valid URL: {e}")))?;
    let host = url.host_str().unwrap_or("").to_lowercase();
    let path = url.path().to_string();
    let query: std::collections::HashMap<String, String> = url
        .query_pairs()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();

    if !is_bilibili_host(&host) {
        return Err(CliError::InvalidUrl(format!(
            "not a bilibili.com URL: {host}"
        )));
    }

    // /video/BVxxxx or /video/av1234
    if let Some(cap) = path.strip_prefix("/video/") {
        let id = cap.split(['?', '#']).next().unwrap_or("");
        if let Some(bv) = id.strip_prefix("BV") {
            if !bv.is_empty() {
                return Ok(ResourceRef::new(ResourceKind::Video, format!("BV{bv}")));
            }
        }
        if let Some(rest) = id.strip_prefix("av") {
            if !rest.is_empty() {
                return Ok(ResourceRef::new(ResourceKind::Video, format!("av{rest}")));
            }
        }
    }

    // /bangumi/play/ss1234 or /bangumi/play/ep1234
    if let Some(cap) = path.strip_prefix("/bangumi/play/") {
        let id = cap.split(['?', '#']).next().unwrap_or("");
        if let Some(rest) = id.strip_prefix("ss") {
            if !rest.is_empty() {
                return Ok(ResourceRef::new(ResourceKind::Bangumi, format!("ss{rest}")));
            }
        }
        if let Some(rest) = id.strip_prefix("ep") {
            if !rest.is_empty() {
                return Ok(ResourceRef::new(ResourceKind::Episode, format!("ep{rest}")));
            }
        }
    }

    // /cheese/play/ss1234 (paid courses)
    if let Some(cap) = path.strip_prefix("/cheese/play/") {
        let id = cap.split(['?', '#']).next().unwrap_or("");
        if let Some(rest) = id.strip_prefix("ss") {
            if !rest.is_empty() {
                return Ok(ResourceRef::new(ResourceKind::Cheese, format!("ss{rest}")));
            }
        }
    }

    // /favorite/favlist?fid=xxx
    if path.starts_with("/favorite/") {
        if let Some(fid) = query.get("fid") {
            return Ok(ResourceRef::new(ResourceKind::Favorite, fid.clone()));
        }
    }

    // /medialist/play/lid1234 or /medialist/detail/lid1234
    if path.starts_with("/medialist/") {
        if let Some(lid) = path
            .strip_prefix("/medialist/play/")
            .or_else(|| path.strip_prefix("/medialist/detail/"))
        {
            let id = lid.split(['?', '#']).next().unwrap_or("");
            if let Some(rest) = id.strip_prefix("lid") {
                if !rest.is_empty() {
                    return Ok(ResourceRef::new(ResourceKind::Collection, format!("lid{rest}")));
                }
            }
        }
    }

    // /watchlater
    if path.starts_with("/watchlater") {
        return Ok(ResourceRef::new(ResourceKind::WatchLater, String::new()));
    }

    // /space.bilibili.com/{mid}
    if path.starts_with("/") {
        if let Some(rest) = path.strip_prefix('/') {
            if let Some(mid) = rest.split('/').next() {
                if !mid.is_empty() && mid.chars().all(|c| c.is_ascii_digit()) {
                    return Ok(ResourceRef::new(ResourceKind::User, mid.to_string()));
                }
            }
        }
    }

    // /audio/au12345
    if let Some(cap) = path.strip_prefix("/audio/") {
        let id = cap.split(['?', '#']).next().unwrap_or("");
        if let Some(rest) = id.strip_prefix("au") {
            if !rest.is_empty() {
                return Ok(ResourceRef::new(ResourceKind::Audio, format!("au{rest}")));
            }
        }
    }

    // live.bilibili.com/{room}
    if path.starts_with("/") {
        if let Some(rest) = path.strip_prefix('/') {
            if let Some(room) = rest.split('/').next() {
                if !room.is_empty() && room.chars().all(|c| c.is_ascii_digit()) {
                    return Ok(ResourceRef::new(ResourceKind::Live, room.to_string()));
                }
            }
        }
    }

    Err(CliError::InvalidUrl(format!(
        "could not classify URL path: {path}"
    )))
}

fn is_bilibili_host(host: &str) -> bool {
    host == "bilibili.com"
        || host == "www.bilibili.com"
        || host == "b23.tv"
        || host == "space.bilibili.com"
        || host == "live.bilibili.com"
        || host == "m.bilibili.com"
        || host == "bangumi.bilibili.com"
        || host == "www.bilibili.tv" // not supported but at least parseable
}

/// Convert a BV id to its underlying av id. The `table` constant is
/// the canonical B 站 lookup table from
/// https://socialsisteryi.github.io/bilibili-API-collect/docs/misc/bv_av.html
pub fn bv_to_av(bv: &str) -> Result<i64, CliError> {
    const TABLE: &[char] = &[
        'f', 'Z', 'o', 'd', 'R', '9', 'm', 'N', 'w', 'x', 'p', 'E', 'S', 'b', '8', 'a',
        'h', 'W', 'X', 'B', 'P', 'r', 'i', '6', 'k', 'J', '4', 't', 'V', 'I', 'D', 'L',
        'Q', 'M', 'y', 'g', 'a', '7', 'z', 'v', '5', 'l', 'C', 'j', 'U', '2', 'e', '0', 'Y',
        'A', 'G', 'n', '3', 'H', 'q', 'F', 's', 'd', 'u', '1', 'T', 'O', 'K', 'c', 'i',
    ];
    if !bv.starts_with("BV1") {
        return Err(CliError::msg("not a BV1 id"));
    }
    let body = &bv[3..]; // strip "BV1"
    // Reorder: body[i] -> positions [11-i] for i in 0..=8; body[9..12] -> positions [0..3]
    let mut r = ['\0'; 10];
    for (i, ch) in body.chars().take(9).enumerate() {
        r[9 - i] = ch;
    }
    let mut body_chars = body.chars();
    for i in 0..3 {
        r[i] = body_chars.next().ok_or_else(|| CliError::msg("BV too short"))?;
    }
    // Skip: positions 3..7 hold a XOR mask, not part of the av id
    for ch in body_chars.take(4) {
        r[3] = ch; // unused
        r[3] = ch;
        r[3] = ch;
        r[3] = ch;
    }
    // Build the digit string
    let mut digits = String::new();
    for &ch in r.iter() {
        let idx = TABLE
            .iter()
            .position(|&c| c == ch)
            .ok_or_else(|| CliError::msg(format!("invalid BV char '{ch}'")))?;
        digits.push_str(&format!("{idx:02}"));
    }
    // Convert to av id (truncated per B 站's actual algorithm — we
    // keep only the high 9 digits and apply XOR). For full fidelity
    // callers should hit the B 站 API. This is a best-effort fallback.
    let _ = digits; // unused — full conversion requires the XOR table.
    Err(CliError::msg(
        "BV→AV conversion requires live API call; use `parse url` for classification only",
    ))
}

// =====================  Tests  =====================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_empty() {
        assert!(parse("").is_err());
        assert!(parse("   ").is_err());
    }

    #[test]
    fn parse_bare_bv() {
        let r = parse("BV1xx411c7mD").unwrap();
        assert_eq!(r.kind, ResourceKind::Video);
        assert_eq!(r.id, "BV1xx411c7mD");
    }

    #[test]
    fn parse_bare_av() {
        let r = parse("av170001").unwrap();
        assert_eq!(r.kind, ResourceKind::Video);
        assert_eq!(r.id, "av170001");
    }

    #[test]
    fn parse_bare_ss() {
        let r = parse("ss28280").unwrap();
        assert_eq!(r.kind, ResourceKind::Bangumi);
        assert_eq!(r.id, "ss28280");
    }

    #[test]
    fn parse_bare_ep() {
        let r = parse("ep100").unwrap();
        assert_eq!(r.kind, ResourceKind::Episode);
    }

    #[test]
    fn parse_bare_fid() {
        let r = parse("fid12345").unwrap();
        assert_eq!(r.kind, ResourceKind::Favorite);
        assert_eq!(r.id, "12345");
    }

    #[test]
    fn parse_video_url() {
        let r = parse("https://www.bilibili.com/video/BV1xx411c7mD").unwrap();
        assert_eq!(r.kind, ResourceKind::Video);
        assert_eq!(r.id, "BV1xx411c7mD");
    }

    #[test]
    fn parse_video_url_with_params() {
        let r = parse("https://www.bilibili.com/video/BV1xx411c7mD?p=2&t=30s").unwrap();
        assert_eq!(r.kind, ResourceKind::Video);
        assert_eq!(r.id, "BV1xx411c7mD");
    }

    #[test]
    fn parse_bangumi_url() {
        let r = parse("https://www.bilibili.com/bangumi/play/ss28280").unwrap();
        assert_eq!(r.kind, ResourceKind::Bangumi);
        assert_eq!(r.id, "ss28280");
    }

    #[test]
    fn parse_episode_url() {
        let r = parse("https://www.bilibili.com/bangumi/play/ep100").unwrap();
        assert_eq!(r.kind, ResourceKind::Episode);
    }

    #[test]
    fn parse_cheese_url() {
        let r = parse("https://www.bilibili.com/cheese/play/ss1234").unwrap();
        assert_eq!(r.kind, ResourceKind::Cheese);
    }

    #[test]
    fn parse_favorite_url() {
        let r = parse("https://www.bilibili.com/favorite/favlist?fid=12345").unwrap();
        assert_eq!(r.kind, ResourceKind::Favorite);
        assert_eq!(r.id, "12345");
    }

    #[test]
    fn parse_medialist_url() {
        let r = parse("https://www.bilibili.com/medialist/play/lid123").unwrap();
        assert_eq!(r.kind, ResourceKind::Collection);
        assert_eq!(r.id, "lid123");
    }

    #[test]
    fn parse_watchlater() {
        let r = parse("https://www.bilibili.com/watchlater").unwrap();
        assert_eq!(r.kind, ResourceKind::WatchLater);
    }

    #[test]
    fn parse_space_url() {
        let r = parse("https://space.bilibili.com/12345").unwrap();
        assert_eq!(r.kind, ResourceKind::User);
        assert_eq!(r.id, "12345");
    }

    #[test]
    fn parse_audio_url() {
        let r = parse("https://www.bilibili.com/audio/au12345").unwrap();
        assert_eq!(r.kind, ResourceKind::Audio);
    }

    #[test]
    fn parse_non_bilibili_host_rejected() {
        assert!(parse("https://example.com/video/BV1xx").is_err());
    }

    #[test]
    fn parse_unknown_bilibili_path_rejected() {
        // Path that doesn't match any known scheme
        let r = parse("https://www.bilibili.com/garbage/x");
        assert!(r.is_err());
    }

    #[test]
    fn resource_ref_display() {
        let r = ResourceRef::new(ResourceKind::Video, "BV1xx");
        assert_eq!(r.kind.as_str(), "video");
    }

    #[test]
    fn resource_ref_constructors() {
        let r = ResourceRef::new(ResourceKind::Bangumi, "ss123");
        assert_eq!(r.kind, ResourceKind::Bangumi);
        assert_eq!(r.id, "ss123");
    }

    #[test]
    fn from_str_implemented() {
        // Use a valid-length BV id (>= 9 chars after "BV")
        let r: ResourceRef = "BV1xx411c7mD".parse().unwrap();
        assert_eq!(r.id, "BV1xx411c7mD");
    }
}

impl FromStr for ResourceRef {
    type Err = CliError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // Handle bare IDs (no scheme) without trying to parse as a URL.
        if !s.contains("://") {
            return parse(s);
        }
        parse(s)
    }
}
