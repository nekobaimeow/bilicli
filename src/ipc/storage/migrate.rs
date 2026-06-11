// SPDX-License-Identifier: GPL-3.0-or-later
// Schema migration — ported from BiliTools `src-tauri/src/storage/migrate.rs`.
// The schema is identical to the GUI version so the CLI can interoperate
// with the same `Storage/storage.db` file.

use crate::error::CliError;
use sqlx::SqlitePool;

const SCHEMA_VERSION: i32 = 1;

const TABLES: &[&str] = &[
    // settings — JSON blob per key
    "CREATE TABLE IF NOT EXISTS settings (
        key TEXT PRIMARY KEY,
        value TEXT NOT NULL,
        updated_at INTEGER NOT NULL DEFAULT (strftime('%s','now'))
    )",
    // cookies — name → value
    "CREATE TABLE IF NOT EXISTS cookies (
        name TEXT PRIMARY KEY,
        value TEXT NOT NULL,
        updated_at INTEGER NOT NULL DEFAULT (strftime('%s','now'))
    )",
    // tasks — submitted download tasks
    "CREATE TABLE IF NOT EXISTS tasks (
        id TEXT PRIMARY KEY,
        type TEXT NOT NULL,
        source TEXT NOT NULL,
        options TEXT NOT NULL DEFAULT '{}',
        status TEXT NOT NULL DEFAULT 'pending',
        progress REAL NOT NULL DEFAULT 0,
        error TEXT,
        created_at INTEGER NOT NULL DEFAULT (strftime('%s','now')),
        updated_at INTEGER NOT NULL DEFAULT (strftime('%s','now')),
        completed_at INTEGER
    )",
    // subtasks — individual files within a task
    "CREATE TABLE IF NOT EXISTS subtasks (
        id TEXT PRIMARY KEY,
        task_id TEXT NOT NULL,
        type TEXT NOT NULL,
        url TEXT NOT NULL,
        filename TEXT,
        size INTEGER,
        downloaded INTEGER NOT NULL DEFAULT 0,
        status TEXT NOT NULL DEFAULT 'pending',
        error TEXT,
        started_at INTEGER,
        completed_at INTEGER,
        FOREIGN KEY(task_id) REFERENCES tasks(id) ON DELETE CASCADE
    )",
    // queue — ordered list of task IDs by queue type
    "CREATE TABLE IF NOT EXISTS queue (
        queue TEXT NOT NULL,
        position INTEGER NOT NULL,
        task_id TEXT NOT NULL,
        FOREIGN KEY(task_id) REFERENCES tasks(id) ON DELETE CASCADE,
        PRIMARY KEY(queue, position)
    )",
    // schedulers — cron-scheduled tasks
    "CREATE TABLE IF NOT EXISTS schedulers (
        id TEXT PRIMARY KEY,
        cron TEXT NOT NULL,
        source TEXT NOT NULL,
        options TEXT NOT NULL DEFAULT '{}',
        enabled INTEGER NOT NULL DEFAULT 1,
        last_run INTEGER,
        next_run INTEGER,
        created_at INTEGER NOT NULL DEFAULT (strftime('%s','now'))
    )",
    // task_events — log of progress/error events for tasks
    "CREATE TABLE IF NOT EXISTS task_events (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        task_id TEXT NOT NULL,
        kind TEXT NOT NULL,
        message TEXT NOT NULL,
        at INTEGER NOT NULL DEFAULT (strftime('%s','now')),
        FOREIGN KEY(task_id) REFERENCES tasks(id) ON DELETE CASCADE
    )",
];

/// Run all migrations. Idempotent.
pub async fn run(pool: &SqlitePool) -> Result<(), CliError> {
    for sql in TABLES {
        sqlx::query(sql).execute(pool).await?;
    }
    // Stamp the schema version in a side table so future migrations
    // can branch.
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS schema_version (
            version INTEGER PRIMARY KEY
        )",
    )
    .execute(pool)
    .await?;
    sqlx::query("INSERT OR IGNORE INTO schema_version (version) VALUES (?)")
        .bind(SCHEMA_VERSION)
        .execute(pool)
        .await?;
    tracing::info!("schema migration v{} applied", SCHEMA_VERSION);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn run_creates_all_tables() {
        let tmp = tempdir().unwrap();
        let url = format!("sqlite://{}?mode=rwc", tmp.path().join("test.db").to_string_lossy());
        let pool = sqlx::SqlitePool::connect(&url).await.unwrap();
        run(&pool).await.unwrap();

        let tables: Vec<(String,)> =
            sqlx::query_as("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
                .fetch_all(&pool)
                .await
                .unwrap();
        let names: Vec<String> = tables.into_iter().map(|(n,)| n).collect();
        for t in [
            "cookies",
            "tasks",
            "subtasks",
            "queue",
            "schedulers",
            "settings",
            "task_events",
            "schema_version",
        ] {
            assert!(names.contains(&t.to_string()), "missing table {t}");
        }
    }

    #[tokio::test]
    async fn run_is_idempotent() {
        let tmp = tempdir().unwrap();
        let url = format!("sqlite://{}?mode=rwc", tmp.path().join("test.db").to_string_lossy());
        let pool = sqlx::SqlitePool::connect(&url).await.unwrap();
        run(&pool).await.unwrap();
        run(&pool).await.unwrap(); // second time — should not error
    }
}
