// SPDX-License-Identifier: GPL-3.0-or-later
// `cache` subcommand.

use crate::backends::{http, paths};
use crate::cli::output::Output;
use crate::cli::root::CacheCmd;
use crate::error::CliError;
use serde::Serialize;

pub async fn run(cmd: CacheCmd, out: &Output) -> Result<(), CliError> {
    match cmd {
        CacheCmd::List => cmd_list(out),
        CacheCmd::Size { key } => cmd_size(&key, out),
        CacheCmd::Clean { key } => cmd_clean(&key, out).await,
        CacheCmd::Open { key } => cmd_open(&key, out),
    }
}

#[derive(Serialize)]
struct CacheEntry {
    name: String,
    path: String,
    size_bytes: u64,
    exists: bool,
}

fn entries() -> Vec<(&'static str, std::path::PathBuf)> {
    let p = paths::Paths::new().unwrap_or_else(|_| paths::Paths::new().unwrap());
    vec![
        ("data", p.data_dir()),
        ("logs", p.log_dir()),
        ("cache", p.cache_dir()),
        ("runtime", p.runtime_dir()),
    ]
}

fn cmd_list(out: &Output) -> Result<(), CliError> {
    let mut rows = Vec::new();
    for (name, path) in entries() {
        let size = if path.is_dir() {
            paths::dir_size(&path).unwrap_or(0)
        } else {
            0
        };
        rows.push(CacheEntry {
            name: name.to_string(),
            path: path.to_string_lossy().to_string(),
            size_bytes: size,
            exists: path.exists(),
        });
    }
    out.list(rows)
}

fn cmd_size(key: &str, out: &Output) -> Result<(), CliError> {
    let (name, path) = entries()
        .into_iter()
        .find(|(n, _)| *n == key)
        .ok_or_else(|| CliError::msg(format!("unknown cache: {key}")))?;
    let size = paths::dir_size(&path).unwrap_or(0);
    out.ok(serde_json::json!({
        "name": name,
        "path": path,
        "size_bytes": size,
    }))
}

async fn cmd_clean(key: &str, out: &Output) -> Result<(), CliError> {
    let (name, path) = entries()
        .into_iter()
        .find(|(n, _)| *n == key)
        .ok_or_else(|| CliError::msg(format!("unknown cache: {key}")))?;
    if !path.is_dir() {
        return out.ok(serde_json::json!({"name": name, "removed": 0, "existed": false}));
    }
    let mut entries_count = 0;
    let mut read_dir = tokio::fs::read_dir(&path).await?;
    while let Some(e) = read_dir.next_entry().await? {
        let p = e.path();
        if p.is_dir() {
            tokio::fs::remove_dir_all(&p).await.ok();
        } else {
            tokio::fs::remove_file(&p).await.ok();
        }
        entries_count += 1;
    }
    out.ok(serde_json::json!({
        "name": name,
        "path": path,
        "removed": entries_count,
    }))
}

fn cmd_open(key: &str, out: &Output) -> Result<(), CliError> {
    let (_, path) = entries()
        .into_iter()
        .find(|(n, _)| *n == key)
        .ok_or_else(|| CliError::msg(format!("unknown cache: {key}")))?;
    std::fs::create_dir_all(&path).ok();
    http::open_path(&path.to_string_lossy())?;
    out.status(format!("opened {key} cache"));
    Ok(())
}
