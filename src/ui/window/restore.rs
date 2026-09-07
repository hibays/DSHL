//! Tray-restore rebuild cycle.

use webui::webui;

use crate::progress;
use std::sync::atomic::Ordering;

use crate::ui::{browser, state};

use super::setup::show_window;

/// The action a caller should take after querying the restore state machine.
/// Pure value — no side effects, fully testable.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum RestoreAction {
    /// Another restore is already in progress; skip this request.
    Skip,
    /// The external browser is actually still alive — focus it instead of
    /// rebuilding. Contains the browser HWND.
    FocusExistingBrowser(usize),
    /// Proceed with a full window rebuild. Contains `browser_mode` and the
    /// URL to navigate to after the rebuild (None = show launcher page).
    Rebuild {
        browser_mode: bool,
        navigate_url: Option<String>,
    },
    /// The rebuild produced no usable window (show failed or user closed it
    /// during rebuild). Roll back to the tray.
    Rollback,
    /// The rebuild succeeded and the window is live. Contains the WebView
    /// HWND (0 in browser mode).
    Success { hwnd: usize },
}

/// Pure state machine for the tray-restore decision. Reads shared state
/// atomics and returns the action the caller should take — no side effects,
/// no platform calls, fully unit-testable.
///
/// `browser_hwnd` is the value of `browser::hwnd()` at call time (injected
/// so the function stays pure). `browser_alive` is whether that HWND is
/// actually alive (`platform::is_window_alive`). `navigate_url` is the URL
/// the caller intends to navigate to after rebuild (from
/// `progress::snapshot().url`).
pub(crate) fn decide_restore_action(
    browser_hwnd: usize,
    browser_alive: bool,
    navigate_url: Option<String>,
) -> RestoreAction {
    // Guard: only one restore at a time.
    if !state::begin_restore() {
        return RestoreAction::Skip;
    }

    // Reality check: if the browser is still alive, don't rebuild.
    if browser_hwnd != 0 && browser_alive {
        return RestoreAction::FocusExistingBrowser(browser_hwnd);
    }

    let browser_mode = state::IS_BROWSER.load(Ordering::SeqCst);
    RestoreAction::Rebuild {
        browser_mode,
        navigate_url,
    }
}

/// Evaluate a completed rebuild: given the show result and close-pending
/// state, returns `Success` or `Rollback`. Also returns the captured WebView
/// HWND (0 in browser mode or on rollback). Pure — no side effects.
pub(crate) fn evaluate_rebuild(shown: bool) -> RestoreAction {
    let hwnd = if shown && !state::CLOSE_PENDING.load(Ordering::SeqCst) {
        let mut h = 0usize;
        if !state::IS_BROWSER.load(Ordering::SeqCst) {
            let window = webui::Window::from_id(state::WINDOW_ID.load(Ordering::SeqCst));
            h = window.get_hwnd() as usize;
        }
        h
    } else {
        0
    };

    if shown && !state::CLOSE_PENDING.swap(false, Ordering::SeqCst) {
        RestoreAction::Success { hwnd }
    } else {
        RestoreAction::Rollback
    }
}

/// Re-create the launcher window after it was closed to tray: show it on the
/// running backend, navigate back to dsh, and re-capture the HWND. Shared by
/// the tray "restore" menu and single-instance activation. With
/// `show_launcher` the window shows the startup page instead of dsh (crash
/// recovery).
pub fn restore_from_tray(show_launcher: bool) {
    let navigate_url = if !show_launcher {
        progress::snapshot().url
    } else {
        None
    };

    let bhwnd = browser::hwnd();
    let balive = bhwnd != 0 && crate::platform::is_window_alive(bhwnd);

    match decide_restore_action(bhwnd, balive, navigate_url) {
        RestoreAction::Skip => {
            crate::debug::emit("restore: already in progress, ignoring request");
        }
        RestoreAction::FocusExistingBrowser(h) => {
            crate::debug::emit("restore: browser still alive, focusing it");
            crate::platform::focus_window(h);
            browser::adopt(h, crate::platform::window_pid(h));
            state::TRAYED.store(false, Ordering::SeqCst);
            state::RESTORING.store(false, Ordering::SeqCst);
        }
        RestoreAction::Rebuild {
            browser_mode,
            navigate_url,
        } => {
            crate::debug::emit("restore window from tray");
            perform_rebuild(browser_mode, navigate_url);
        }
        _ => unreachable!("decide_restore_action never returns Success/Rollback"),
    }
}

/// Execute a full window rebuild. Separated from the decision logic so the
/// state machine remains pure and testable.
fn perform_rebuild(browser_mode: bool, navigate_url: Option<String>) {
    state::stop_keepalive();

    // Defer any close that lands while the window is being re-created: letting
    // webui tear the WebView down mid-`show_wv` deadlocks its own show-wait
    // loop (same protocol as `setup()`). A close during the rebuild is
    // honoured below by going back to the tray, so the tray never ends up
    // unable to summon a dead window.
    state::CLOSE_PENDING.store(false, Ordering::SeqCst);
    state::SETUP_DONE.store(false, Ordering::SeqCst);

    if browser_mode {
        browser::note_window_recreated();
    }
    let shown = show_window(browser_mode, navigate_url, false);

    match evaluate_rebuild(shown) {
        RestoreAction::Success { hwnd } => {
            state::TRAYED.store(false, Ordering::SeqCst);
            state::SETUP_DONE.store(true, Ordering::SeqCst);
            // A close in the instant between the two stores above was still
            // deferred (the handler stays deferred until SETUP_DONE flips), so
            // the WebView is intact; roll back to the tray if the user closed
            // it.
            if state::CLOSE_PENDING.swap(false, Ordering::SeqCst) {
                crate::debug::emit("restore: close landed during rebuild, rolling back");
                state::stop_keepalive();
                let wid = state::WINDOW_ID.load(Ordering::SeqCst);
                if wid != 0 {
                    webui::destroy(wid);
                }
                state::finish_restore_fail();
                return;
            }
            state::RESTORING.store(false, Ordering::SeqCst);
            crate::debug::emit("restore: window re-created");
            if hwnd != 0 {
                crate::platform::focus_window(hwnd);
            }
        }
        RestoreAction::Rollback => {
            crate::debug::emit("restore: window not kept (failed or closed during rebuild)");
            state::stop_keepalive();
            let wid = state::WINDOW_ID.load(Ordering::SeqCst);
            if wid != 0 {
                webui::destroy(wid);
            }
            state::finish_restore_fail();
        }
        _ => unreachable!("evaluate_rebuild only returns Success or Rollback"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ensure each test starts with a clean restore state.
    fn clean_state() {
        state::RESTORING.store(false, Ordering::SeqCst);
        state::CLOSE_PENDING.store(false, Ordering::SeqCst);
        state::IS_BROWSER.store(false, Ordering::SeqCst);
    }

    #[test]
    fn skip_when_already_restoring() {
        clean_state();
        state::RESTORING.store(true, Ordering::SeqCst);
        let action = decide_restore_action(0, false, None);
        assert_eq!(action, RestoreAction::Skip);
        clean_state();
    }

    #[test]
    fn focus_existing_browser_when_alive() {
        clean_state();
        let action = decide_restore_action(42, true, Some("http://example.com".into()));
        assert_eq!(action, RestoreAction::FocusExistingBrowser(42));
    }

    #[test]
    fn rebuild_when_browser_gone() {
        clean_state();
        state::IS_BROWSER.store(true, Ordering::SeqCst);
        let action = decide_restore_action(0, false, Some("http://dsh".into()));
        assert_eq!(
            action,
            RestoreAction::Rebuild {
                browser_mode: true,
                navigate_url: Some("http://dsh".into()),
            }
        );
        clean_state();
    }

    #[test]
    fn rebuild_webview_mode() {
        clean_state();
        state::IS_BROWSER.store(false, Ordering::SeqCst);
        let action = decide_restore_action(99, false, None);
        assert_eq!(
            action,
            RestoreAction::Rebuild {
                browser_mode: false,
                navigate_url: None,
            }
        );
    }

    #[test]
    fn evaluate_rebuild_success_webview() {
        clean_state();
        state::IS_BROWSER.store(false, Ordering::SeqCst);
        // WINDOW_ID 0 → HWND will be 0, but the action is still Success
        let action = evaluate_rebuild(true);
        assert!(matches!(action, RestoreAction::Success { .. }));
    }

    #[test]
    fn evaluate_rebuild_rollback_on_close_pending() {
        clean_state();
        state::CLOSE_PENDING.store(true, Ordering::SeqCst);
        let action = evaluate_rebuild(true);
        assert_eq!(action, RestoreAction::Rollback);
    }

    #[test]
    fn evaluate_rebuild_rollback_on_show_failure() {
        clean_state();
        let action = evaluate_rebuild(false);
        assert_eq!(action, RestoreAction::Rollback);
    }
}
