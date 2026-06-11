// SPDX-License-Identifier: GPL-3.0-or-later
// bilitools-cli unified error type.
// Adapted from BiliTools `src-tauri/src/errors.rs` (TauriError).
// Differences:
//   - `TauriError::new` is replaced with a plain constructor.
//   - The `TauriError::Error` variant (Tauri error) is dropped.
//   - The HTTP `StatusCode` is wrapped as a string for serde-friendliness.

use std::path::PathBuf;

#[derive(thiserror::Error, Debug)]
pub enum CliError {
    #[error("config: {0}")]
    Config(String),

    #[error("auth: {0}")]
    Auth(#[from] AuthError),

    #[error("network: {0}")]
    Network(#[from] reqwest::Error),

    #[error("database: {0}")]
    Database(#[from] sqlx::Error),

    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("serialization: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("bilibili api error: code={code} {message}")]
    Api { code: i64, message: String },

    #[error("bilibili http error: status={status} {message}")]
    Http { status: u16, message: String },

    #[error("not logged in: {0}")]
    NotLoggedIn(String),

    #[error("invalid url: {0}")]
    InvalidUrl(String),

    #[error("dependency missing: {0} — install it or set it in `bilitools config set sidecar.<name>`")]
    MissingDependency(String),

    #[error("path not found: {0}")]
    PathNotFound(PathBuf),

    #[error("parse error: {0}")]
    Parse(String),

    #[error("task not found: {0}")]
    TaskNotFound(String),

    #[error("task cancelled")]
    Cancelled,

    #[error("io runtime: {0}")]
    Other(String),
}

impl CliError {
    pub fn msg(s: impl Into<String>) -> Self {
        CliError::Other(s.into())
    }

    pub fn api(code: i64, message: impl Into<String>) -> Self {
        CliError::Api {
            code,
            message: message.into(),
        }
    }

    pub fn http(status: u16, message: impl Into<String>) -> Self {
        CliError::Http {
            status,
            message: message.into(),
        }
    }

    /// Stable error code for --json output.
    pub fn code(&self) -> &'static str {
        match self {
            CliError::Config(_) => "CONFIG",
            CliError::Auth(_) => "AUTH",
            CliError::Network(_) => "NETWORK",
            CliError::Database(_) => "DATABASE",
            CliError::Io(_) => "IO",
            CliError::Serde(_) => "SERDE",
            CliError::Api { .. } => "API",
            CliError::Http { .. } => "HTTP",
            CliError::NotLoggedIn(_) => "NOT_LOGGED_IN",
            CliError::InvalidUrl(_) => "INVALID_URL",
            CliError::MissingDependency(_) => "MISSING_DEPENDENCY",
            CliError::PathNotFound(_) => "PATH_NOT_FOUND",
            CliError::Parse(_) => "PARSE",
            CliError::TaskNotFound(_) => "TASK_NOT_FOUND",
            CliError::Cancelled => "CANCELLED",
            CliError::Other(_) => "OTHER",
        }
    }
}

#[derive(thiserror::Error, Debug)]
pub enum AuthError {
    #[error("scan login failed: {0}")]
    Scan(String),

    #[error("cookie refresh failed: {0}")]
    Refresh(String),

    #[error("login cancelled")]
    Cancelled,

    #[error("login timeout after {0:?}")]
    Timeout(std::time::Duration),

    #[error("qrcode generation failed: {0}")]
    Qrcode(String),
}

pub type CliResult<T> = Result<T, CliError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_code_is_stable() {
        assert_eq!(CliError::Config("x".into()).code(), "CONFIG");
        assert_eq!(CliError::NotLoggedIn("x".into()).code(), "NOT_LOGGED_IN");
        assert_eq!(CliError::Cancelled.code(), "CANCELLED");
        assert_eq!(
            CliError::MissingDependency("ffmpeg".into()).code(),
            "MISSING_DEPENDENCY"
        );
    }

    #[test]
    fn auth_error_variants() {
        let e = AuthError::Scan("expired".into());
        assert!(format!("{e}").contains("expired"));
    }
}
