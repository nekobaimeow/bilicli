// SPDX-License-Identifier: GPL-3.0-or-later
// Queue persistence — ported from BiliTools `src-tauri/src/storage/queue.rs`.
// Stores an ordered list of task_ids per queue.

use crate::error::CliError;
use crate::ipc::storage::db;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum QueueKind {
    Downloading,
    Pending,
    Scheduled,
    Failed,
}

impl QueueKind {
    pub fn as_str(self) -> &'static str {
        match self {
            QueueKind::Downloading => "downloading",
            QueueKind::Pending => "pending",
            QueueKind::Scheduled => "scheduled",
            QueueKind::Failed => "failed",
        }
    }
    pub fn parse(s: &str) -> Result<Self, CliError> {
        Ok(match s {
            "downloading" => Self::Downloading,
            "pending" => Self::Pending,
            "scheduled" => Self::Scheduled,
            "failed" => Self::Failed,
            other => return Err(CliError::msg(format!("unknown queue kind: {other}"))),
        })
    }
}

pub async fn load() -> Result<Vec<(QueueKind, Vec<String>)>, CliError> {
    let pool = db::get_db().await?;
    let rows: Vec<(String, i64, String)> = sqlx::query_as(
        "SELECT queue, position, task_id FROM queue ORDER BY queue, position",
    )
    .fetch_all(&pool)
    .await?;
    let mut out: Vec<(QueueKind, Vec<String>)> = Vec::new();
    for (q, _, t) in rows {
        let kind = QueueKind::parse(&q)?;
        if let Some(last) = out.last_mut() {
            if last.0 == kind {
                last.1.push(t);
                continue;
            }
        }
        out.push((kind, vec![t]));
    }
    Ok(out)
}

pub async fn insert(kind: QueueKind, task_id: &str) -> Result<(), CliError> {
    let pool = db::get_db().await?;
    let next_pos: (i64,) = sqlx::query_as(
        "SELECT COALESCE(MAX(position), -1) + 1 FROM queue WHERE queue = ?",
    )
    .bind(kind.as_str())
    .fetch_one(&pool)
    .await?;
    sqlx::query("INSERT INTO queue (queue, position, task_id) VALUES (?, ?, ?)")
        .bind(kind.as_str())
        .bind(next_pos.0)
        .bind(task_id)
        .execute(&pool)
        .await?;
    Ok(())
}

pub async fn remove(kind: QueueKind, task_id: &str) -> Result<(), CliError> {
    let pool = db::get_db().await?;
    sqlx::query("DELETE FROM queue WHERE queue = ? AND task_id = ?")
        .bind(kind.as_str())
        .bind(task_id)
        .execute(&pool)
        .await?;
    Ok(())
}

pub async fn clear(kind: QueueKind) -> Result<(), CliError> {
    let pool = db::get_db().await?;
    sqlx::query("DELETE FROM queue WHERE queue = ?")
        .bind(kind.as_str())
        .execute(&pool)
        .await?;
    Ok(())
}

pub async fn contains(kind: QueueKind, task_id: &str) -> Result<bool, CliError> {
    let pool = db::get_db().await?;
    let row: Option<(i64,)> = sqlx::query_as(
        "SELECT 1 FROM queue WHERE queue = ? AND task_id = ?",
    )
    .bind(kind.as_str())
    .bind(task_id)
    .fetch_optional(&pool)
    .await?;
    Ok(row.is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn fresh() {
        let tmp = std::env::temp_dir().join(format!(
            "bilitools-cli-queue-{}",
            uuid::Uuid::new_v4()
        ));
        db::set_data_dir(Some(tmp.clone())).unwrap();
        db::close_db().await.ok();
        db::init().await.unwrap();
    }

    #[tokio::test]
    async fn insert_and_load() {
        fresh().await;
        insert(QueueKind::Pending, "t1").await.unwrap();
        insert(QueueKind::Pending, "t2").await.unwrap();
        let q = load().await.unwrap();
        let pending = q.iter().find(|(k, _)| *k == QueueKind::Pending).unwrap();
        assert_eq!(pending.1, vec!["t1".to_string(), "t2".to_string()]);
    }

    #[tokio::test]
    async fn remove_works() {
        fresh().await;
        insert(QueueKind::Failed, "x").await.unwrap();
        remove(QueueKind::Failed, "x").await.unwrap();
        assert!(!contains(QueueKind::Failed, "x").await.unwrap());
    }

    #[tokio::test]
    async fn parse_unknown_errors() {
        assert!(QueueKind::parse("garbage").is_err());
    }
}
