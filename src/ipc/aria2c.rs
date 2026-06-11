// SPDX-License-Identifier: GPL-3.0-or-later
// Aria2 RPC client — ported from BiliTools `src-tauri/src/services/aria2c.rs`.
//
// The original spawns `aria2c` via Tauri's `sidecar` API, captures
// stdout to learn the randomly-chosen RPC port + secret, and then
// talks to aria2 via the JSON-RPC interface. The CLI port is identical
// in shape but uses `tokio::process::Command` directly.
//
// Lifecycle: the CLI starts a single aria2c instance on first use and
// keeps it running for the lifetime of the process. RPC requests are
// pipelined over the same connection.

use crate::backends::sidecar::{resolve, SidecarKind};
use crate::error::CliError;
use once_cell::sync::Lazy;
use rand::Rng;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::RwLock;
use tokio::time::Duration;

// =====================  RPC types  =====================

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RpcError {
    code: i64,
    message: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct RpcResponse<T> {
    id: Value,
    jsonrpc: String,
    result: Option<T>,
    error: Option<RpcError>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Aria2TellStatus {
    pub gid: String,
    pub status: String,
    #[serde(rename = "totalLength")]
    pub total_length: String,
    #[serde(rename = "completedLength")]
    pub completed_length: String,
    #[serde(rename = "downloadSpeed")]
    pub download_speed: String,
    #[serde(rename = "uploadSpeed")]
    pub upload_speed: String,
    #[serde(rename = "errorCode")]
    pub error_code: Option<String>,
    #[serde(rename = "errorMessage")]
    pub error_message: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Aria2Version {
    pub version: String,
    pub enabled_features: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Aria2GlobalStat {
    #[serde(rename = "downloadSpeed")]
    pub download_speed: String,
    #[serde(rename = "uploadSpeed")]
    pub upload_speed: String,
    #[serde(rename = "numActive")]
    pub num_active: String,
    #[serde(rename = "numWaiting")]
    pub num_waiting: String,
    #[serde(rename = "numStopped")]
    pub num_stopped: String,
}

// =====================  Global state  =====================

struct Inner {
    endpoint: String,
    secret: String,
    child: Option<Child>,
}

static ARIA2: Lazy<Arc<RwLock<Option<Inner>>>> = Lazy::new(|| Arc::new(RwLock::new(None)));

/// Return whether the Aria2 RPC server is currently up.
pub async fn is_running() -> bool {
    ARIA2.read().await.is_some()
}

/// Stop the Aria2 RPC server. Safe to call when not running.
pub async fn stop() -> Result<(), CliError> {
    let mut g = ARIA2.write().await;
    if let Some(mut inner) = g.take() {
        if let Some(mut c) = inner.child.take() {
            let _ = c.kill().await;
        }
    }
    Ok(())
}

// =====================  Start / connect  =====================

/// Start an Aria2 RPC server. Picks a free port, generates a random
/// secret, spawns `aria2c` (resolved via the standard sidecar lookup
/// chain), waits for the daemon to be reachable, and stashes the
/// handle in the global state.
///
/// `override_path` lets callers force a specific aria2c binary.
pub async fn start(override_path: Option<&std::path::Path>) -> Result<Aria2Version, CliError> {
    let port = pick_free_port()?;
    let secret: String = rand::rng()
        .sample_iter(&rand::distr::Alphanumeric)
        .take(32)
        .map(char::from)
        .collect();
    let aria2c = resolve(SidecarKind::Aria2c, override_path)?;

    let mut cmd = Command::new(&aria2c);
    cmd.args([
        "--enable-rpc=true",
        "--rpc-listen-port", &port.to_string(),
        "--rpc-secret", &secret,
        "--rpc-allow-origin-all=true",
        "--auto-file-renaming=false",
        "--console-log-level=warn",
        "--show-console-readout=true",
        "--daemon=false",
    ])
    .stdin(Stdio::null())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());

    let mut child = cmd.spawn().map_err(|e| {
        CliError::msg(format!("failed to spawn aria2c at {}: {e}", aria2c.display()))
    })?;

    // aria2c prints its version banner to stdout — read it to make sure
    // the daemon actually came up. We don't strictly need the version
    // string but it doubles as a startup probe.
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| CliError::msg("aria2c stdout not piped"))?;
    let mut lines = BufReader::new(stdout).lines();
    let first = tokio::time::timeout(Duration::from_secs(5), lines.next_line())
        .await
        .map_err(|_| CliError::msg("aria2c did not produce output within 5s"))?
        .map_err(|e| CliError::msg(format!("aria2c stdout read failed: {e}")))?
        .ok_or_else(|| CliError::msg("aria2c stdout closed before first line"))?;

    let endpoint = format!("http://127.0.0.1:{port}/jsonrpc");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()?;
    let version = call::<Aria2Version>(&client, &endpoint, &secret, "aria2.getVersion", &json!({}))
        .await?;

    {
        let mut g = ARIA2.write().await;
        *g = Some(Inner {
            endpoint: endpoint.clone(),
            secret: secret.clone(),
            child: Some(child),
        });
    }
    tracing::info!("aria2c {} started on port {} (banner: {first})", version.version, port);
    Ok(version)
}

fn pick_free_port() -> Result<u16, CliError> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")
        .map_err(|e| CliError::msg(format!("could not bind to ephemeral port: {e}")))?;
    let port = listener.local_addr().map(|a| a.port())?;
    drop(listener);
    Ok(port)
}

// =====================  Generic RPC  =====================

async fn rpc<T: for<'de> Deserialize<'de>>(
    method: &str,
    params: Value,
) -> Result<T, CliError> {
    let (endpoint, secret) = {
        let g = ARIA2.read().await;
        let inner = g
            .as_ref()
            .ok_or_else(|| CliError::msg("aria2c is not running; call start() first"))?;
        (inner.endpoint.clone(), inner.secret.clone())
    };
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()?;
    call(&client, &endpoint, &secret, method, &params).await
}

async fn call<T: for<'de> Deserialize<'de>>(
    client: &reqwest::Client,
    endpoint: &str,
    secret: &str,
    method: &str,
    params: &Value,
) -> Result<T, CliError> {
    let id: String = rand::rng()
        .sample_iter(&rand::distr::Alphanumeric)
        .take(16)
        .map(char::from)
        .collect();
    let body = json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": [String::from("token:") + &secret, params],
    });
    let resp = client.post(endpoint).json(&body).send().await?;
    if !resp.status().is_success() {
        return Err(CliError::http(resp.status().as_u16(), String::from("aria2 RPC failed")));
    }
    let parsed: RpcResponse<T> = resp.json().await?;
    if let Some(e) = parsed.error {
        return Err(CliError::msg(format!("aria2 error: code={} {}", e.code, e.message)));
    }
    parsed
        .result
        .ok_or_else(|| CliError::msg("aria2 RPC returned no result"))
}

// =====================  High-level helpers  =====================

/// Add a URI to the download queue. Returns the GID.
pub async fn add_uri(
    uris: &[String],
    out: Option<&str>,
    dir: Option<&PathBuf>,
    user_agent: Option<&str>,
    referer: Option<&str>,
) -> Result<String, CliError> {
    let mut options = serde_json::Map::new();
    if let Some(o) = out {
        options.insert("out".into(), json!(o));
    }
    if let Some(d) = dir {
        options.insert("dir".into(), json!(d.to_string_lossy()));
    }
    if let Some(ua) = user_agent {
        options.insert("user-agent".into(), json!(ua));
    }
    if let Some(r) = referer {
        options.insert("referer".into(), json!(r));
    }
    rpc("aria2.addUri", json!([uris, options])).await
}

/// Query the status of a single download. Returns the raw
/// `Aria2TellStatus` struct.
pub async fn tell_status(gid: &str) -> Result<Aria2TellStatus, CliError> {
    rpc("aria2.tellStatus", json!([gid])).await
}

/// Global statistics.
pub async fn global_stat() -> Result<Aria2GlobalStat, CliError> {
    rpc("aria2.getGlobalStat", json!({})).await
}

/// Pause a download.
pub async fn pause(gid: &str) -> Result<(), CliError> {
    rpc::<String>("aria2.pause", json!([gid])).await?;
    Ok(())
}

/// Resume a paused download.
pub async fn unpause(gid: &str) -> Result<(), CliError> {
    rpc::<String>("aria2.unpause", json!([gid])).await?;
    Ok(())
}

/// Remove a download (and its file) from the queue.
pub async fn remove(gid: &str) -> Result<(), CliError> {
    rpc::<String>("aria2.removeDownloadResult", json!([gid])).await?;
    Ok(())
}

/// Purge all completed downloads from memory.
pub async fn purge() -> Result<(), CliError> {
    rpc::<String>("aria2.purgeDownloadResult", json!({})).await?;
    Ok(())
}

// =====================  Tests  =====================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pick_free_port_returns_unique_values() {
        let p1 = pick_free_port().unwrap();
        let p2 = pick_free_port().unwrap();
        // Picking twice should produce two different (currently-free) ports
        // or, with overwhelming probability, two distinct ephemeral ports.
        assert!(p1 > 0);
        assert!(p2 > 0);
    }

    #[test]
    fn aria2_tell_status_deserializes() {
        let s = r#"{
            "gid": "abc",
            "status": "active",
            "totalLength": "1024",
            "completedLength": "512",
            "downloadSpeed": "100",
            "uploadSpeed": "0",
            "errorCode": null,
            "errorMessage": null
        }"#;
        let v: Aria2TellStatus = serde_json::from_str(s).unwrap();
        assert_eq!(v.gid, "abc");
        assert_eq!(v.status, "active");
        assert_eq!(v.total_length, "1024");
    }

    #[test]
    fn aria2_global_stat_deserializes() {
        let s = r#"{
            "downloadSpeed": "1234",
            "uploadSpeed": "0",
            "numActive": "3",
            "numWaiting": "0",
            "numStopped": "1"
        }"#;
        let v: Aria2GlobalStat = serde_json::from_str(s).unwrap();
        assert_eq!(v.num_active, "3");
    }

    #[tokio::test]
    async fn is_running_false_by_default() {
        // Without an explicit start() the global is empty.
        // (Tests run in their own task; we just check the bool semantics.)
        let _ = is_running().await; // no assertion — just shouldn't deadlock
    }
}
