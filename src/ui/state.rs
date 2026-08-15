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
/// PID of the external browser window process (0 until captured).
pub(crate) static BROWSER_PID: AtomicUsize = AtomicUsize::new(0);
/// True once the browser pid has been probed at least once (logging only).
pub(crate) static BROWSER_CHECKED: AtomicBool = AtomicBool::new(false);
/// HWND of the embedded WebView window (0 until captured).
pub(crate) static WEBVIEW_HWND: AtomicUsize = AtomicUsize::new(0);
/// True once [`crate::ui::setup`] has finished creating and showing the
/// window.
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

/// `--config` CLI value (kept for the launch flow).
pub(crate) static CLI_CONFIG_PATH: LazyLock<Mutex<Option<PathBuf>>> =
    LazyLock::new(|| Mutex::new(None));
/// Path of the dshl.toml actually loaded (for "open config" and the UI).
pub(crate) static CONFIG_PATH: LazyLock<Mutex<Option<PathBuf>>> =
    LazyLock::new(|| Mutex::new(None));
/// PID of a stale dsh that did not exit on Ctrl+C and is awaiting the user's
/// explicit confirmation before being force-killed (0 = none).
pub(crate) static STALE_PID: AtomicU32 = AtomicU32::new(0);

/// Ask the launcher to shut down (SIGINT/SIGTERM handler, close handler).
pub fn request_shutdown() {
    SHUTDOWN_REQUESTED.store(true, Ordering::SeqCst);
}
