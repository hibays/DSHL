//! Public high-level controls — the only surface the shared runner
//! (dshl-cli) is allowed to touch: show / hide / focus / visibility.

use std::sync::atomic::Ordering;

use crate::tray;
use crate::ui::{browser, state};

use super::{restore_from_tray, setup};
use crate::ui::launch;

/// Show the launcher window. If the window was trayed, restore it; if it was
/// never created (kernel boot via the addon track that skipped `setup`), go
/// through the full setup with the stashed CLI config; otherwise just focus
/// the existing visible window.
pub fn show() {
    if state::TRAYED.load(Ordering::SeqCst) {
        restore_from_tray(false);
    } else if state::WINDOW_ID.load(Ordering::SeqCst) == 0 {
        // Window was never created: go through a full setup. Only safe if
        // SETUP_DONE is false; otherwise we'd build a second window next to
        // the existing one (restore_from_tray above handles the trayed path).
        setup(state::cli_config_path());
        launch::launch_flow();
    } else {
        // Already visible — focus the CURRENT surface. Browser mode has no
        // WEBVIEW_HWND; without this branch the activation was a silent
        // no-op there.
        focus_current();
    }
}

/// Bring the CURRENTLY VISIBLE launcher surface to the foreground: the
/// tracked external browser window in browser mode, otherwise the embedded
/// WebView HWND. Used by single-instance activation and `show` — in browser
/// mode `WEBVIEW_HWND` is always zero, so a WebView-only focus was a silent
/// no-op.
pub fn focus_current() {
    if state::IS_BROWSER.load(Ordering::SeqCst) {
        // Prefer the detection pid (live tracking); fall back to the
        // teardown pid — it survives the close-to-tray forget, covering the
        // window where a misjudged close left the real browser alive.
        let pid = match browser::pid() {
            0 => browser::pid_for_teardown(),
            p => p,
        };
        if pid != 0
            && let Some(hwnd) = crate::platform::find_hwnd_by_pid(pid)
        {
            crate::debug::emit(&format!("focus_current: browser hwnd {hwnd:#x}"));
            crate::platform::focus_window(hwnd);
        }
        return;
    }
    let hwnd = state::WEBVIEW_HWND.load(Ordering::SeqCst);
    if hwnd != 0 {
        crate::platform::focus_window(hwnd);
    }
}

/// Hide the launcher window. When `close-to-tray` is enabled this transitions
/// to the tray (window resources freed, tray icon visible, dsh keeps
/// running). When `close-to-tray` is disabled this is a no-op: hiding the
/// window without a tray to hand over to is equivalent to quitting, which
/// must be explicit via [`crate::ui::request_shutdown`].
pub fn hide() {
    if !state::CLOSE_TO_TRAY.load(Ordering::SeqCst) {
        return;
    }
    // Manually request a "close to tray" transition: the supervisor loop
    // picks up PENDING_DESTROY and does the full teardown with tray start.
    let wid = state::WINDOW_ID.load(Ordering::SeqCst);
    if wid != 0 {
        // Drop the keep-alive (mirrors on_webview_close path) and mark the
        // window trayed. The supervisor loop will do the actual destroy on
        // the main thread.
        if let Some(keepalive) = state::KEEPALIVE.lock().unwrap().take() {
            keepalive.stop();
        }
        state::PENDING_DESTROY.store(wid, Ordering::SeqCst);
        state::TRAYED.store(true, Ordering::SeqCst);
        state::WEBVIEW_HWND.store(0, Ordering::SeqCst);
        tray::start();
        tray::hide_to_tray();
    }
}

/// True iff the launcher window currently exists and is not in the tray.
pub fn is_visible() -> bool {
    !state::TRAYED.load(Ordering::SeqCst) && state::WINDOW_ID.load(Ordering::SeqCst) != 0
}
