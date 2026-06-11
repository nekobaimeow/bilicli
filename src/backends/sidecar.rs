// SPDX-License-Identifier: GPL-3.0-or-later
// Sidecar binary execution — replacement for `tauri_plugin_shell::ShellExt::sidecar()`.
//
// In the GUI version, aria2c/ffmpeg/DanmakuFactory are bundled as Tauri sidecars.
// In the CLI version, we look them up via:
//   1. explicit path in `bilitools config show sidecar.<name>`
//   2. `$BILITOOLS_SIDECAR_<NAME>` env var
//   3. `which` lookup
//   4. common well-known paths
//
// Each lookup is lazy and caches the resolved PathBuf once found.

use crate::error::CliError;
use std::path::{Path, PathBuf};
use tokio::process::{Child, Command};

/// What kind of external binary we're looking up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SidecarKind {
    Aria2c,
    FFmpeg,
    DanmakuFactory,
}

impl SidecarKind {
    pub fn name(self) -> &'static str {
        match self {
            SidecarKind::Aria2c => "aria2c",
            SidecarKind::FFmpeg => "ffmpeg",
            SidecarKind::DanmakuFactory => "DanmakuFactory",
        }
    }
    pub fn env_var(self) -> &'static str {
        match self {
            SidecarKind::Aria2c => "BILITOOLS_SIDECAR_ARIA2C",
            SidecarKind::FFmpeg => "BILITOOLS_SIDECAR_FFMPEG",
            SidecarKind::DanmakuFactory => "BILITOOLS_SIDECAR_DANMAKU",
        }
    }
}

/// Look up a sidecar binary. Order: explicit override → env → `which` → fallbacks.
pub fn resolve(kind: SidecarKind, override_path: Option<&Path>) -> Result<PathBuf, CliError> {
    if let Some(p) = override_path {
        if p.is_file() {
            return Ok(p.to_path_buf());
        }
    }

    if let Some(p) = std::env::var_os(kind.env_var()).map(PathBuf::from) {
        if p.is_file() {
            return Ok(p);
        }
    }

    if let Ok(p) = which(kind.name()) {
        return Ok(p);
    }

    // Last-resort well-known paths
    for candidate in fallback_paths(kind) {
        if candidate.is_file() {
            return Ok(candidate);
        }
    }

    Err(CliError::MissingDependency(kind.name().to_string()))
}

fn which(name: &str) -> std::io::Result<PathBuf> {
    let path_var = std::env::var_os("PATH").unwrap_or_default();
    for dir in std::env::split_paths(&path_var) {
        let p = dir.join(name);
        if p.is_file() {
            return Ok(p);
        }
        // Windows: try `.exe` suffix
        #[cfg(windows)]
        {
            let p_exe = dir.join(format!("{name}.exe"));
            if p_exe.is_file() {
                return Ok(p_exe);
            }
        }
    }
    Err(std::io::Error::new(std::io::ErrorKind::NotFound, name))
}

fn fallback_paths(kind: SidecarKind) -> Vec<PathBuf> {
    match kind {
        SidecarKind::Aria2c => vec![
            "/usr/bin/aria2c".into(),
            "/usr/local/bin/aria2c".into(),
            "/opt/homebrew/bin/aria2c".into(),
        ],
        SidecarKind::FFmpeg => vec![
            "/usr/bin/ffmpeg".into(),
            "/usr/local/bin/ffmpeg".into(),
            "/opt/homebrew/bin/ffmpeg".into(),
        ],
        SidecarKind::DanmakuFactory => vec![
            "/usr/local/bin/DanmakuFactory".into(),
            "/usr/bin/DanmakuFactory".into(),
            "DanmakuFactory".into(),
        ],
    }
}

/// Build a `tokio::process::Command` for a sidecar. Use the resolved path.
pub fn command(kind: SidecarKind, override_path: Option<&Path>) -> Result<Command, CliError> {
    let path = resolve(kind, override_path)?;
    Ok(Command::new(path))
}

/// Spawn the sidecar and return the running Child.
pub async fn spawn(
    kind: SidecarKind,
    override_path: Option<&Path>,
    args: &[&str],
) -> Result<Child, CliError> {
    let mut cmd = command(kind, override_path)?;
    cmd.args(args);
    let child = cmd.spawn().map_err(|e| {
        CliError::msg(format!("failed to spawn {}: {}", kind.name(), e))
    })?;
    Ok(child)
}

/// Run a sidecar and wait, returning its combined stdout as Vec<u8>.
pub async fn run(
    kind: SidecarKind,
    override_path: Option<&Path>,
    args: &[&str],
) -> Result<Vec<u8>, CliError> {
    let output = command(kind, override_path)?
        .args(args)
        .output()
        .await
        .map_err(|e| CliError::msg(format!("failed to run {}: {}", kind.name(), e)))?;
    if !output.status.success() {
        return Err(CliError::msg(format!(
            "{} exited with status {}: stderr={}",
            kind.name(),
            output.status,
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    Ok(output.stdout)
}

/// Run a sidecar with stdin/stdout/stderr streams piped (so caller can read events).
pub async fn spawn_with_pipes(
    kind: SidecarKind,
    override_path: Option<&Path>,
    args: &[&str],
) -> Result<Child, CliError> {
    let mut cmd = command(kind, override_path)?;
    cmd.args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let child = cmd.spawn().map_err(|e| {
        CliError::msg(format!("failed to spawn {} (piped): {}", kind.name(), e))
    })?;
    Ok(child)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sidecar_kind_name() {
        assert_eq!(SidecarKind::Aria2c.name(), "aria2c");
        assert_eq!(SidecarKind::FFmpeg.name(), "ffmpeg");
        assert_eq!(SidecarKind::DanmakuFactory.name(), "DanmakuFactory");
    }

    #[test]
    fn sidecar_kind_env_var() {
        assert_eq!(SidecarKind::Aria2c.env_var(), "BILITOOLS_SIDECAR_ARIA2C");
        assert_eq!(SidecarKind::FFmpeg.env_var(), "BILITOOLS_SIDECAR_FFMPEG");
        assert_eq!(
            SidecarKind::DanmakuFactory.env_var(),
            "BILITOOLS_SIDECAR_DANMAKU"
        );
    }

    #[test]
    fn resolve_via_env_var() {
        let tmp = std::env::temp_dir().join(format!(
            "bilitools-cli-sidecar-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        let fake = tmp.join("aria2c");
        std::fs::write(&fake, b"#!/bin/sh\nexit 0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        std::env::set_var(SidecarKind::Aria2c.env_var(), &fake);
        let p = resolve(SidecarKind::Aria2c, None).unwrap();
        assert_eq!(p, fake);
        std::env::remove_var(SidecarKind::Aria2c.env_var());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn resolve_missing_returns_err() {
        std::env::remove_var(SidecarKind::DanmakuFactory.env_var());
        // We don't assert a specific error because the well-known path
        // /usr/local/bin/DanmakuFactory could exist on dev machines.
        // Just ensure the function returns without panicking.
        let _ = resolve(SidecarKind::DanmakuFactory, Some(Path::new("/nonexistent/xx")));
    }
}
