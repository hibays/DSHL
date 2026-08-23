//! Kernel lifecycle: boot, status, running-handle ownership.
//!
//! `launch()` is the entry point for the plugin track — it spawns the full
//! dshl kernel on a background thread and hands back a [`RunHandle`] whose
//! methods (in `super::supervisor` / `super::window` / `super::tray`) drive
//! the running kernel across threads.

use std::path::PathBuf;
use std::sync::{LazyLock, Mutex};

use dshl_cli::{RunHandle, RunOptions};
use napi_derive::napi;

use crate::types::{LaunchOptions, LaunchStatus};

/// Handle to the running kernel. An `Option<RunHandle>` lets callers tell
/// "has launch() been called?" from "is it currently alive?"
/// (`RunHandle::is_running()`).
pub(crate) static RUN_HANDLE: LazyLock<Mutex<Option<RunHandle>>> =
    LazyLock::new(|| Mutex::new(None));

/// Boot the full dshl kernel on a background thread (returns immediately).
///
/// The same exact code paths as the installer binary run: DPI init, locale
/// init, debug setup, optional single-instance lock, option signal handler,
/// window setup, launch flow, and the 50 ms supervisor loop that handles
/// tray menus / dsh supervision / crash recovery / single-instance
/// activation.
///
/// Idempotent: if a kernel is already running in this process, the returned
/// status reuses the existing instance (no duplicate window).
#[napi]
pub fn launch(options: Option<LaunchOptions>) -> napi::Result<bool> {
    let mut guard = RUN_HANDLE.lock().unwrap_or_else(|p| p.into_inner());
    if let Some(h) = guard.as_ref() {
        if h.is_running() {
            return Ok(false);
        }
        // Finished handle — drop it so a fresh one is built below.
        *guard = None;
    }
    let opts = options.unwrap_or(LaunchOptions {
        config: None,
        debug: None,
        enable_single_instance: None,
        enable_control_pipe: None,
        install_signal_handler: None,
    });
    let run_opts = RunOptions {
        config: opts.config.map(PathBuf::from),
        debug: opts.debug.unwrap_or(false),
        enable_single_instance: opts.enable_single_instance.unwrap_or(true),
        enable_control_pipe: opts.enable_control_pipe.unwrap_or(false),
        install_signal_handler: opts.install_signal_handler.unwrap_or(false),
    };
    let handle = dshl_cli::run_with_options(run_opts)
        .map_err(|e| napi::Error::from_reason(e.to_string()))?;
    *guard = Some(handle);
    Ok(true)
}

/// True iff a kernel was booted via `launch()` and is still currently alive
/// (supervisor loop hasn't returned yet).
#[napi]
pub fn is_kernel_running() -> bool {
    RUN_HANDLE
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .as_ref()
        .map(|h| h.is_running())
        .unwrap_or(false)
}

/// Aggregate status — one call lets the JS state route populate everything.
#[napi]
pub fn launch_status() -> LaunchStatus {
    let running = is_kernel_running();
    LaunchStatus {
        launched: dshl_cli::is_launched(),
        kernel_running: running,
        window_visible: dshl_cli::window_is_visible(),
        tray_visible: dshl_cli::tray_is_visible(),
    }
}
