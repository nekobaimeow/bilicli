// SPDX-License-Identifier: GPL-3.0-or-later
//! Unified output layer — `--json` flag produces a stable JSON shape;
//! human mode produces pretty tables.

use crate::error::CliError;
use comfy_table::Table;
use serde::Serialize;
use std::io::Write;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputMode {
    Human,
    Json,
}

#[derive(Clone)]
pub struct Output {
    pub mode: OutputMode,
    pub writer: OutputWriter,
}

#[derive(Clone)]
pub enum OutputWriter {
    Stdout,
    Stderr,
    /// Write to a buffer; used by tests.
    Buffer(std::sync::Arc<std::sync::Mutex<Vec<u8>>>),
}

impl OutputWriter {
    pub fn write_all(&self, data: &[u8]) -> std::io::Result<()> {
        match self {
            OutputWriter::Stdout => std::io::stdout().write_all(data),
            OutputWriter::Stderr => std::io::stderr().write_all(data),
            OutputWriter::Buffer(buf) => buf.lock().unwrap().write_all(data),
        }
    }
    pub fn bytes(&self) -> Vec<u8> {
        match self {
            OutputWriter::Buffer(buf) => buf.lock().unwrap().clone(),
            _ => Vec::new(),
        }
    }
}

impl Output {
    pub fn new(mode: OutputMode) -> Self {
        Self {
            mode,
            writer: OutputWriter::Stdout,
        }
    }
    pub fn json() -> Self {
        Self::new(OutputMode::Json)
    }
    pub fn human() -> Self {
        Self::new(OutputMode::Human)
    }
    pub fn is_json(&self) -> bool {
        matches!(self.mode, OutputMode::Json)
    }
    /// Write `ok: true` with an optional data payload.
    pub fn ok<T: Serialize>(&self, data: T) -> Result<(), CliError> {
        match self.mode {
            OutputMode::Json => {
                let envelope = serde_json::json!({"ok": true, "data": data});
                self.writer
                    .write_all(serde_json::to_vec(&envelope)?.as_slice())
                    .map_err(CliError::from)?;
                self.writer.write_all(b"\n").map_err(CliError::from)
            }
            OutputMode::Human => {
                let s = serde_json::to_string_pretty(&data).unwrap_or_else(|_| "<unserializable>".into());
                self.writer.write_all(s.as_bytes()).map_err(CliError::from)?;
                self.writer.write_all(b"\n").map_err(CliError::from)
            }
        }
    }
    /// Write a bare "OK" / status line (no data).
    pub fn status(&self, msg: impl AsRef<str>) -> Result<(), CliError> {
        match self.mode {
            OutputMode::Json => {
                let envelope = serde_json::json!({"ok": true, "data": {"status": msg.as_ref()}});
                self.writer
                    .write_all(serde_json::to_vec(&envelope)?.as_slice())
                    .map_err(CliError::from)?;
                self.writer.write_all(b"\n").map_err(CliError::from)
            }
            OutputMode::Human => {
                let s: &str = msg.as_ref();
                self.writer.write_all(s.as_bytes()).map_err(CliError::from)?;
                self.writer.write_all(b"\n").map_err(CliError::from)
            }
        }
    }
    /// Write an error.
    pub fn err(&self, err: &CliError) -> Result<(), CliError> {
        match self.mode {
            OutputMode::Json => {
                let envelope = serde_json::json!({
                    "ok": false,
                    "error": {
                        "code": err.code(),
                        "message": format!("{err}"),
                    }
                });
                self.writer
                    .write_all(serde_json::to_vec(&envelope)?.as_slice())
                    .map_err(CliError::from)?;
                self.writer.write_all(b"\n").map_err(CliError::from)
            }
            OutputMode::Human => self
                .writer
                .write_all(format!("error[{}]: {}\n", err.code(), err).as_bytes())
                .map_err(CliError::from),
        }
    }

    /// Render an array of objects as a table (Human mode) or a JSON
    /// array (Json mode).
    pub fn list<T: Serialize>(&self, rows: Vec<T>) -> Result<(), CliError> {
        match self.mode {
            OutputMode::Json => self.ok(rows),
            OutputMode::Human => {
                if rows.is_empty() {
                    return self.writer.write_all(b"(empty)\n").map_err(CliError::from);
                }
                let first = serde_json::to_value(&rows[0]).map_err(CliError::from)?;
                if let Some(obj) = first.as_object() {
                    let mut table = Table::new();
                    table.set_header(obj.keys().cloned().collect::<Vec<_>>());
                    for r in &rows {
                        let v = serde_json::to_value(r).map_err(CliError::from)?;
                        if let Some(o) = v.as_object() {
                            let row: Vec<String> = obj
                                .keys()
                                .map(|k| {
                                    o.get(k)
                                        .map(|v| match v {
                                            serde_json::Value::String(s) => s.clone(),
                                            serde_json::Value::Null => String::new(),
                                            other => other.to_string(),
                                        })
                                        .unwrap_or_default()
                                })
                                .collect();
                            table.add_row(row);
                        }
                    }
                    self.writer
                        .write_all(table.to_string().as_bytes())
                        .map_err(CliError::from)?;
                    self.writer.write_all(b"\n").map_err(CliError::from)
                } else {
                    self.writer
                        .write_all(b"(non-object rows)\n")
                        .map_err(CliError::from)
                }
            }
        }
    }

    // Unused: comfy_table ContentRow trait import — left for future expansion.
    #[allow(dead_code)]
    fn _row_marker(_: comfy_table::Row) {}
}
