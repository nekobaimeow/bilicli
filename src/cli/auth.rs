// SPDX-License-Identifier: GPL-3.0-or-later
// `auth` subcommand.

use crate::cli::output::Output;
use crate::cli::root::AuthCmd;
use crate::error::CliError;
use crate::ipc::login;
use serde::Serialize;

pub async fn run(cmd: AuthCmd, out: &Output) -> Result<(), CliError> {
    match cmd {
        AuthCmd::Qrcode { output } => cmd_qrcode(output, out).await,
        AuthCmd::QrcodePoll { key } => cmd_qrcode_poll(&key, out).await,
        AuthCmd::QrcodeCancel => {
            login::stop_login();
            out.status("scan login cancelled")
        }
        AuthCmd::Status => cmd_status(out).await,
        AuthCmd::Refresh => {
            login::refresh_cookie().await?;
            out.status("cookies refreshed")
        }
        AuthCmd::Exit => {
            login::exit().await?;
            out.status("logged out")
        }
    }
}

#[derive(Serialize)]
struct QrcodeOut<'a> {
    qr_url: &'a str,
    qrcode_key: &'a str,
    qr_png_path: Option<std::path::PathBuf>,
    qr_png_base64: Option<String>,
}

async fn cmd_qrcode(
    output: Option<std::path::PathBuf>,
    out: &Output,
) -> Result<(), CliError> {
    let start = login::start_scan_login().await?;
    let mut path_buf: Option<std::path::PathBuf> = None;
    let mut base64_png: Option<String> = None;
    if let Some(p) = output {
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        std::fs::write(&p, &start.qr_png)?;
        path_buf = Some(p);
    } else {
        // Inline base64 in JSON mode; in human mode just print the URL.
        use base64::Engine;
        base64_png = Some(base64::engine::general_purpose::STANDARD.encode(&start.qr_png));
    }
    let payload = QrcodeOut {
        qr_url: &start.qr_url,
        qrcode_key: &start.qrcode_key,
        qr_png_path: path_buf,
        qr_png_base64: base64_png,
    };
    out.ok(payload)
}

async fn cmd_qrcode_poll(key: &str, out: &Output) -> Result<(), CliError> {
    let ev = login::poll_scan_login(key).await?;
    out.ok(ev)
}

async fn cmd_status(out: &Output) -> Result<(), CliError> {
    let s = login::status().await;
    out.ok(s)
}
