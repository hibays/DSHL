//! Shared UI state: the atomics and lazily-initialised paths that the UI
//! submodules (bindings / window / launch / supervisor) coordinate through.
//!
//! Keeping every piece of mutable cross-module state in one place makes the
//! coupling between the UI modules explicit: they never reach into each
//! other's privates, only through these statics and the few accessor fns.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{LazyLock, Mutex};

/// webui window id of the current launcher window (0 until created).
pub(crate) static WINDOW_ID: AtomicUsize = AtomicUsize::new(0);
/// Set when the app should exit (dsh exited, explicit exit request, …).
pub(crate) static SHOULD_EXIT: AtomicBool = AtomicBool::new(false);
/// True while a launch flow is running (only one at a time).
pub(crate) static FLOW_RUNNING: AtomicBool = AtomicBool::new(false);
/// True once dsh is up and the window has been navigated to it (supervisor
/// phase).
pub(crate) static LAUNCHED: AtomicBool = AtomicBool::new(false);
/// Set by the SIGINT/SIGTERM handler (and the WebView close handler).
pub(crate) static SHUTDOWN_REQUESTED: AtomicBool = AtomicBool::new(false);
/// True when the startup window is an external browser (vs. embedded
/// WebView).
pub(crate) static IS_BROWSER: AtomicBool = AtomicBool::new(false);
/// Browser-mode close-detection latch (the `webui::is_shown` fallback used
/// when the browser pid was never captured): true once the CURRENT window's
/// browser WebSocket has been seen connected. Cleared when the window goes
/// to the tray and on every re-creation, so a freshly restored browser must
/// connect again before its disappearance counts as a real close — otherwise
/// the stale latch would instantly re-enter "browser closed" right after a
/// HWND of the embedded WebView window (0 until captured).
pub(crate) static WEBVIEW_HWND: AtomicUsize = AtomicUsize::new(0);
/// True once [`super::setup`] has finished creating and showing the window.
pub(crate) static SETUP_DONE: AtomicBool = AtomicBool::new(false);
/// True when the user closed the window while it was still being created.
pub(crate) static CLOSE_PENDING: AtomicBool = AtomicBool::new(false);
/// `close-to-tray` config: closing the window hides to the tray (Windows /
/// macOS) or keeps the launcher running without a window (Linux, window
/// re-created on restore).
pub(crate) static CLOSE_TO_TRAY: AtomicBool = AtomicBool::new(false);
/// True when the window is currently hidden/closed in tray mode.
pub(crate) static TRAYED: AtomicBool = AtomicBool::new(false);
/// True while a tray restore (window re-creation) is in progress, so a
/// double-click or menu item during the slow rebuild does not stack
/// requests.
pub(crate) static RESTORING: AtomicBool = AtomicBool::new(false);
/// The current window's keep-alive WebSocket handle (WebView mode only; see
/// [`crate::wskeep`]). Stopped (and dropped) when the window closes to the
/// tray so its webui server can shut down; replaced on each window
/// re-creation.
pub(crate) static KEEPALIVE: Mutex<Option<crate::wskeep::KeepAlive>> = Mutex::new(None);
/// Browser-pid capture retry bookkeeping. Lives in shared state so a tray
/// restore can RESET the budget: each close->restore cycle is a fresh chance
/// to locate the browser process, otherwise the second and later cycles lose
/// webui window id whose resources the supervisor should free (0 = none).
/// Set when a window closes to the tray so its (large) struct + server + port
/// are freed promptly at close time instead of being held while trayed and
/// freed only on the next restore; consumed by the supervisor loop.
pub(crate) static PENDING_DESTROY: AtomicUsize = AtomicUsize::new(0);
/// Launcher page URL (`http://localhost:<port>/index.html`), captured when a
/// window is created; the crash recovery navigates back to it.
pub(crate) static LAUNCHER_URL: LazyLock<Mutex<String>> =
    LazyLock::new(|| Mutex::new(String::new()));
/// User cancelled the auto-restart (the countdown thread aborts).
pub(crate) static CRASH_CANCELLED: AtomicBool = AtomicBool::new(false);
/// User clicked 立即重启 — the countdown thread restarts dsh immediately.
pub(crate) static CRASH_RESTART_NOW: AtomicBool = AtomicBool::new(false);
/// Set by the launch worker after a crash; the UI event loop (main thread)
/// navigates back to the launcher page / restores the tray window.
pub(crate) static CRASH_NAVIGATE_PENDING: AtomicBool = AtomicBool::new(false);
/// Set by a control-plane `restart` request: the supervised dsh is asked to
/// exit, and when its supervisor observes a clean exit it relaunches instead
/// of shutting the launcher down.
pub(crate) static RESTART_REQUESTED: AtomicBool = AtomicBool::new(false);
/// Monotonic generation of the crash-recovery run. Each new crash increments
/// it; a superseded countdown thread notices `CRASH_GEN != its own` and exits
/// without touching the newer banner, so back-to-back crashes never run two
/// countdowns at once.
pub(crate) static CRASH_GEN: AtomicU32 = AtomicU32::new(0);

/// `--config` CLI value (kept for the launch flow).
pub(crate) static CLI_CONFIG_PATH: LazyLock<Mutex<Option<PathBuf>>> =
    LazyLock::new(|| Mutex::new(None));

/// Snapshot the `--config` CLI/option path so callers outside this module
/// (e.g. the napi `window_show` fallback that re-runs `setup`) can pass the
/// same config on the second boot without stashing it elsewhere.
pub(crate) fn cli_config_path() -> Option<PathBuf> {
    CLI_CONFIG_PATH.lock().unwrap().clone()
}
/// Path of the dshl.toml actually loaded (for "open config" and the UI).
pub(crate) static CONFIG_PATH: LazyLock<Mutex<Option<PathBuf>>> =
    LazyLock::new(|| Mutex::new(None));

/// Format every runtime flag as a single diagnostic line. Call when the
/// supervisor looks stuck to get a snapshot of the full UI state without
/// grepping log lines across four modules.
pub(crate) fn dump() -> String {
    format!(
        "window_id={window_id} trayed={trayed} setup_done={setup_done} \
         close_pending={close_pending} restoring={restoring} \
         pending_destroy={pending_destroy} is_browser={is_browser} \
         webview_hwnd={webview_hwnd:#x} launched={launched} \
         flow_running={flow_running} shutdown_requested={shutdown_requested} \
         should_exit={should_exit} close_to_tray={close_to_tray} \
         crash_gen={crash_gen} restart_requested={restart_requested}",
        window_id = WINDOW_ID.load(Ordering::SeqCst),
        trayed = TRAYED.load(Ordering::SeqCst),
        setup_done = SETUP_DONE.load(Ordering::SeqCst),
        close_pending = CLOSE_PENDING.load(Ordering::SeqCst),
        restoring = RESTORING.load(Ordering::SeqCst),
        pending_destroy = PENDING_DESTROY.load(Ordering::SeqCst),
        is_browser = IS_BROWSER.load(Ordering::SeqCst),
        webview_hwnd = WEBVIEW_HWND.load(Ordering::SeqCst),
        launched = LAUNCHED.load(Ordering::SeqCst),
        flow_running = FLOW_RUNNING.load(Ordering::SeqCst),
        shutdown_requested = SHUTDOWN_REQUESTED.load(Ordering::SeqCst),
        should_exit = SHOULD_EXIT.load(Ordering::SeqCst),
        close_to_tray = CLOSE_TO_TRAY.load(Ordering::SeqCst),
        crash_gen = CRASH_GEN.load(Ordering::SeqCst),
        restart_requested = RESTART_REQUESTED.load(Ordering::SeqCst),
    )
}

// ---------------------------------------------------------------------------
// State transition helpers — single source of truth for flag combinations.
//
// Every code path that mutates TRAYED / CLOSE_PENDING / RESTORING /
// PENDING_DESTROY / KEEPALIVE MUST go through these helpers. This replaces
// the previous pattern of five independent write-sites each maintaining a
// slightly different flag combination by convention.
// ---------------------------------------------------------------------------

/// Stop the window's keep-alive WebSocket so its webui server can shut down.
/// Safe to call any number of times (the handle is taken once).
pub(crate) fn stop_keepalive() {
    if let Some(keepalive) = KEEPALIVE.lock().unwrap().take() {
        keepalive.stop();
    }
}

/// Close-to-tray transition: stop the keep-alive, mark the window as
/// trayed, and record the window id for deferred destruction by the
/// supervisor loop. Called from the WebView close handler (which cannot
/// call `webui::destroy` on the webui event thread).
///
/// Does NOT clear `WEBVIEW_HWND` — the supervisor reads it when deciding
/// whether to focus on restore, and the stale handle is harmless while
/// trayed (the window is destroyed promptly).
pub(crate) fn enter_trayed_deferred(window_id: usize) {
    stop_keepalive();
    PENDING_DESTROY.store(window_id, Ordering::SeqCst);
    TRAYED.store(true, Ordering::SeqCst);
    crate::debug::emit(&format!("state: enter_trayed_deferred (win={window_id})"));
}

/// Close-to-tray transition with immediate resource cleanup. The caller
/// (supervisor win_gone or browser_close path) runs on the main thread
/// and can safely destroy the window / free the HWND.
pub(crate) fn enter_trayed_now(window_id: usize) {
    stop_keepalive();
    PENDING_DESTROY.store(window_id, Ordering::SeqCst);
    TRAYED.store(true, Ordering::SeqCst);
    crate::debug::emit(&format!("state: enter_trayed_now (win={window_id})"));
}

/// Begin a tray-restore rebuild. Returns `true` if the restore can proceed
/// (the guard was free); `false` if a rebuild is already in progress.
pub(crate) fn begin_restore() -> bool {
    !RESTORING.swap(true, Ordering::SeqCst)
}

/// Finish a failed tray restore: clear the restoring flag and go back to
/// trayed so the next attempt starts clean. The caller is responsible for
/// destroying the window and stopping the keep-alive before calling this
/// (those operations depend on `webui` which this module does not import).
pub(crate) fn finish_restore_fail() {
    WINDOW_ID.store(0, Ordering::SeqCst);
    WEBVIEW_HWND.store(0, Ordering::SeqCst);
    crate::ui::browser::clear_pid();
    TRAYED.store(true, Ordering::SeqCst);
    SETUP_DONE.store(true, Ordering::SeqCst);
    RESTORING.store(false, Ordering::SeqCst);
    crate::debug::emit("state: finish_restore_fail");
}

/// True iff the kernel has finished the startup pipeline and the window is
/// showing (or has navigated to) the real dsh URL. Exposed as a `pub` query
/// so `ui` can re-export it via `pub use` (the `state` module itself is
/// crate-private, so outside crates can only reach this through `ui::is_launched`).
pub fn is_launched() -> bool {
    LAUNCHED.load(Ordering::SeqCst)
}

/// Reset every "sticky" runtime flag (atomics + lazy paths) so a second
/// kernel boot in the same process doesn't inherit stale state. Called by
/// the shared entry point (dshl-cli) before each `ui::setup` — both tracks
/// (bin + addon) share the same single global UI state, so the entry point
/// owns the reset.
///
/// `CRASH_GEN` is intentionally **not** reset: it is monotonic so a stale
/// countdown thread notices its own generation is superseded. `CONFIG_PATH`
/// is not reset either: it is overwritten on the next `launch_flow`. The
/// stale-dsh PID is no longer kept here — `progress::stale_pid()` is the
/// single source of truth.
pub fn reset_runtime_state() {
    WINDOW_ID.store(0, Ordering::SeqCst);
    SHOULD_EXIT.store(false, Ordering::SeqCst);
    FLOW_RUNNING.store(false, Ordering::SeqCst);
    LAUNCHED.store(false, Ordering::SeqCst);
    SHUTDOWN_REQUESTED.store(false, Ordering::SeqCst);
    IS_BROWSER.store(false, Ordering::SeqCst);
    crate::ui::browser::reset_runtime_state();
    WEBVIEW_HWND.store(0, Ordering::SeqCst);
    SETUP_DONE.store(false, Ordering::SeqCst);
    CLOSE_PENDING.store(false, Ordering::SeqCst);
    TRAYED.store(false, Ordering::SeqCst);
    RESTORING.store(false, Ordering::SeqCst);
    PENDING_DESTROY.store(0, Ordering::SeqCst);
    CRASH_CANCELLED.store(false, Ordering::SeqCst);
    CRASH_RESTART_NOW.store(false, Ordering::SeqCst);
    CRASH_NAVIGATE_PENDING.store(false, Ordering::SeqCst);
    RESTART_REQUESTED.store(false, Ordering::SeqCst);
    stop_keepalive();
    *CLI_CONFIG_PATH.lock().unwrap() = None;
    *LAUNCHER_URL.lock().unwrap() = String::new();
}
