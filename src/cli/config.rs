// SPDX-License-Identifier: GPL-3.0-or-later
// `config` subcommand.

use crate::cli::output::Output;
use crate::cli::root::ConfigCmd;
use crate::error::CliError;
use crate::ipc::storage::config;

pub async fn run(cmd: ConfigCmd, out: &Output) -> Result<(), CliError> {
    match cmd {
        ConfigCmd::Show => cmd_show(out).await,
        ConfigCmd::Get { key } => cmd_get(&key, out).await,
        ConfigCmd::Set { key, value } => cmd_set(&key, &value, out).await,
        ConfigCmd::Reset => cmd_reset(out).await,
        ConfigCmd::Path => cmd_path(out).await,
    }
}

async fn cmd_show(out: &Output) -> Result<(), CliError> {
    let s = config::read().await;
    out.ok(serde_json::to_value(&s).unwrap_or(serde_json::Value::Null))
}

async fn cmd_get(key: &str, out: &Output) -> Result<(), CliError> {
    let v = config::get(key).await?;
    out.ok(v)
}

async fn cmd_set(key: &str, value: &str, out: &Output) -> Result<(), CliError> {
    // Walk the dotted path and set the value.
    let mut json: serde_json::Value =
        serde_json::to_value(config::read().await).map_err(|e| CliError::msg(e.to_string()))?;
    let parts: Vec<&str> = key.split('.').collect();
    if let Some((last, parents)) = parts.split_last() {
        let mut cur = &mut json;
        for p in parents {
            cur = cur
                .get_mut(p)
                .ok_or_else(|| CliError::msg(format!("unknown config key: {key}")))?;
        }
        // Try to preserve the type of the existing value.
        let new_v = if let Some(existing) = cur.get(last) {
            match existing {
                serde_json::Value::Bool(_) => serde_json::Value::Bool(parse_bool(value)),
                serde_json::Value::Number(n) => {
                    if let Some(i) = n.as_i64() {
                        serde_json::Value::Number(serde_json::Number::from(
                            value.parse::<i64>().map_err(|_| {
                                CliError::msg(format!("not an integer: {value}"))
                            })?,
                        ))
                    } else if let Some(u) = n.as_u64() {
                        serde_json::Value::Number(serde_json::Number::from(
                            value.parse::<u64>().map_err(|_| {
                                CliError::msg(format!("not an unsigned: {value}"))
                            })?,
                        ))
                    } else {
                        serde_json::Value::Number(serde_json::Number::from_f64(
                            value
                                .parse::<f64>()
                                .map_err(|_| CliError::msg(format!("not a number: {value}")))?,
                        )
                        .ok_or_else(|| CliError::msg(format!("not a number: {value}")))?)
                    }
                }
                _ => serde_json::Value::String(value.to_string()),
            }
        } else {
            serde_json::Value::String(value.to_string())
        };
        cur.as_object_mut()
            .ok_or_else(|| CliError::msg(String::from("not a settable field")))?
            .insert((*last).to_string(), new_v);
    }
    let new_settings: config::Settings = serde_json::from_value(json)?;
    config::write(&new_settings).await?;
    out.ok(serde_json::json!({"key": key, "value": value}))
}

fn parse_bool(s: &str) -> bool {
    matches!(s.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on")
}

async fn cmd_reset(out: &Output) -> Result<(), CliError> {
    let s = config::Settings::defaults();
    config::write(&s).await?;
    out.status("configuration reset to defaults")
}

async fn cmd_path(out: &Output) -> Result<(), CliError> {
    let s = config::read().await;
    out.ok(serde_json::json!({
        "db": crate::ipc::storage::db::db_path(),
        "data_dir": crate::backends::paths::Paths::new()?.data_dir(),
        "down_dir": s.down_dir,
        "temp_dir": s.temp_dir,
    }))
}
