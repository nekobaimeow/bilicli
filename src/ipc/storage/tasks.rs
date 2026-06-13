// SPDX-License-Identifier: GPL-3.0-or-later
// Tasks persistence — ported from BiliTools `src-tauri/src/storage/tasks.rs`.

use crate::error::CliError;
use crate::ipc::storage::db;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskStatus {
    Pending,
    Running,
    Paused,
    Completed,
    Failed,
    Cancelled,
}

impl TaskStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            TaskStatus::Pending => "pending",
            TaskStatus::Running => "running",
            TaskStatus::Paused => "paused",
            TaskStatus::Completed => "completed",
            TaskStatus::Failed => "failed",
            TaskStatus::Cancelled => "cancelled",
        }
    }
    pub fn parse(s: &str) -> Result<Self, CliError> {
        Ok(match s {
            "pending" => Self::Pending,
            "running" => Self::Running,
            "paused" => Self::Paused,
            "completed" => Self::Completed,
            "failed" => Self::Failed,
            "cancelled" => Self::Cancelled,
            other => return Err(CliError::msg(format!("unknown task status: {other}"))),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskType {
    Video,
    Audio,
    AudioVideo,
    Bangumi,
    Favorite,
    WatchLater,
    Interactive,
    Music,
    Subtitle,
    Danmaku,
    Cover,
    Nfo,
    Thumbnail,
    Other,
}

impl TaskType {
    pub fn as_str(self) -> &'static str {
        match self {
            TaskType::Video => "video",
            TaskType::Audio => "audio",
            TaskType::AudioVideo => "audio_video",
            TaskType::Bangumi => "bangumi",
            TaskType::Favorite => "favorite",
            TaskType::WatchLater => "watch_later",
            TaskType::Interactive => "interactive",
            TaskType::Music => "music",
            TaskType::Subtitle => "subtitle",
            TaskType::Danmaku => "danmaku",
            TaskType::Cover => "cover",
            TaskType::Nfo => "nfo",
            TaskType::Thumbnail => "thumbnail",
            TaskType::Other => "other",
        }
    }
    pub fn parse(s: &str) -> Result<Self, CliError> {
        Ok(match s {
            "video" => Self::Video,
            "audio" => Self::Audio,
            "audio_video" => Self::AudioVideo,
            "bangumi" => Self::Bangumi,
            "favorite" => Self::Favorite,
            "watch_later" => Self::WatchLater,
            "interactive" => Self::Interactive,
            "music" => Self::Music,
            "subtitle" => Self::Subtitle,
            "danmaku" => Self::Danmaku,
            "cover" => Self::Cover,
            "nfo" => Self::Nfo,
            "thumbnail" => Self::Thumbnail,
            "other" => Self::Other,
            other => return Err(CliError::msg(format!("unknown task type: {other}"))),
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub task_type: TaskType,
    pub source: String,
    pub options: JsonValue,
    pub status: TaskStatus,
    pub progress: f32,
    pub error: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub completed_at: Option<i64>,
}

pub async fn load() -> Result<Vec<Task>, CliError> {
    let pool = db::get_db().await?;
    let rows: Vec<(
        String, String, String, String, String, f32, Option<String>, i64, i64, Option<i64>,
    )> = sqlx::query_as(
        "SELECT id, type, source, options, status, progress, error, created_at, updated_at, completed_at \
         FROM tasks ORDER BY created_at DESC",
    )
    .fetch_all(&pool)
    .await?;
    rows.into_iter()
        .map(|(id, t, s, o, st, p, e, c, u, comp)| {
            Ok(Task {
                id,
                task_type: TaskType::parse(&t)?,
                source: s,
                options: serde_json::from_str(&o).unwrap_or(JsonValue::Object(Default::default())),
                status: TaskStatus::parse(&st)?,
                progress: p,
                error: e,
                created_at: c,
                updated_at: u,
                completed_at: comp,
            })
        })
        .collect()
}

pub async fn get(id: &str) -> Result<Option<Task>, CliError> {
    let pool = db::get_db().await?;
    let row: Option<(
        String, String, String, String, String, f32, Option<String>, i64, i64, Option<i64>,
    )> = sqlx::query_as(
        "SELECT id, type, source, options, status, progress, error, created_at, updated_at, completed_at \
         FROM tasks WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(&pool)
    .await?;
    match row {
        None => Ok(None),
        Some((id, t, s, o, st, p, e, c, u, comp)) => Ok(Some(Task {
            id,
            task_type: TaskType::parse(&t)?,
            source: s,
            options: serde_json::from_str(&o).unwrap_or(JsonValue::Object(Default::default())),
            status: TaskStatus::parse(&st)?,
            progress: p,
            error: e,
            created_at: c,
            updated_at: u,
            completed_at: comp,
        })),
    }
}

pub async fn insert(task: &Task) -> Result<(), CliError> {
    let pool = db::get_db().await?;
    let opts = serde_json::to_string(&task.options)?;
    sqlx::query(
        "INSERT INTO tasks (id, type, source, options, status, progress, error, created_at, updated_at, completed_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&task.id)
    .bind(task.task_type.as_str())
    .bind(&task.source)
    .bind(opts)
    .bind(task.status.as_str())
    .bind(task.progress)
    .bind(&task.error)
    .bind(task.created_at)
    .bind(task.updated_at)
    .bind(task.completed_at)
    .execute(&pool)
    .await?;
    Ok(())
}

pub async fn update(task: &Task) -> Result<(), CliError> {
    let pool = db::get_db().await?;
    let opts = serde_json::to_string(&task.options)?;
    sqlx::query(
        "UPDATE tasks SET type=?, source=?, options=?, status=?, progress=?, error=?, updated_at=?, completed_at=? WHERE id=?",
    )
    .bind(task.task_type.as_str())
    .bind(&task.source)
    .bind(opts)
    .bind(task.status.as_str())
    .bind(task.progress)
    .bind(&task.error)
    .bind(crate::ipc::shared::get_sec())
    .bind(task.completed_at)
    .bind(&task.id)
    .execute(&pool)
    .await?;
    Ok(())
}

pub async fn remove(id: &str) -> Result<(), CliError> {
    let pool = db::get_db().await?;
    sqlx::query("DELETE FROM tasks WHERE id = ?")
        .bind(id)
        .execute(&pool)
        .await?;
    Ok(())
}

pub async fn log_event(task_id: &str, kind: &str, message: &str) -> Result<(), CliError> {
    let pool = db::get_db().await?;
    sqlx::query("INSERT INTO task_events (task_id, kind, message) VALUES (?, ?, ?)")
        .bind(task_id)
        .bind(kind)
        .bind(message)
        .execute(&pool)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn fresh() {
        let tmp = std::env::temp_dir().join(format!(
            "bilicli-cli-tasks-{}",
            uuid::Uuid::new_v4()
        ));
        db::set_data_dir(Some(tmp.clone())).unwrap();
        db::close_db().await.ok();
        db::init().await.unwrap();
    }

    fn sample_task(id: &str) -> Task {
        Task {
            id: id.into(),
            task_type: TaskType::Video,
            source: format!("https://example.com/{id}"),
            options: serde_json::json!({"quality": 80}),
            status: TaskStatus::Pending,
            progress: 0.0,
            error: None,
            created_at: 1_700_000_000,
            updated_at: 1_700_000_000,
            completed_at: None,
        }
    }

    #[tokio::test]
    async fn insert_get_update_remove() {
        fresh().await;
        let t = sample_task("abc");
        insert(&t).await.unwrap();
        let back = get("abc").await.unwrap().unwrap();
        assert_eq!(back.id, "abc");
        assert_eq!(back.task_type, TaskType::Video);
        assert_eq!(back.status, TaskStatus::Pending);

        let mut updated = back.clone();
        updated.status = TaskStatus::Running;
        updated.progress = 0.5;
        update(&updated).await.unwrap();
        let back2 = get("abc").await.unwrap().unwrap();
        assert_eq!(back2.status, TaskStatus::Running);
        assert!((back2.progress - 0.5).abs() < 1e-6);

        remove("abc").await.unwrap();
        assert!(get("abc").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn load_returns_all() {
        fresh().await;
        insert(&sample_task("a")).await.unwrap();
        insert(&sample_task("b")).await.unwrap();
        let all = load().await.unwrap();
        assert_eq!(all.len(), 2);
    }

    #[tokio::test]
    async fn log_event_inserts_row() {
        fresh().await;
        insert(&sample_task("ev")).await.unwrap();
        log_event("ev", "info", "started").await.unwrap();
        log_event("ev", "error", "boom").await.unwrap();
        let pool = db::get_db().await.unwrap();
        let n: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM task_events WHERE task_id='ev'")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n.0, 2);
    }

    #[test]
    fn task_type_roundtrip() {
        for t in [
            TaskType::Video,
            TaskType::Audio,
            TaskType::AudioVideo,
            TaskType::Bangumi,
            TaskType::Favorite,
            TaskType::WatchLater,
            TaskType::Interactive,
            TaskType::Music,
            TaskType::Subtitle,
            TaskType::Danmaku,
            TaskType::Cover,
            TaskType::Nfo,
            TaskType::Thumbnail,
            TaskType::Other,
        ] {
            assert_eq!(TaskType::parse(t.as_str()).unwrap(), t);
        }
    }

    #[test]
    fn task_status_roundtrip() {
        for s in [
            TaskStatus::Pending,
            TaskStatus::Running,
            TaskStatus::Paused,
            TaskStatus::Completed,
            TaskStatus::Failed,
            TaskStatus::Cancelled,
        ] {
            assert_eq!(TaskStatus::parse(s.as_str()).unwrap(), s);
        }
    }
}
