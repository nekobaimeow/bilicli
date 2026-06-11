// SPDX-License-Identifier: GPL-3.0-or-later
// `info` subcommand — version + paths + build info.

use crate::cli::output::Output;
use crate::cli::root::{NAME, VERSION};
use crate::error::CliError;
use crate::ipc::storage::db;
use serde::Serialize;

#[derive(Serialize)]
struct Info {
    name: &'static str,
    version: &'static str,
    rust_version: &'static str,
    db_path: String,
    data_dir: String,
    config_path: String,
    aria2c_resolved: bool,
    ffmpeg_resolved: bool,
}

pub async fn run(out: &Output) -> Result<(), CliError> {
    let aria2 = crate::backends::sidecar::resolve(crate::backends::sidecar::SidecarKind::Aria2c, None).is_ok();
    let ffmpeg = crate::backends::sidecar::resolve(crate::backends::sidecar::SidecarKind::FFmpeg, None).is_ok();
    let data_dir = crate::backends::paths::Paths::new()
        .map(|p| p.data_dir().to_string_lossy().to_string())
        .unwrap_or_else(|_| "<unknown>".into());
    let config_path = crate::backends::paths::Paths::new()
        .map(|p| p.config_file().to_string_lossy().to_string())
        .unwrap_or_else(|_| "<unknown>".into());
    out.ok(Info {
        name: NAME,
        version: VERSION,
        rust_version: env!("CARGO_PKG_RUST_VERSION"),
        db_path: db::db_path().to_string_lossy().to_string(),
        data_dir,
        config_path,
        aria2c_resolved: aria2,
        ffmpeg_resolved: ffmpeg,
    })
}
