// SPDX-License-Identifier: GPL-3.0-or-later
//! Health check — verify that the runtime is ready to download.

use crate::backends::sidecar::{resolve, SidecarKind};
use crate::error::CliError;
use crate::ipc::storage::db;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DoctorReport {
    pub ok: bool,
    pub checks: Vec<CheckResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckResult {
    pub name: String,
    pub ok: bool,
    pub detail: String,
    pub hint: Option<String>,
}

/// Run all health checks and return a structured report.
pub async fn run() -> Result<DoctorReport, CliError> {
    let mut checks = Vec::new();

    // 1. Database reachable
    match db::init().await {
        Ok(()) => checks.push(CheckResult {
            name: "database".into(),
            ok: true,
            detail: format!("SQLite at {}", db::db_path().to_string_lossy()),
            hint: None,
        }),
        Err(e) => checks.push(CheckResult {
            name: "database".into(),
            ok: false,
            detail: format!("{e}"),
            hint: Some("check BILITOOLS_DATA_DIR or filesystem permissions".into()),
        }),
    }

    // 2. aria2c
    checks.push(check_sidecar(SidecarKind::Aria2c));

    // 3. ffmpeg
    checks.push(check_sidecar(SidecarKind::FFmpeg));

    // 4. DanmakuFactory (optional)
    checks.push(check_sidecar(SidecarKind::DanmakuFactory));

    // 5. B 站 reachable
    checks.push(check_bilibili_reachable().await);

    let ok = checks.iter().all(|c| c.ok || c.name == "danmaku_factory"); // DanmakuFactory is optional
    Ok(DoctorReport { ok, checks })
}

fn check_sidecar(kind: SidecarKind) -> CheckResult {
    match resolve(kind, None) {
        Ok(p) => CheckResult {
            name: kind.name().to_string(),
            ok: true,
            detail: format!("found at {}", p.display()),
            hint: None,
        },
        Err(_) => CheckResult {
            name: kind.name().to_string(),
            ok: false,
            detail: "not found in PATH".into(),
            hint: Some(match kind {
                SidecarKind::Aria2c => {
                    "install aria2 (apt install aria2 / brew install aria2) or set sidecar.aria2c"
                        .into()
                }
                SidecarKind::FFmpeg => {
                    "install ffmpeg (apt install ffmpeg / brew install ffmpeg) or set sidecar.ffmpeg"
                        .into()
                }
                SidecarKind::DanmakuFactory => {
                    "download DanmakuFactory from https://github.com/hihkm/DanmakuFactory and set sidecar.danmakufactory"
                        .into()
                }
            }),
        },
    }
}

async fn check_bilibili_reachable() -> CheckResult {
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            return CheckResult {
                name: "bilibili_api".into(),
                ok: false,
                detail: format!("could not build HTTP client: {e}"),
                hint: None,
            }
        }
    };
    match client
        .get("https://api.bilibili.com/x/web-interface/nav")
        .send()
        .await
    {
        Ok(r) if r.status().is_success() => CheckResult {
            name: "bilibili_api".into(),
            ok: true,
            detail: format!("HTTP {}", r.status().as_u16()),
            hint: None,
        },
        Ok(r) => CheckResult {
            name: "bilibili_api".into(),
            ok: false,
            detail: format!("HTTP {}", r.status().as_u16()),
            hint: Some("bilibili may be blocked by your network; consider setting a proxy".into()),
        },
        Err(e) => CheckResult {
            name: "bilibili_api".into(),
            ok: false,
            detail: format!("{e}"),
            hint: Some("check your internet connection or proxy settings".into()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn doctor_returns_structured_report() {
        let r = run().await.unwrap();
        // Even if the network is down we still get a structured report.
        assert!(!r.checks.is_empty());
        assert!(r.checks.iter().any(|c| c.name == "database"));
    }
}
