// SPDX-License-Identifier: GPL-3.0-or-later
//! Adapters that replace Tauri / tauri-plugin-* in the CLI port.
//!
//! In BiliTools' original Rust code, the Tauri framework owns:
//!   * paths (`tauri::Manager::path`)
//!   * HTTP client (`tauri_plugin_http::reqwest`)
//!   * shell sidecars (`tauri_plugin_shell::ShellExt::sidecar`)
//!   * opener (`tauri_plugin_opener`)
//!   * logging (`tauri_plugin_log`)
//!
//! The CLI port replaces each with a small pure-Rust module so the
//! business logic can run without the Tauri runtime.

pub mod http;
pub mod paths;
pub mod sidecar;

pub use http::open_path;
