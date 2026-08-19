//! Modular, cross-platform launcher shutdown.
//!
//! Every shutdown entry point — the frontend `exit_app` binding, the tray
//! "quit" menu, SIGINT/SIGTERM (Ctrl+C), the WebView/browser close handlers
//! and the dsh clean-exit path — funnels into one composed teardown. Each
//! directly-callable exit step is a small module-level function and
//! [`shutdown`] composes them in a fixed order, mirroring the [`crate::tray`]
//! module's tiny platform-agnostic interface and webui's own examples (whose
//! `close_app` callback calls `webui_exit()`, then `webui_wait()` returns and
//! the example finishes with `webui_clean()`):
//!
//! 1. [`stop_keepalive`] — close the WebView keep-alive WebSocket so the
//!    window's webui server can stop.
//! 2. [`webui_exit`]     — webui's canonical, thread-safe, cross-platform
//!    teardown trigger: it closes every window/browser and stops every server,
//!    and makes `webui::wait_async()` return `false`. This is the exact method
//!    webui's own examples call.
//! 3. [`stop_tray`]      — remove the tray icon and stop its threads.
//! 4. [`stop_dsh`]       — graceful SIGINT/SIGTERM stop of the supervised dsh
//!    (commits its session log), the same mechanism on every OS.
//! 5. [`webui_clean`]    — final webui cleanup: waits for the server threads
//!    and releases the WebView2 controller, so the WebView2 browser processes
//!    exit on their own.
//!
//! Because webui's own exit signals the WebView thread to release the
//! controller in the order WebView2 expects, no process-tree scavenging (and
//! no Windows-only Toolhelp code) is ever needed — this module replaces the
//! old `reap_webview_descendants()` hack entirely.

use std::sync::atomic::Ordering;

use webui::webui;

use super::state;

/// Ask the launcher to shut down. Thread-safe; callable from any thread
/// (SIGINT/SIGTERM handler, webui event thread, tray menu, window close
/// handler).
///
/// Only the run_loop observes the flags and drives the composed teardown on
/// the main thread — webui is main-thread oriented, so the actual
/// [`webui::exit`] / [`webui::clean`] calls always happen there (see
/// [`shutdown`]), never on the signalling thread.
pub fn request_shutdown() {
    // SHOULD_EXIT is set too (not just SHUTDOWN_REQUESTED) so every shutdown
    // request also aborts a pending crash-recovery auto-restart and any launch
    // flow that is still deciding what to do.
    state::SHUTDOWN_REQUESTED.store(true, Ordering::SeqCst);
    state::SHOULD_EXIT.store(true, Ordering::SeqCst);
}

/// Whether a shutdown was requested (and not yet acted on). Used by the
/// control-plane tests to assert the wire `shutdown` reached the launcher.
#[cfg(test)]
pub(crate) fn shutdown_requested() -> bool {
    state::SHUTDOWN_REQUESTED.load(Ordering::SeqCst)
}

/// Close the window's keep-alive WebSocket so its webui server can stop. Safe
/// to call any number of times (the handle is taken once).
pub(crate) fn stop_keepalive() {
    if let Some(keepalive) = state::KEEPALIVE.lock().unwrap().take() {
        keepalive.stop();
    }
}

/// webui's universal, cross-platform teardown trigger: closes every window /
/// browser and stops every server, then makes `webui::wait_async()` return
/// `false`. Idempotent (a second call is a no-op).
pub(crate) fn webui_exit() {
    crate::debug::emit("exit: webui::exit()");
    webui::exit();
    crate::debug::emit("exit: webui::exit() returned");
}

/// Remove the tray icon and stop its background threads (no-op if the tray
/// was never started).
pub(crate) fn stop_tray() {
    crate::tray::shutdown();
}

/// Gracefully stop the supervised dsh child via Ctrl+C / SIGTERM (commits
/// dsh's session log). No-op when no child is tracked.
pub(crate) fn stop_dsh() {
    super::launch::kill_dsh();
}

/// Final webui cleanup: waits for the remaining server threads and releases
/// the WebView2 controller. Idempotent (it calls `webui_exit()` internally
/// when needed).
pub(crate) fn webui_clean() {
    crate::debug::emit("exit: webui::clean()");
    webui::clean();
    crate::debug::emit("exit: webui::clean() returned");
}

/// Run the full shutdown sequence. Called by the run_loop after it breaks.
///
/// `webui_running` mirrors webui's own `wait_async()` result from the last
/// loop pass: when webui has already finished tearing down on its own (e.g.
/// the window was closed for real and its servers stopped), there is nothing
/// left to signal, so the explicit [`webui_exit`] is skipped and the
/// idempotent [`webui_clean`] finalises the teardown.
///
/// `webui::exit()` runs *before* `kill_dsh()`: the window closes promptly
/// instead of staying frozen while the (up to 30s) graceful dsh stop waits,
/// and the WebView2 controller is released while dsh finishes shutting down —
/// the WebView2 browser processes then exit on their own, which is what makes
/// the old force-reap of `msedgewebview2.exe` unnecessary.
pub fn shutdown(webui_running: bool) {
    stop_keepalive();
    if webui_running {
        webui_exit();
    }
    stop_tray();
    stop_dsh();
    webui_clean();
}
