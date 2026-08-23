//! Run-time options + outcome enums shared by the bin track and the addon
//! track.
//!
//! `RunOptions` is the struct form of the CLI flags (`--config`, `--debug`,
//! …) so the addon track can launch the kernel without re-parsing argv.
//! `RunOutcome` carries the CLI-only branches (`--help`, `--version`, bad
//! args, already running) out of `run_cli` so the binary shell can translate
//! them to exit codes without `std::process::exit` (which would kill the
//! hosting Node process in the addon case).

use std::path::PathBuf;

/// Result of a `run_cli` or `run_with_options` call that did **not** raise a
/// hard error but also did not start the event loop. The caller decides how
/// to report it (stdout print + `process.exit` in the bin, or a JS return
/// value in the cdylib).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunOutcome {
    HelpPrinted,
    VersionPrinted,
    ArgsError(String),
    AlreadyRunning,
}

/// Options for launching the full kernel programmatically (Track B addon).
#[derive(Debug, Default, Clone)]
pub struct RunOptions {
    pub config: Option<PathBuf>,
    pub debug: bool,
    /// When true, the runner honours the launcher-level single-instance lock
    /// ([ui] single-instance in dshl.toml). Disable when a higher-level
    /// supervisor (Node) already owns single-instance guarantees.
    pub enable_single_instance: bool,
    /// Enable the legacy HTTP control plane (DSHL_CONTROL_URL env). Defaults
    /// to false for the plugin track (the napi surface itself is the control
    /// plane); the CLI bin always passes true for backwards compatibility.
    pub enable_control_pipe: bool,
    /// When true, install the OS signal handler (Ctrl+C / SIGTERM) that asks
    /// the launcher to shut down cleanly. Set false when another host (Node)
    /// already owns signal handling and will call `RunHandle::shutdown()`.
    pub install_signal_handler: bool,
}
