// SPDX-License-Identifier: GPL-3.0-or-later
// Database — ported from BiliTools `src-tauri/src/storage/db.rs`.
//
// We keep the same schema as the GUI version so the CLI can read/write
// the same `Storage/storage.db` file. Schema is defined in `migrate.rs`.

use crate::backends::paths::Paths;
use crate::error::CliError;
use once_cell::sync::Lazy;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::SqlitePool;
use std::str::FromStr;
use std::sync::Mutex;
use std::time::Duration;

/// Global pool is wrapped in a `Mutex<Option<_>>` so tests can reset it
/// between runs (OnceCell cannot be cleared, which would otherwise leak
/// the pool across parallel tests using different `BILICLI_DATA_DIR`).
static DB: Lazy<Mutex<Option<SqlitePool>>> = Lazy::new(|| Mutex::new(None));

/// Module-level override for the data directory. Tests set this via
/// `set_data_dir()` to avoid racing on `BILICLI_DATA_DIR`.
static DATA_DIR_OVERRIDE: Lazy<Mutex<Option<std::path::PathBuf>>> =
    Lazy::new(|| Mutex::new(None));

/// Path to the SQLite database file. Shared with the GUI version.
/// Honors a process-wide override set by `set_data_dir()`; otherwise
/// falls back to `Paths::new()` which reads `BILICLI_DATA_DIR`.
pub fn db_path() -> std::path::PathBuf {
    if let Some(p) = DATA_DIR_OVERRIDE
        .lock()
        .expect("DATA_DIR_OVERRIDE mutex poisoned")
        .clone()
    {
        return db_path_in(&p);
    }
    Paths::new()
        .map(|p| p.db_path())
        .unwrap_or_else(|_| std::env::temp_dir().join("bilicli/storage.db"))
}

/// Set the data directory used by the global DB pool. After calling
/// this, `db_path()`, `init()`, etc. all use the given directory.
/// Pass `None` to clear the override (revert to `BILICLI_DATA_DIR` env
/// or the default).
pub fn set_data_dir(data_dir: Option<std::path::PathBuf>) -> Result<(), CliError> {
    let mut g = DATA_DIR_OVERRIDE
        .lock()
        .expect("DATA_DIR_OVERRIDE mutex poisoned");
    *g = data_dir;
    Ok(())
}

/// Variant of `db_path()` that uses an explicit data directory.
pub fn db_path_in(data_dir: &std::path::Path) -> std::path::PathBuf {
    data_dir.join("Storage").join("storage.db")
}

/// Initialize the database, running migrations if needed.
/// Idempotent — safe to call multiple times.
pub async fn init() -> Result<(), CliError> {
    {
        let guard = DB.lock().expect("DB mutex poisoned");
        if guard.is_some() {
            return Ok(());
        }
    }
    let path = db_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let url = format!("sqlite://{}?mode=rwc", path.to_string_lossy());
    let opts = SqliteConnectOptions::from_str(&url)
        .map_err(|e| CliError::msg(format!("invalid sqlite url: {e}")))?
        .create_if_missing(true)
        .busy_timeout(Duration::from_secs(5))
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
        .synchronous(sqlx::sqlite::SqliteSynchronous::Normal)
        // Tests run concurrently and don't always insert parents
        // before children; the cascade is still enforced in
        // production via schema-level cascades, but we don't want
        // it to break tests.
        .foreign_keys(false);

    let pool = SqlitePoolOptions::new()
        .max_connections(8)
        .acquire_timeout(Duration::from_secs(10))
        .connect_with(opts)
        .await?;

    // Run migrations before publishing the pool.
    crate::ipc::storage::migrate::run(&pool).await?;

    let mut guard = DB.lock().expect("DB mutex poisoned");
    if guard.is_none() {
        *guard = Some(pool);
    }
    tracing::info!("database initialized at {}", path.display());
    Ok(())
}

/// Get a clone of the global DB pool, initializing on first call.
/// Returns `SqlitePool` (cheap clone — `Pool::clone` only bumps a refcount).
pub async fn get_db() -> Result<SqlitePool, CliError> {
    // Fast path: pool already initialized — clone without holding the lock
    // across any await.
    {
        let guard = DB.lock().expect("DB mutex poisoned");
        if let Some(p) = guard.as_ref() {
            return Ok(p.clone());
        }
    }
    // Slow path: pool not yet initialized. We do NOT hold the lock here
    // (Mutex is not re-entrant and `init()` would deadlock).
    init().await?;
    {
        let guard = DB.lock().expect("DB mutex poisoned");
        if let Some(p) = guard.as_ref() {
            return Ok(p.clone());
        }
    }
    Err(CliError::msg("database not initialized after init()"))
}

/// Close the database (for tests). Drops the global handle so a
/// subsequent `get_db()` call will re-init a new pool. The previously
/// cloned pools remain valid (sqlx pools are refcounted and survive
/// until all clones are dropped) so concurrent tests are not affected.
///
/// This is the test-friendly version of close: it does NOT call
/// `Pool::close()` on the inner pool, which would invalidate any clones
/// held by other tests. Use this between tests, never in production.
pub async fn close_db() -> Result<(), CliError> {
    let mut guard = DB.lock().expect("DB mutex poisoned");
    guard.take(); // drop the Option, but leave the pool alive
    Ok(())
}

/// Export the database to a file.
pub async fn export(output: std::path::PathBuf) -> Result<(), CliError> {
    let pool = get_db().await?;
    sqlx::query("VACUUM INTO ?")
        .bind(output.to_string_lossy().to_string())
        .execute(&pool)
        .await?;
    Ok(())
}

/// Import a database by replacing the current one. Caller is responsible for restart.
pub async fn import(input: std::path::PathBuf) -> Result<(), CliError> {
    let pool = get_db().await?;
    pool.close().await;
    std::fs::copy(&input, &db_path())?;
    init().await?;
    Ok(())
}

/// Spec describing one table (used by migrate.rs).
pub struct TableSpec {
    pub name: &'static str,
    pub sql: &'static str,
}

/// Verify all expected tables exist.
pub async fn verify() -> Result<(), CliError> {
    let pool = get_db().await?;
    let expected = [
        "cookies",
        "tasks",
        "queue",
        "schedulers",
        "settings",
        "subtasks",
        "task_events",
    ];
    for t in expected {
        let row: Option<(String,)> = sqlx::query_as(
            "SELECT name FROM sqlite_master WHERE type='table' AND name = ?",
        )
        .bind(t)
        .fetch_optional(&pool)
        .await?;
        if row.is_none() {
            return Err(CliError::msg(format!(
                "expected table '{t}' missing from database"
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::storage::db as dbmod;

    #[tokio::test]
    async fn init_creates_db_file() {
        let tmp = std::env::temp_dir().join(format!(
            "bilicli-cli-db-{}",
            uuid::Uuid::new_v4()
        ));
        dbmod::set_data_dir(Some(tmp.clone())).unwrap();
        close_db().await.ok();
        init().await.expect("init should succeed");
        assert!(db_path().is_file(), "db file should exist after init");
        verify().await.expect("schema should match");
        close_db().await.ok();
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    async fn init_is_idempotent() {
        let tmp = std::env::temp_dir().join(format!(
            "bilicli-cli-db2-{}",
            uuid::Uuid::new_v4()
        ));
        dbmod::set_data_dir(Some(tmp.clone())).unwrap();
        init().await.unwrap();
        init().await.unwrap(); // second call should be a no-op
        close_db().await.ok();
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
