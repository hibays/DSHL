//! Tray-restore rebuild cycle.

use webui::webui;

use crate::progress;
use std::sync::atomic::Ordering;

use crate::ui::{browser, state};

use super::setup::show_window;

/// Re-create the launcher window after it was closed to tray: show it on the
/// running backend, navigate back to dsh, and re-capture the HWND. Shared by
/// the tray "restore" menu and single-instance activation. With
/// `show_launcher` the window shows the startup page instead of dsh (crash
/// recovery).
pub fn restore_from_tray(show_launcher: bool) {
    // Restore fires only once per request: while the (slow) window rebuild
    // is running, further double-clicks or menu items are ignored. If the
    // rebuild fails, the guard is released so the user can retry.
    if state::RESTORING.swap(true, Ordering::SeqCst) {
        crate::debug::emit("restore: already in progress, ignoring request");
        return;
    }
    crate::debug::emit("restore window from tray");

    // REALITY CHECK before rebuilding: if the external browser window is
    // actually still open, the earlier "closed" verdict was a misjudgment —
    // respawning here would create a SECOND browser window. Focus the live
    // one and un-mark the tray state instead; nothing needs rebuilding.
    let bhwnd = browser::hwnd();
    if bhwnd != 0 && crate::platform::is_window_alive(bhwnd) {
        {
            crate::platform::focus_window(bhwnd);
        }
        // Re-adopt the live window as the tracked pid: the supervisor's
        // poll_close then treats it as healthy-alive instead of re-firing
        // the spurious ToTray every few hundred ms. (The geometry sampler
        // is not restarted for this revived window; the store keeps its
        // last persisted values, which match what is on screen.)
        browser::adopt(bhwnd, crate::platform::window_pid(bhwnd));
        state::TRAYED.store(false, Ordering::SeqCst);
        state::RESTORING.store(false, Ordering::SeqCst);
        return;
    }

    // The webui-side window really is gone (or never existed): full rebuild.

    // The previous window's webui resources were already freed by the
    // supervisor loop when it closed to the tray (PENDING_DESTROY), so the
    // memory is released at close time rather than held while trayed. Stop any
    // leftover keep-alive defensively (the close handler normally already did;
    // this also covers platforms without a close handler).
    if let Some(keepalive) = state::KEEPALIVE.lock().unwrap().take() {
        keepalive.stop();
    }

    // Defer any close that lands while the window is being re-created: letting
    // webui tear the WebView down mid-`show_wv` deadlocks its own show-wait
    // loop (same protocol as `setup()`). A close during the rebuild is
    // honoured below by going back to the tray, so the tray never ends up
    // unable to summon a dead window.
    state::CLOSE_PENDING.store(false, Ordering::SeqCst);
    state::SETUP_DONE.store(false, Ordering::SeqCst);

    // Navigate the rebuilt window back to dsh (unless this is the crash
    // recovery path, which shows the launcher page instead).
    let navigate_back = if !show_launcher {
        progress::snapshot().url
    } else {
        None
    };
    let browser_mode = state::IS_BROWSER.load(Ordering::SeqCst);
    if browser_mode {
        // The browser is re-opened by `show_window`; mark a fresh probe pending
        // so the supervisor logs the newly located pid once capture lands.
        browser::note_window_recreated();
    }
    let shown = show_window(browser_mode, navigate_back, false);

    // With the close handler still deferred, no close could have torn the
    // WebView down. If the window is up and no close landed during the rebuild,
    // finish the restore; otherwise tear down and go back to the tray.
    let mut hwnd = 0usize;
    let mut keep = shown && !state::CLOSE_PENDING.load(Ordering::SeqCst);
    if keep {
        // `show_window` already held the WebView keep-alive / re-captured the
        // browser pid and applied the theme; the only step left here is the
        // synchronous HWND capture so the supervisor loop sees a live handle
        // immediately (the async capture would lag behind and the stale-zero
        // HWND could look like "no window") — WebView mode only.
        if !state::IS_BROWSER.load(Ordering::SeqCst) {
            let window = webui::Window::from_id(state::WINDOW_ID.load(Ordering::SeqCst));
            hwnd = window.get_hwnd() as usize;
            if hwnd != 0 {
                state::WEBVIEW_HWND.store(hwnd, Ordering::SeqCst);
            }
        }
        // A close that landed during the rebuild was deferred; honour it.
        keep = !state::CLOSE_PENDING.swap(false, Ordering::SeqCst);
    }
    if keep {
        state::TRAYED.store(false, Ordering::SeqCst);
        // Arm normal close handling only once the window is fully ours.
        state::SETUP_DONE.store(true, Ordering::SeqCst);
        // A close in the instant between the two stores above was still
        // deferred (the handler stays deferred until SETUP_DONE flips), so the
        // WebView is intact; roll back to the tray if the user closed it.
        keep = !state::CLOSE_PENDING.swap(false, Ordering::SeqCst);
    }

    if keep {
        // Window is live again; allow a future restore cycle (close to tray
        // again and double-click once more).
        state::RESTORING.store(false, Ordering::SeqCst);
        crate::debug::emit("restore: window re-created");
        // A freshly re-created window is not automatically the foreground
        // window; focus it so single-instance activation and tray restore
        // both bring dsh to the front.
        if hwnd != 0 {
            crate::platform::focus_window(hwnd);
        }
        return;
    }

    // The rebuild did not produce a window that should stay open: the show
    // failed entirely, or the user closed it during the rebuild (deferred
    // above). Free the fresh window and go back to the tray so the next
    // restore builds a clean one.
    crate::debug::emit("restore: window not kept (failed or closed during rebuild)");
    // Stop the keep-alive `show_window` may have spawned for a window we are
    // about to destroy, and free the window's webui resources
    // (struct/server/port) — allocated by `create_window` even when show
    // failed.
    if let Some(keepalive) = state::KEEPALIVE.lock().unwrap().take() {
        keepalive.stop();
    }
    let wid = state::WINDOW_ID.load(Ordering::SeqCst);
    if wid != 0 {
        webui::destroy(wid);
    }
    state::WINDOW_ID.store(0, Ordering::SeqCst);
    state::WEBVIEW_HWND.store(0, Ordering::SeqCst);
    browser::clear_pid();
    state::TRAYED.store(true, Ordering::SeqCst);
    state::SETUP_DONE.store(true, Ordering::SeqCst);
    state::RESTORING.store(false, Ordering::SeqCst);
}
