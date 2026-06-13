// SPDX-License-Identifier: GPL-3.0-or-later
// Cookie storage — ported from BiliTools `src-tauri/src/storage/cookies.rs`.
// Behavior: load/insert/delete cookies from SQLite `cookies` table.

use crate::error::CliError;
use crate::ipc::storage::db::get_db;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CookieEntry {
    pub name: String,
    pub value: String,
    pub expires: Option<i64>,
}

/// Load all cookies as a HashMap<name, value>.
/// Mirrors BiliTools' `cookies::load`.
pub async fn load() -> Result<HashMap<String, String>, CliError> {
    let pool = get_db().await?;
    let rows: Vec<(String, String)> = sqlx::query_as("SELECT name, value FROM cookies")
        .fetch_all(&pool)
        .await?;
    Ok(rows.into_iter().collect())
}

/// Insert (or update) a cookie. Accepts strings of the form `name=value`.
pub async fn insert(cookie: &str) -> Result<(), CliError> {
    let (name, value) = match cookie.split_once('=') {
        Some((n, v)) => (n.trim().to_string(), v.trim().to_string()),
        None => {
            // Delete-only path
            return delete(cookie.trim()).await;
        }
    };
    if name.is_empty() {
        return Ok(());
    }
    let pool = get_db().await?;
    sqlx::query(
        "INSERT INTO cookies (name, value) VALUES (?, ?) \
         ON CONFLICT(name) DO UPDATE SET value = excluded.value",
    )
    .bind(&name)
    .bind(&value)
    .execute(&pool)
    .await?;
    Ok(())
}

/// Delete a cookie by name.
pub async fn delete(name: &str) -> Result<(), CliError> {
    let pool = get_db().await?;
    sqlx::query("DELETE FROM cookies WHERE name = ?")
        .bind(name)
        .execute(&pool)
        .await?;
    Ok(())
}

/// Clear all cookies.
pub async fn clear() -> Result<(), CliError> {
    let pool = get_db().await?;
    sqlx::query("DELETE FROM cookies").execute(&pool).await?;
    Ok(())
}

/// Get a single cookie value.
pub async fn get(name: &str) -> Result<Option<String>, CliError> {
    let pool = get_db().await?;
    let row: Option<(String,)> = sqlx::query_as("SELECT value FROM cookies WHERE name = ?")
        .bind(name)
        .fetch_optional(&pool)
        .await?;
    Ok(row.map(|(v,)| v))
}

/// Check whether a specific cookie is present.
pub async fn has(name: &str) -> Result<bool, CliError> {
    Ok(get(name).await?.is_some())
}

/// Return all cookies with names (no values) — useful for status checks.
pub async fn names() -> Result<Vec<String>, CliError> {
    let pool = get_db().await?;
    let rows: Vec<(String,)> = sqlx::query_as("SELECT name FROM cookies")
        .fetch_all(&pool)
        .await?;
    Ok(rows.into_iter().map(|(n,)| n).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::storage::db;

    /// Helper to create a fresh in-memory DB.
    async fn fresh_db() {
        let tmp = std::env::temp_dir().join(format!(
            "bilicli-cli-test-{}",
            uuid::Uuid::new_v4()
        ));
        db::set_data_dir(Some(tmp)).unwrap();
        db::close_db().await.ok();
        db::init().await.expect("init db");
    }

    #[tokio::test]
    async fn insert_then_load() {
        fresh_db().await;
        insert("SESSID=abc123").await.unwrap();
        let cookies = load().await.unwrap();
        assert_eq!(cookies.get("SESSID"), Some(&"abc123".to_string()));
    }

    #[tokio::test]
    async fn insert_overwrites() {
        fresh_db().await;
        insert("k=v1").await.unwrap();
        insert("k=v2").await.unwrap();
        let c = load().await.unwrap();
        assert_eq!(c.get("k"), Some(&"v2".to_string()));
    }

    #[tokio::test]
    async fn delete_removes() {
        fresh_db().await;
        insert("gone=x").await.unwrap();
        delete("gone").await.unwrap();
        assert!(!has("gone").await.unwrap());
    }

    #[tokio::test]
    async fn clear_removes_all() {
        fresh_db().await;
        insert("a=1").await.unwrap();
        insert("b=2").await.unwrap();
        clear().await.unwrap();
        let n = names().await.unwrap();
        assert!(n.is_empty(), "expected no cookies, got {n:?}");
    }

    #[tokio::test]
    async fn names_returns_sorted_insertion_order_or_alphabetical() {
        // SQL has no defined order without ORDER BY; we just check the set.
        fresh_db().await;
        insert("buvid3=1").await.unwrap();
        insert("buvid4=2").await.unwrap();
        let n = names().await.unwrap();
        let mut expected = vec!["buvid3".to_string(), "buvid4".to_string()];
        expected.sort();
        let mut got = n.clone();
        got.sort();
        assert_eq!(got, expected);
    }
}
