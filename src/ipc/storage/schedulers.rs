// SPDX-License-Identifier: GPL-3.0-or-later
// Schedulers persistence — ported from BiliTools `src-tauri/src/storage/schedulers.rs`.

use crate::error::CliError;
use crate::ipc::storage::db;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Scheduler {
    pub id: String,
    pub cron: String,
    pub source: String,
    pub options: JsonValue,
    pub enabled: bool,
    pub last_run: Option<i64>,
    pub next_run: Option<i64>,
    pub created_at: i64,
}

pub async fn load() -> Result<Vec<Scheduler>, CliError> {
    let pool = db::get_db().await?;
    let rows: Vec<(
        String, String, String, String, i64, Option<i64>, Option<i64>, i64,
    )> = sqlx::query_as(
        "SELECT id, cron, source, options, enabled, last_run, next_run, created_at \
         FROM schedulers ORDER BY created_at DESC",
    )
    .fetch_all(&pool)
    .await?;
    rows.into_iter()
        .map(|(id, cron, source, options, enabled, last, next, created)| {
            Ok(Scheduler {
                id,
                cron,
                source,
                options: serde_json::from_str(&options)
                    .unwrap_or(JsonValue::Object(Default::default())),
                enabled: enabled != 0,
                last_run: last,
                next_run: next,
                created_at: created,
            })
        })
        .collect()
}

pub async fn get(id: &str) -> Result<Option<Scheduler>, CliError> {
    let pool = db::get_db().await?;
    let row: Option<(
        String, String, String, String, i64, Option<i64>, Option<i64>, i64,
    )> = sqlx::query_as(
        "SELECT id, cron, source, options, enabled, last_run, next_run, created_at \
         FROM schedulers WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(&pool)
    .await?;
    match row {
        None => Ok(None),
        Some((id, cron, source, options, enabled, last, next, created)) => Ok(Some(Scheduler {
            id,
            cron,
            source,
            options: serde_json::from_str(&options)
                .unwrap_or(JsonValue::Object(Default::default())),
            enabled: enabled != 0,
            last_run: last,
            next_run: next,
            created_at: created,
        })),
    }
}

pub async fn insert(s: &Scheduler) -> Result<(), CliError> {
    let pool = db::get_db().await?;
    let opts = serde_json::to_string(&s.options)?;
    sqlx::query(
        "INSERT INTO schedulers (id, cron, source, options, enabled, last_run, next_run, created_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&s.id)
    .bind(&s.cron)
    .bind(&s.source)
    .bind(opts)
    .bind(if s.enabled { 1 } else { 0 })
    .bind(s.last_run)
    .bind(s.next_run)
    .bind(s.created_at)
    .execute(&pool)
    .await?;
    Ok(())
}

pub async fn remove(id: &str) -> Result<(), CliError> {
    let pool = db::get_db().await?;
    sqlx::query("DELETE FROM schedulers WHERE id = ?")
        .bind(id)
        .execute(&pool)
        .await?;
    Ok(())
}

pub async fn update(s: &Scheduler) -> Result<(), CliError> {
    let pool = db::get_db().await?;
    let opts = serde_json::to_string(&s.options)?;
    sqlx::query(
        "UPDATE schedulers SET cron=?, source=?, options=?, enabled=?, last_run=?, next_run=? WHERE id=?",
    )
    .bind(&s.cron)
    .bind(&s.source)
    .bind(opts)
    .bind(if s.enabled { 1 } else { 0 })
    .bind(s.last_run)
    .bind(s.next_run)
    .bind(&s.id)
    .execute(&pool)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn fresh() {
        let tmp = std::env::temp_dir().join(format!(
            "bilitools-cli-sched-{}",
            uuid::Uuid::new_v4()
        ));
        db::set_data_dir(Some(tmp.clone())).unwrap();
        db::close_db().await.ok();
        db::init().await.unwrap();
    }

    fn sample(id: &str) -> Scheduler {
        Scheduler {
            id: id.into(),
            cron: "0 0 * * * *".into(),
            source: "https://example.com".into(),
            options: serde_json::json!({"q": "1080p"}),
            enabled: true,
            last_run: None,
            next_run: None,
            created_at: 1_700_000_000,
        }
    }

    #[tokio::test]
    async fn insert_get_remove() {
        fresh().await;
        let s = sample("s1");
        insert(&s).await.unwrap();
        let back = get("s1").await.unwrap().unwrap();
        assert_eq!(back.cron, "0 0 * * * *");
        assert!(back.enabled);
        remove("s1").await.unwrap();
        assert!(get("s1").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn update_toggles_enabled() {
        fresh().await;
        let mut s = sample("s2");
        insert(&s).await.unwrap();
        s.enabled = false;
        s.last_run = Some(42);
        update(&s).await.unwrap();
        let back = get("s2").await.unwrap().unwrap();
        assert!(!back.enabled);
        assert_eq!(back.last_run, Some(42));
    }
}
