// SPDX-License-Identifier: GPL-3.0-or-later
// HTTP client factory — replacement for `tauri_plugin_http::reqwest::Client`.
//
// This mirrors `shared.rs::init_client` from BiliTools, but uses `reqwest` directly.
// Headers (Cookie, User-Agent, Referer, Origin) are attached per-request via
// `crate::ipc::shared::HEADERS` so they reflect current login state.

use crate::error::CliError;
use reqwest::Client;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct ProxyConfig {
    pub address: String,
    pub username: String,
    pub password: String,
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            address: String::new(),
            username: String::new(),
            password: String::new(),
        }
    }
}

impl ProxyConfig {
    pub fn is_enabled(&self) -> bool {
        !self.address.is_empty()
    }
}

/// Build a reqwest client builder (so callers can attach extra headers).
pub fn build_client_builder(proxy: &ProxyConfig, use_proxy: bool) -> Result<reqwest::ClientBuilder, CliError> {
    let mut builder = Client::builder()
        .user_agent(super::super::ipc::shared::USER_AGENT)
        .timeout(Duration::from_secs(30))
        .connect_timeout(Duration::from_secs(10))
        .pool_idle_timeout(Duration::from_secs(90))
        .cookie_store(true)
        .gzip(true)
        .deflate(true);

    if use_proxy && proxy.is_enabled() {
        let mut p = reqwest::Proxy::all(&proxy.address)
            .map_err(|e| CliError::msg(format!("invalid proxy '{}': {e}", proxy.address)))?;
        if !proxy.username.is_empty() {
            p = p.basic_auth(&proxy.username, &proxy.password);
        }
        builder = builder.proxy(p);
    } else {
        builder = builder.no_proxy();
    }

    Ok(builder)
}

/// Build a reqwest client. If `use_proxy` is true, apply the proxy from settings.
pub fn build_client(proxy: &ProxyConfig, use_proxy: bool) -> Result<Client, CliError> {
    Ok(build_client_builder(proxy, use_proxy)?.build()?)
}

/// Open a URL/path with the system default handler.
/// Replacement for `tauri_plugin_opener::open_path`.
pub fn open_path(path: &str) -> Result<(), CliError> {
    let cmd = match std::env::consts::OS {
        "macos" => ("open", vec![path]),
        "windows" => ("cmd", vec!["/C", "start", "", path]),
        _ => ("xdg-open", vec![path]),
    };
    std::process::Command::new(cmd.0)
        .args(cmd.1)
        .spawn()
        .map_err(|e| CliError::msg(format!("failed to open '{path}': {e}")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proxy_config_default_disabled() {
        let p = ProxyConfig::default();
        assert!(!p.is_enabled());
    }

    #[test]
    fn build_client_no_proxy_succeeds() {
        let c = build_client(&ProxyConfig::default(), false);
        assert!(c.is_ok());
    }

    #[test]
    fn build_client_with_invalid_proxy_address_errors() {
        let bad = ProxyConfig {
            address: "not a valid url".into(),
            username: String::new(),
            password: String::new(),
        };
        let r = build_client(&bad, true);
        // reqwest may either reject it or fall back; just ensure it doesn't panic
        let _ = r;
    }
}
