//! Shared types for the PTY module.

use std::collections::HashMap;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SpawnOptions {
    /// Shell executable override. If `None` we fall back to the OS default
    /// (powershell on Windows, bash on mac/linux).
    pub shell: Option<String>,
    /// Initial cwd. Defaults to the process cwd.
    pub cwd: Option<String>,
    /// Exact environment overrides (merged on top of the inherited env).
    pub env: Option<HashMap<String, String>>,
    /// Extra directories to prepend to `PATH` (before the inherited `PATH`).
    pub prepend_path: Option<Vec<String>>,
    pub cols: Option<u16>,
    pub rows: Option<u16>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SpawnResult {
    pub id: String,
    pub pid: u32,
    pub ws_url: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SessionInfo {
    pub id: String,
    pub pid: u32,
    pub shell: String,
    pub cwd: String,
    pub started_at_ms: u64,
    pub alive: bool,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ServerInfo {
    pub host: String,
    pub port: u16,
    pub token: String,
    pub url_prefix: String, // e.g. "ws://127.0.0.1:34127/_pty"
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ControlMsg {
    Resize { cols: u16, rows: u16 },
    Kill,
}
