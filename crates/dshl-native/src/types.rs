//! napi struct definitions shared by every [`crate`] module.
//!
//! napi-derive generates its own TS type per `#[napi(object)]` struct, so we
//! cannot re-export `dshl_core` / `dshl_cli` types directly — we mirror them
//! here (kept structurally identical so the JS-side `.d.ts` stays stable
//! across kernel refactors).

use napi_derive::napi;

/// Full options passed to `launch()`. Most fields match `RunOptions`; the
/// defaults (all-null) mirror the installer binary's defaults so a JS caller
/// can just write `launch({})` for the same behaviour as double-clicking the
/// installed dshl.
#[napi(object)]
pub struct LaunchOptions {
    /// Optional absolute path to a dshl.toml. When omitted the kernel
    /// resolves config through the standard <config>/dshl.toml search path
    /// (same as the installer binary).
    pub config: Option<String>,
    /// Print runtime logs to stderr (equivalent to `dshl -d`).
    pub debug: Option<bool>,
    /// Honour [ui] single-instance in dshl.toml. Defaults to TRUE so the
    /// plugin matches the installer's behaviour; set to FALSE when Node owns
    /// single-instance guarantees above this addon.
    pub enable_single_instance: Option<bool>,
    /// Start the legacy HTTP control plane (sets DSHL_CONTROL_URL for dsh
    /// children started by the kernel). Defaults to FALSE: the napi surface
    /// itself is the control plane; enable it for back-compat with older
    /// dshl-aware tooling that expects the WS endpoint.
    pub enable_control_pipe: Option<bool>,
    /// Install the OS signal handler (Ctrl+C / SIGTERM → shutdown). Defaults
    /// to FALSE so Node remains the owner of signal handling; set TRUE for
    /// standalone-kernel behaviour parity with the installer binary.
    pub install_signal_handler: Option<bool>,
}

/// Options for `openTerminal` — shared with the desktop contract so JS side
/// can feed the dsh runtime PATH directly.
#[napi(object)]
pub struct OpenTerminalOptions {
    pub cwd: String,
    pub path: Option<String>,
}

/// Options for `restart` when no kernel is running — we spawn a detached
/// copy of the hosting dsh process. Shape is identical to the legacy mirror
/// version for backwards compatibility with JS callers that use it directly.
#[napi(object)]
pub struct RequestRestartOptions {
    pub cmd: String,
    #[napi(ts_type = "string[]")]
    pub args: Vec<String>,
    pub cwd: Option<String>,
    pub path: Option<String>,
}

/// Host platform facts. Thinly wraps `dshl_core::platform::*` detection.
#[napi(object)]
pub struct PlatformInfo {
    pub os: String,
    pub arch: String,
    pub shell: String,
}

/// Health-check payload; matches the control-pipe `ping` dispatch exactly so
/// callers get the same shape whether they went through pipe / addon.
#[napi(object)]
pub struct PingInfo {
    pub pong: bool,
    pub version: String,
}

/// Status returned by `launchStatus()` — useful for the HTTP state route so
/// it can report "window/tray/dsh launched" without polling JS-side timers.
#[napi(object)]
pub struct LaunchStatus {
    pub launched: bool,
    pub kernel_running: bool,
    pub window_visible: bool,
    pub tray_visible: bool,
}

// ---------------------------------------------------------------------------
// Embedded PTY structs.
// ---------------------------------------------------------------------------

#[napi(object)]
pub struct TerminalSpawnOptions {
    pub shell: Option<String>,
    pub cwd: Option<String>,
    pub env: Option<std::collections::HashMap<String, String>>,
    pub prepend_path: Option<Vec<String>>,
    pub cols: Option<u16>,
    pub rows: Option<u16>,
}

#[napi(object)]
pub struct TerminalSpawnResult {
    pub id: String,
    pub pid: i64,
    pub ws_url: String,
}

#[napi(object)]
pub struct TerminalSessionInfo {
    pub id: String,
    pub pid: i64,
    pub shell: String,
    pub cwd: String,
    pub started_at_ms: i64,
    pub alive: bool,
}

#[napi(object)]
pub struct TerminalServerInfo {
    pub host: String,
    pub port: i64,
    pub token: String,
    pub url_prefix: String,
}
