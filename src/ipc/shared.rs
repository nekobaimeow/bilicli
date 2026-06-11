// SPDX-License-Identifier: GPL-3.0-or-later
// Originally: BiliTools `src-tauri/src/shared.rs`
// Adapted for CLI:
//   - Removed `tauri::AppHandle`, `tauri::Manager`, `tauri::Theme`, `WindowEffect`.
//   - Removed `tauri_plugin_http::reqwest` — replaced with `crate::backends::http::build_client`.
//   - Removed `tauri_plugin_shell::ShellExt` — replaced with `crate::backends::sidecar`.
//   - Removed `tauri_specta::Event` — replaced with `tracing` macros.
//   - `init_client_inner` now takes a `ProxyConfig` directly instead of reading from app.

use crate::backends::http::ProxyConfig;
use crate::error::CliError;
use crate::ipc::storage::cookies;
use arc_swap::ArcSwap;
use once_cell::sync::Lazy;
use rand::{distr::Alphanumeric, Rng};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;

pub const USER_AGENT: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/132.0.0.0 Safari/537.36";

pub const DEFAULT_REFERER: &str = "https://www.bilibili.com/";
pub const DEFAULT_ORIGIN: &str = "https://www.bilibili.com";

pub static CONFIG: Lazy<ArcSwap<ProxyConfig>> =
    Lazy::new(|| ArcSwap::from_pointee(ProxyConfig::default()));

/// Globally-shared HTTP headers (Cookie, User-Agent, Referer, Origin).
/// Mirrors `HEADERS` in BiliTools.
pub static HEADERS: Lazy<Headers> = Lazy::new(Headers::new);

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct HeadersData {
    #[serde(rename = "Cookie")]
    pub cookie: String,
    #[serde(rename = "User-Agent")]
    pub user_agent: String,
    #[serde(rename = "Referer")]
    pub referer: String,
    #[serde(rename = "Origin")]
    pub origin: String,
}

pub struct Headers {
    map: RwLock<BTreeMap<String, String>>,
}

impl Default for Headers {
    fn default() -> Self {
        Self::new()
    }
}

impl Headers {
    pub fn new() -> Self {
        let mut map = BTreeMap::new();
        map.insert("User-Agent".into(), USER_AGENT.into());
        map.insert("Referer".into(), DEFAULT_REFERER.into());
        map.insert("Origin".into(), DEFAULT_ORIGIN.into());
        map.insert("Cookie".into(), String::new());
        Self {
            map: RwLock::new(map),
        }
    }

    /// Re-read cookies from storage and rebuild the Cookie header.
    /// Replaces `Headers::refresh` in BiliTools.
    pub async fn refresh(&self) -> Result<(), CliError> {
        let mut map = self.map.write().await;
        let cookies = cookies::load()
            .await?
            .iter()
            .map(|(name, value)| {
                format!(
                    "{}={}",
                    name,
                    value
                        .to_string()
                        .replace("\\\"", "")
                        .trim_matches('"')
                )
            })
            .collect::<Vec<_>>()
            .join("; ");
        map.insert("Cookie".into(), cookies);

        // In the GUI, this emits an event so the WebView reloads. The CLI
        // has no such listener, so we just trace it.
        tracing::debug!("HEADERS refreshed (cookie length = {})", map.get("Cookie").map(|s| s.len()).unwrap_or(0));
        Ok(())
    }

    pub async fn to_header_map(&self) -> Result<HeaderMap, CliError> {
        let mut headers = HeaderMap::new();
        let map = self.map.read().await;
        for (key, value) in &*map {
            let name = HeaderName::from_bytes(key.as_bytes())
                .map_err(|e| CliError::msg(format!("invalid header name '{key}': {e}")))?;
            let val = HeaderValue::from_str(value)
                .map_err(|e| CliError::msg(format!("invalid header value for '{key}': {e}")))?;
            headers.insert(name, val);
        }
        Ok(headers)
    }

    pub async fn cookie(&self) -> String {
        self.map
            .read()
            .await
            .get("Cookie")
            .cloned()
            .unwrap_or_default()
    }
}

/// Initialize a reqwest client with the current headers, optionally using a proxy.
pub async fn init_client() -> Result<reqwest::Client, CliError> {
    init_client_inner(true).await
}

pub async fn init_client_no_proxy() -> Result<reqwest::Client, CliError> {
    init_client_inner(false).await
}

pub async fn init_client_inner(use_proxy: bool) -> Result<reqwest::Client, CliError> {
    use crate::backends::http::build_client_builder;
    let proxy = CONFIG.load();
    let mut builder = build_client_builder(&proxy, use_proxy)?;
    let headers = HEADERS.to_header_map().await?;
    builder = builder.default_headers(headers);
    Ok(builder.build()?)
}

/// Replace the global proxy configuration.
pub fn set_proxy(cfg: ProxyConfig) {
    CONFIG.store(Arc::new(cfg));
}

pub fn get_sec() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub fn get_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub fn random_string(len: usize) -> String {
    rand::rng()
        .sample_iter(&Alphanumeric)
        .take(len)
        .map(char::from)
        .collect()
}

/// Equivalent of `tauri::AppHandle.path().temp_dir()`.
pub fn temp_dir() -> PathBuf {
    std::env::temp_dir().join("bilitools")
}

pub fn ensure_temp_dir() -> std::io::Result<()> {
    let p = temp_dir();
    std::fs::create_dir_all(p)
}

/// If `auto_rename` is true and `path` exists, return a new path with `_1`, `_2`, ... suffix.
/// Equivalent of `get_unique_path` in BiliTools.
pub fn get_unique_path(mut path: PathBuf, auto_rename: bool) -> PathBuf {
    if !auto_rename {
        return path;
    }
    let mut count = 1;
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".into());
    let ext = path.extension().map(|e| e.to_string_lossy().to_string());
    while path.exists() {
        path.set_file_name(match &ext {
            Some(ext) => format!("{stem}_{count}.{ext}"),
            None => format!("{stem}_{count}"),
        });
        count += 1;
    }
    path
}

/// Equivalent of `get_image` in BiliTools — fetch a URL and write its bytes to `path`.
pub async fn get_image(path: &Path, url: &str) -> Result<(), CliError> {
    let client = init_client().await?;
    let response = client.get(url).send().await?;
    if !response.status().is_success() {
        return Err(CliError::http(
            response.status().as_u16(),
            format!("Error while fetching thumb {url}"),
        ));
    }
    let bytes = response.bytes().await?;
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::write(path, &bytes).await?;
    Ok(())
}

/// Replaces `process_err` — log an error and return the original.
pub fn process_err<T: ToString, U>(e: T, name: &str) -> T {
    tracing::error!("{name}: {}", e.to_string());
    e
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn headers_init_has_default_keys() {
        let h = Headers::new();
        let map = h.map.blocking_read();
        assert!(map.contains_key("User-Agent"));
        assert!(map.contains_key("Referer"));
        assert!(map.contains_key("Origin"));
        assert!(map.contains_key("Cookie"));
        assert_eq!(map.get("User-Agent").unwrap(), USER_AGENT);
    }

    #[test]
    fn headers_to_map_succeeds() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let h = Headers::new();
            let map = h.to_header_map().await.unwrap();
            assert!(map.contains_key("user-agent"));
            assert!(map.contains_key("referer"));
            assert!(map.contains_key("origin"));
        });
    }

    #[test]
    fn get_sec_and_millis_sensible() {
        let s = get_sec();
        let m = get_millis();
        // 2025-01-01 ~ 1735689600; 2026 ~ 1767225600
        assert!(s > 1_700_000_000, "sec should be > 2023, got {s}");
        assert!(m > s * 1000, "millis should be > sec*1000");
        assert!(m - s * 1000 < 1000, "millis excess should be < 1000");
    }

    #[test]
    fn random_string_length() {
        assert_eq!(random_string(0).len(), 0);
        assert_eq!(random_string(8).len(), 8);
        assert_eq!(random_string(32).len(), 32);
    }

    #[test]
    fn random_string_differs() {
        let a = random_string(32);
        let b = random_string(32);
        assert_ne!(a, b);
    }

    #[test]
    fn get_unique_path_no_collision() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("x.txt");
        std::fs::write(&p, b"").unwrap();
        let unique = get_unique_path(p.clone(), true);
        assert_ne!(unique, p);
        assert!(unique.to_string_lossy().contains("x_1.txt"));
    }

    #[test]
    fn get_unique_path_disabled_returns_same() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("y.txt");
        std::fs::write(&p, b"").unwrap();
        let unique = get_unique_path(p.clone(), false);
        assert_eq!(unique, p);
    }

    #[test]
    fn temp_dir_is_bilitools_named() {
        let p = temp_dir();
        assert!(p.ends_with("bilitools"));
    }
}
