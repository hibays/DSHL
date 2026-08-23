//! Supervisor controls: shutdown / restart, plus the detached-restart
//! fallback used when no kernel is running (legacy addon behaviour parity).
//!
//! When a kernel *is* running, both `shutdown()` and `restart()` defer to
//! `dshl_cli::request_*` (thread-safe flag flips that the supervisor loop
//! observes on the kernel thread). When no kernel is running, `restart()`
//! spawns a detached copy of the hosting dsh process so a plugin-only Node
//! host can still rotate dsh without a kernel boot.

use std::process::{Command, Stdio};

use napi_derive::napi;

use crate::kernel::is_kernel_running;
use crate::types::RequestRestartOptions;

/// Ask the running kernel to shut down cleanly (stops dsh via graceful
/// SIGINT/SIGTERM so dsh saves its session log; tears down the window +
/// tray; runs the composed webui cleanup). Returns quickly — the actual
/// shutdown proceeds on the kernel's own threads.
///
/// When no kernel is running this is still valid for contract symmetry with
/// the control-pipe shutdown dispatch; it returns true and the JS side is
/// expected to do `process.exit()` itself if that is the desired outcome.
#[napi]
pub fn shutdown() -> bool {
    if is_kernel_running() {
        dshl_cli::request_shutdown();
        true
    } else {
        // Signal the caller (JS) that shutdown is "acknowledged". Historically
        // the pipe shutdown dispatch just set a flag too; we don't exit the
        // hosting Node process from inside Rust.
        true
    }
}

/// Ask the supervised dsh (and launcher kernel) to restart. Two modes:
///   * kernel is running → defer to the kernel's restart semantics: it
///     signals dsh, waits for a clean exit, then re-enters the launch flow
///     (no process-level teardown — keeps the tray / window handle).
///   * kernel is NOT running → spawn a detached child using the provided
///     `{cmd, args, cwd, path}` options (same as the legacy addon restart).
///     JS side should `process.exit(0)` on the next tick after this returns
///     true so the HTTP response flushes before exit.
#[napi]
pub fn restart(options: Option<RequestRestartOptions>) -> bool {
    if is_kernel_running() {
        dshl_cli::request_restart();
        true
    } else if let Some(opts) = options {
        spawn_detached_restart(&opts)
    } else {
        false
    }
}

fn spawn_detached_restart(opts: &RequestRestartOptions) -> bool {
    let mut cmd = Command::new(&opts.cmd);
    cmd.args(&opts.args);
    if let Some(cwd) = opts
        .cwd
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(std::path::PathBuf::from)
    {
        cmd.current_dir(cwd);
    }
    if let Some(p) = opts.path.as_deref().map(std::ffi::OsStr::new) {
        cmd.env("PATH", p);
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    spawn_detached(&mut cmd)
}

// ---------------------------------------------------------------------------
// Platform specifics for the detached-restart fallback. Std-only; mirrors
// the historical addon version (deliberately kept identical for
// behaviour-parity across plugin-track restarts).
// ---------------------------------------------------------------------------

#[cfg(windows)]
fn spawn_detached(cmd: &mut Command) -> bool {
    use std::os::windows::process::CommandExt;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    cmd.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW | DETACHED_PROCESS);
    cmd.spawn().map(|_c| ()).is_ok()
}

#[cfg(unix)]
fn spawn_detached(cmd: &mut Command) -> bool {
    use std::os::unix::process::CommandExt;
    cmd.process_group(0);
    cmd.spawn().map(|_c| ()).is_ok()
}

#[cfg(all(not(windows), not(unix)))]
fn spawn_detached(cmd: &mut Command) -> bool {
    cmd.spawn().map(|_c| ()).is_ok()
}
