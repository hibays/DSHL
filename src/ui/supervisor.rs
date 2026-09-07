//! The main event loop and supervisor: runs webui's event loop, detects the
//! window/browser going away, handles tray requests and single-instance
//! activation, and performs the final clean shutdown.

use std::sync::atomic::Ordering;

use webui::webui;

use super::browser;
use super::exit;
use super::state;
use super::window;
use crate::progress;
use crate::tray;

/// Run the webui event loop until shutdown, then run the composed teardown
/// ([`exit::shutdown`]) to clean up dsh and webui.
/// Browser window closed while close-to-tray is on: free the (now idle)
/// webui window, reset every piece of browser-tracking state, and enter tray
/// mode. Single source for the transition invariants — both the pid-based and
/// the `is_shown`-latch close paths funnel here, so adding a new piece of
/// browser state means updating ONE place, not two diverging copies.
fn browser_close_enter_tray() {
    crate::debug::emit("close-to-tray: browser window closed, dsh keeps running");
    state::PENDING_DESTROY.store(state::WINDOW_ID.load(Ordering::SeqCst), Ordering::SeqCst);
    browser::note_closed_to_tray();
    state::TRAYED.store(true, Ordering::SeqCst);
}

pub fn run_loop() {
    crate::debug::emit("run_loop: started");

    // close-to-tray enabled: create the tray icon right away, not only on
    // the first window close, so the user knows the launcher lives in the
    // tray (and can quit via its menu) from the very start. Idempotent —
    // the close handler's later start() is a no-op.
    if state::CLOSE_TO_TRAY.load(Ordering::SeqCst) {
        tray::start();
    }

    // The last `webui::wait_async()` result; passed to `exit::shutdown` so it
    // only signals webui (webui::exit) when there is still something to tear
    // down (when the window already closed on its own, `wait_async()` returns
    // false and the idempotent `webui::clean()` finalises the teardown).
    let mut alive;

    // Browser-mode lifecycle state (pid, shown-latch, capture budget,
    // navigation latch) is private to `super::browser`; `restore_from_tray`
    // re-arms it per tray cycle via `note_window_recreated`. This loop only
    // asks `poll_close()` every tick and executes the decision it returns.
    loop {
        alive = webui::wait_async();

        // Success path: the supervisor finished (dsh exited) or an explicit
        // exit was requested.
        if state::SHOULD_EXIT.load(Ordering::SeqCst) {
            crate::debug::emit("run_loop: SHOULD_EXIT");
            break;
        }
        // Ctrl+C / SIGTERM / WebView window close.
        if state::SHUTDOWN_REQUESTED.load(Ordering::SeqCst) {
            crate::debug::emit(&format!("run_loop: shutdown requested (alive={alive})"));
            break;
        }
        // Tray menu "quit": go through the normal clean shutdown path.
        if tray::quit_requested() {
            crate::debug::emit("run_loop: tray quit requested");
            exit::request_shutdown();
        }

        // Free a window that was closed to the tray promptly, instead of
        // holding its (large) webui struct + server + port while trayed and
        // freeing it only on the next restore. Set by the WebView close
        // handler, the win-gone branch or the browser-close branch; destroy
        // runs here on the main thread (webui is main-thread oriented). Never
        // free a window webui still reports as shown/connected (the WebView
        // may be mid-teardown right after the close handler); retry next pass.
        let pending_destroy = state::PENDING_DESTROY.swap(0, Ordering::SeqCst);
        if pending_destroy != 0 {
            if webui::is_shown(pending_destroy) {
                state::PENDING_DESTROY.store(pending_destroy, Ordering::SeqCst);
            } else {
                crate::debug::emit(&format!(
                    "run_loop: freeing closed window id {pending_destroy}"
                ));
                webui::destroy(pending_destroy);
                // Only clear WINDOW_ID if it still refers to the window we
                // destroyed (a restore in the meantime creates a newer one).
                if state::WINDOW_ID.load(Ordering::SeqCst) == pending_destroy {
                    state::WINDOW_ID.store(0, Ordering::SeqCst);
                }
            }
        }

        // Crash recovery: dsh exited unexpectedly. Navigate the window back
        // to the launcher page so the user sees the auto-restart countdown —
        // restoring the tray window first if it is currently hidden.
        if state::CRASH_NAVIGATE_PENDING.swap(false, Ordering::SeqCst) {
            crate::debug::emit("run_loop: crash recovery — show startup page");
            if state::TRAYED.load(Ordering::SeqCst) {
                window::restore_from_tray(true);
            } else {
                window::navigate_to_launcher();
            }
        }

        // Tray "open dsh": open the dsh URL via the platform `open_url`
        // path. This is the historical contract of the menu item — it hands
        // the URL to the OS, it does not manage launcher windows.
        if tray::open_url_requested()
            && let Some(url) = progress::snapshot().url
            && let Err(e) = crate::platform::open_url(&url)
        {
            crate::debug::emit(&format!("tray open-dsh: open_url failed: {e}"));
        }

        // Startup phase: if the window is gone before dsh was handed off,
        // treat it as "user closed the launcher" and stop.
        if !state::LAUNCHED.load(Ordering::SeqCst) && !alive {
            crate::debug::emit("run_loop: startup window gone");
            // The flag MUST be raised before breaking out: the launch worker's
            // checkpoints (src/ui/launch.rs) all key off SHUTDOWN_REQUESTED.
            // Without it a worker still inside `flow::run` never learns about
            // this exit and can spawn dsh after teardown has already begun
            // (`exit::shutdown`'s kill_dsh has run by then), orphaning the
            // child. Idempotent; also aborts a pending crash-restart.
            exit::request_shutdown();
            break;
        }
        // Browser mode: ONE poll tick owns all browser-lifecycle detection -
        // pid-alive check, the was_shown latch for an uncaptured pid, capture
        // throttling and its retry budget - and returns what must happen when
        // a close is detected. Startup phase: a close quits outright (there
        // is nothing to hand over to yet). Supervising phase: honour
        // close-to-tray. Skipped while TRAYED: the window has already been
        // handed over (and its webui id freed), so every tick would only
        // re-detect the same stale "closed" state in a loop.
        if state::IS_BROWSER.load(Ordering::SeqCst) && !state::TRAYED.load(Ordering::SeqCst) {
            let phase = if state::LAUNCHED.load(Ordering::SeqCst) {
                browser::Phase::Supervising
            } else {
                browser::Phase::Startup
            };
            // Presence primitive (HWND liveness) lives inside poll_close —
            // the tick only supplies the socket signal and window id.
            let tick = browser::poll_close(
                phase,
                state::WINDOW_ID.load(Ordering::SeqCst),
                webui::is_shown,
            );
            match tick.action {
                browser::CloseAction::ToTray => browser_close_enter_tray(),
                browser::CloseAction::Quit => {
                    crate::debug::emit("browser window closed; shutting down");
                    exit::request_shutdown();
                }
                browser::CloseAction::None => {}
            }
            if tick.retry_capture {
                window::capture_browser_pid();
            }
        }

        // Supervisor phase: detect the window that shows dsh going away.
        if state::LAUNCHED.load(Ordering::SeqCst) {
            // Browser mode is handled by the poll_close tick above (both
            // phases). Here: WebView mode only - detect the window actually
            // being destroyed (user close, or webui's bridge-drop cleanup)
            // via its HWND, rather than webui's `connected` state (which
            // flips on navigate too).
            #[cfg(target_os = "windows")]
            let hwnd = state::WEBVIEW_HWND.load(Ordering::SeqCst);
            #[cfg(target_os = "windows")]
            let win_gone = hwnd != 0 && !crate::platform::is_window_alive(hwnd);
            #[cfg(not(target_os = "windows"))]
            let win_gone = !alive;
            if win_gone && !state::TRAYED.load(Ordering::SeqCst) {
                if state::CLOSE_TO_TRAY.load(Ordering::SeqCst) {
                    crate::debug::emit("close-to-tray: window gone, dsh keeps running");
                    if let Some(keepalive) = state::KEEPALIVE.lock().unwrap().take() {
                        keepalive.stop();
                    }
                    state::PENDING_DESTROY
                        .store(state::WINDOW_ID.load(Ordering::SeqCst), Ordering::SeqCst);
                    state::WEBVIEW_HWND.store(0, Ordering::SeqCst);
                    state::TRAYED.store(true, Ordering::SeqCst);
                } else {
                    crate::debug::emit("webview window closed; shutting down");
                    exit::request_shutdown();
                }
            }
        }

        // Tray "restore window": re-create the window (it was destroyed on
        // close to save memory), re-apply the saved geometry, and navigate
        // back to dsh. The request flag is ALWAYS consumed: when the window
        // is already visible the request is dropped (focusing the window as
        // feedback) instead of lingering — otherwise a restore click while
        // visible would fire the moment the window is later closed to the
        // tray again ("closing opens a new window").
        if tray::restore_requested() {
            if state::TRAYED.load(Ordering::SeqCst) {
                window::restore_from_tray(false);
            } else {
                crate::debug::emit("restore requested but window visible; focusing instead");
                let hwnd = state::WEBVIEW_HWND.load(Ordering::SeqCst);
                if hwnd != 0 {
                    crate::platform::focus_window(hwnd);
                }
            }
        }

        // Single-instance activation: a second dshl was launched and asked
        // this instance to come to the foreground. Restore from the tray if
        // hidden, otherwise just focus the existing window.
        if crate::platform::single_instance::poll_activate() {
            crate::debug::emit("single-instance: activation requested by a second instance");
            if state::TRAYED.load(Ordering::SeqCst) {
                window::restore_from_tray(false);
            } else {
                // Backend-aware: browser mode focuses the tracked Edge
                // window (WEBVIEW_HWND is always zero there).
                window::focus_current();
            }
        }

        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    // Composed, cross-platform teardown (see `exit`): stop the keep-alive,
    // webui::exit() to close the window/servers (only when webui is still
    // running), tray shutdown, graceful dsh stop, then webui::clean().
    exit::shutdown(alive);
    crate::debug::emit("run_loop: exiting");
}
