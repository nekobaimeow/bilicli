// SPDX-License-Identifier: GPL-3.0-or-later
// Configuration — ported from BiliTools `src-tauri/src/storage/config.rs`,
// with GUI-only fields removed (theme, window_effect, clipboard, notify,
// drag_search, auto_check_update).
//
// Storage: the `Settings` struct is JSON-serialized and stored in the
// SQLite `settings` table under a single `settings` key, matching
// BiliTools' storage layout. The CLI does NOT need a separate TOML file
// for these — the GUI reads the same row.

use crate::backends::paths::Paths;
use crate::error::CliError;
use crate::ipc::storage::db;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

const SETTINGS_KEY: &str = "settings";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Settings {
    pub add_metadata: bool,
    pub auto_download: bool,
    pub block_pcdn: bool,
    pub convert: SettingsConvert,
    pub default: SettingsDefault,
    pub down_dir: PathBuf,
    pub format: SettingsFormat,
    pub language: String,
    pub max_conc: usize,
    pub temp_dir: PathBuf,
    pub organize: SettingsOrganize,
    pub proxy: SettingsProxy,
    pub sidecar: SettingsSidecar,
    pub speed_limit: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SettingsConvert {
    pub danmaku: bool,
    pub mp4: bool,
    pub mp3: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SettingsDefault {
    pub res: u32,
    pub abr: u32,
    pub enc: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SettingsFormat {
    pub pattern: String,
    pub time_format: String,
    pub time_zone: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SettingsOrganize {
    pub auto_rename: bool,
    pub top_folder: bool,
    pub sub_folder: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SettingsProxy {
    pub address: String,
    pub username: String,
    pub password: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SettingsSidecar {
    pub aria2c: PathBuf,
    pub ffmpeg: PathBuf,
    pub danmakufactory: PathBuf,
}

impl SettingsSidecar {
    pub fn empty() -> Self {
        Self {
            aria2c: PathBuf::new(),
            ffmpeg: PathBuf::new(),
            danmakufactory: PathBuf::new(),
        }
    }
}

impl Settings {
    /// Returns the default settings. Lazy — does not touch the database.
    pub fn defaults() -> Self {
        let paths = Paths::new().ok();
        let data_dir = paths
            .as_ref()
            .map(|p| p.data_dir())
            .unwrap_or_else(|| std::env::temp_dir().join("bilitools"));
        let temp = std::env::temp_dir().join("bilitools");
        let down = paths
            .as_ref()
            .map(|p| p.default_download_dir())
            .unwrap_or_else(|| data_dir.join("downloads"));
        Self {
            add_metadata: true,
            auto_download: false,
            block_pcdn: true,
            convert: SettingsConvert {
                danmaku: true,
                mp4: false,
                mp3: false,
            },
            default: SettingsDefault {
                res: 80,
                abr: 30280,
                enc: 7,
            },
            down_dir: down,
            format: SettingsFormat {
                pattern: "{title}/{title}".to_string(),
                time_format: "%Y-%m-%d".to_string(),
                time_zone: "local".to_string(),
            },
            language: detect_locale(),
            max_conc: 3,
            temp_dir: temp,
            organize: SettingsOrganize {
                auto_rename: true,
                top_folder: true,
                sub_folder: true,
            },
            proxy: SettingsProxy {
                address: String::new(),
                username: String::new(),
                password: String::new(),
            },
            sidecar: SettingsSidecar::empty(),
            speed_limit: 0,
        }
    }
}

/// Detect a reasonable default language code.
fn detect_locale() -> String {
    if let Ok(lang) = std::env::var("LANG") {
        let lower = lang.to_lowercase();
        if lower.starts_with("zh") {
            if lower.contains("tw") || lower.contains("hk") {
                return "zh-HK".into();
            }
            return "zh-CN".into();
        }
        if lower.starts_with("en") {
            return "en-US".into();
        }
        if lower.starts_with("ja") {
            return "ja-JP".into();
        }
    }
    "en-US".into()
}

/// Read the current settings from the database. Returns defaults if no row.
pub async fn read() -> Settings {
    match try_read().await {
        Ok(s) => s,
        Err(e) => {
            tracing::debug!("settings read failed, returning defaults: {e}");
            Settings::defaults()
        }
    }
}

async fn try_read() -> Result<Settings, CliError> {
    let pool = db::get_db().await?;
    let row: Option<(String,)> = sqlx::query_as("SELECT value FROM settings WHERE key = ?")
        .bind(SETTINGS_KEY)
        .fetch_optional(&pool)
        .await?;
    match row {
        Some((s,)) => Ok(serde_json::from_str(&s)?),
        None => Ok(Settings::defaults()),
    }
}

/// Persist settings to the database.
pub async fn write(settings: &Settings) -> Result<(), CliError> {
    let pool = db::get_db().await?;
    let json = serde_json::to_string(settings)?;
    sqlx::query(
        "INSERT INTO settings (key, value, updated_at) VALUES (?, ?, strftime('%s','now')) \
         ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
    )
    .bind(SETTINGS_KEY)
    .bind(json)
    .execute(&pool)
    .await?;
    Ok(())
}

/// Read-modify-write. Convenient for `bilitools config set key value`.
pub async fn update<F>(f: F) -> Result<Settings, CliError>
where
    F: FnOnce(&mut Settings),
{
    let mut s = read().await;
    f(&mut s);
    write(&s).await?;
    Ok(s)
}

/// Get a single field by dotted path. Returns Err on unknown field.
pub async fn get(field: &str) -> Result<serde_json::Value, CliError> {
    let s = read().await;
    let json = serde_json::to_value(&s)?;
    let mut cur = &json;
    for part in field.split('.') {
        cur = cur
            .get(part)
            .ok_or_else(|| CliError::msg(format!("unknown config key: {field}")))?;
    }
    Ok(cur.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn fresh() {
        let tmp = std::env::temp_dir().join(format!(
            "bilitools-cli-cfg-{}",
            uuid::Uuid::new_v4()
        ));
        db::set_data_dir(Some(tmp.clone())).unwrap();
        db::close_db().await.ok();
        db::init().await.unwrap();
    }

    #[tokio::test]
    async fn defaults_have_sane_values() {
        let s = Settings::defaults();
        assert!(s.add_metadata);
        assert!(s.block_pcdn);
        assert!(s.organize.auto_rename);
        assert!(!s.proxy.address.is_empty() == false);
        assert!(s.max_conc >= 1);
    }

    #[tokio::test]
    async fn write_then_read_roundtrip() {
        fresh().await;
        let mut s = Settings::defaults();
        s.max_conc = 8;
        s.proxy.address = "http://127.0.0.1:7890".into();
        write(&s).await.unwrap();
        let back = read().await;
        assert_eq!(back.max_conc, 8);
        assert_eq!(back.proxy.address, "http://127.0.0.1:7890");
    }

    #[tokio::test]
    async fn update_modifies_field() {
        fresh().await;
        let s = update(|c| c.max_conc = 16).await.unwrap();
        assert_eq!(s.max_conc, 16);
        let back = read().await;
        assert_eq!(back.max_conc, 16);
    }

    #[tokio::test]
    async fn get_dotted_path() {
        fresh().await;
        let v = get("max_conc").await.unwrap();
        assert!(v.is_u64() || v.is_i64());
    }

    #[tokio::test]
    async fn get_unknown_key_errors() {
        fresh().await;
        let r = get("does_not_exist").await;
        assert!(r.is_err());
    }

    #[test]
    fn detect_locale_zh() {
        std::env::set_var("LANG", "zh_CN.UTF-8");
        assert_eq!(detect_locale(), "zh-CN");
        std::env::set_var("LANG", "zh_TW.UTF-8");
        assert_eq!(detect_locale(), "zh-HK");
        std::env::remove_var("LANG");
    }

    #[test]
    fn detect_locale_en() {
        std::env::set_var("LANG", "en_US.UTF-8");
        assert_eq!(detect_locale(), "en-US");
        std::env::remove_var("LANG");
    }
}
