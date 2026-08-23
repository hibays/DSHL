//! Embedded PTY / browser-terminal napi wrappers.
//!
//! Thin napi shims over `dshl_core::pty`. The PTY server itself runs inside
//! `dshl-core` (portable-pty + tokio-tungstenite) on a random port + random
//! token; this module only lifts the spawn/list/kill/resize/write/query
//! surface to JS so the in-page xterm.js can talk to it.

use napi_derive::napi;

use crate::types::{
    TerminalServerInfo, TerminalSessionInfo, TerminalSpawnOptions, TerminalSpawnResult,
};

#[napi]
pub fn terminal_spawn(opts: TerminalSpawnOptions) -> napi::Result<TerminalSpawnResult> {
    let o = dshl_core::pty::SpawnOptions {
        shell: opts.shell,
        cwd: opts.cwd,
        env: opts.env,
        prepend_path: opts.prepend_path,
        cols: opts.cols,
        rows: opts.rows,
    };
    match dshl_core::pty::spawn(o) {
        Ok(r) => Ok(TerminalSpawnResult {
            id: r.id,
            pid: r.pid as i64,
            ws_url: r.ws_url,
        }),
        Err(e) => Err(napi::Error::from_reason(format!("terminal_spawn: {e}"))),
    }
}

#[napi]
pub fn terminal_list() -> Vec<TerminalSessionInfo> {
    dshl_core::pty::list()
        .into_iter()
        .map(|s| TerminalSessionInfo {
            id: s.id,
            pid: s.pid as i64,
            shell: s.shell,
            cwd: s.cwd,
            started_at_ms: s.started_at_ms as i64,
            alive: s.alive,
        })
        .collect()
}

#[napi]
pub fn terminal_kill(id: String) -> bool {
    dshl_core::pty::kill(&id)
}

#[napi]
pub fn terminal_resize(id: String, cols: u16, rows: u16) -> bool {
    dshl_core::pty::resize(&id, cols, rows)
}

#[napi]
pub fn terminal_write(id: String, data: String) -> bool {
    dshl_core::pty::write(&id, data.as_bytes())
}

#[napi]
pub fn terminal_ws_endpoint() -> Option<TerminalServerInfo> {
    dshl_core::pty::server_endpoint().map(|s| TerminalServerInfo {
        host: s.host,
        port: s.port as i64,
        token: s.token,
        url_prefix: s.url_prefix,
    })
}
