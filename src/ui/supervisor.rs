//! The main event loop and supervisor: runs webui's event loop, detects the
//! window/browser going away, handles tray requests and single-instance
//! activation, and performs the final clean shutdown.

use std::sync::atomic::Ordering;

use webui::webui;

use super::exit;
use super::state;
use super::window;
use crate::progress;
use crate::tray;

/// Run the webui event loop until shutdown, then run the composed teardown
/// ([`exit::shutdown`]) to clean up dsh and webui.
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

        // Tray "open dsh url": open the dsh deploy page in the system default
        // browser (e.g. while the window is hidden to the tray).
        if tray::open_url_requested()
            && let Some(url) = progress::snapshot().url
        {
            crate::debug::emit(&format!("tray: opening dsh url in system browser ({url})"));
            let _ = crate::platform::open_url(&url);
        }

        // Startup phase: if the window is gone before dsh was handed off,
        // treat it as "user closed the launcher" and stop.
        if !state::LAUNCHED.load(Ordering::SeqCst) && !alive {
            crate::debug::emit("run_loop: startup window gone");
            break;
        }

        // Supervisor phase: detect the window that shows dsh going away.
        if state::LAUNCHED.load(Ordering::SeqCst) {
            if state::IS_BROWSER.load(Ordering::SeqCst) {
                // Browser mode: track the external browser process. Closing it
                // either hands over to the tray (close-to-tray, dsh keeps
                // running and the tray re-opens the browser on restore) or
                // shuts down and reaps dsh.
                let pid = state::BROWSER_PID.load(Ordering::SeqCst);
                if pid != 0 {
                    if !state::BROWSER_CHECKED.swap(true, Ordering::SeqCst) {
                        crate::debug::emit(&format!(
                            "browser supervisor active (pid {pid}, alive={})",
                            crate::platform::process_alive(pid as u32)
                        ));
                    }
                    if !crate::platform::process_alive(pid as u32) {
                        if state::CLOSE_TO_TRAY.load(Ordering::SeqCst) {
                            // Browser mode has no WebView close handler to
                            // hand the close over to the tray, so this branch
                            // is it: keep dsh running, free the (now idle)
                            // webui window, and let the tray re-open the
                            // browser on restore.
                            crate::debug::emit(
                                "close-to-tray: browser window closed, dsh keeps running",
                            );
                            state::PENDING_DESTROY
                                .store(state::WINDOW_ID.load(Ordering::SeqCst), Ordering::SeqCst);
                            state::BROWSER_PID.store(0, Ordering::SeqCst);
                            state::BROWSER_CHECKED.store(false, Ordering::SeqCst);
                            state::TRAYED.store(true, Ordering::SeqCst);
                        } else {
                            crate::debug::emit("browser window closed; shutting down");
                            exit::request_shutdown();
                        }
                    }
                }
            } else {
                // WebView mode: the embedded window shows dsh. Detect the
                // window actually being destroyed (user close, or webui's
                // bridge-drop cleanup) via its HWND, rather than webui's
                // `connected` state (which flips on navigate too).
                #[cfg(target_os = "windows")]
                let hwnd = state::WEBVIEW_HWND.load(Ordering::SeqCst);
                #[cfg(target_os = "windows")]
                let win_gone = hwnd != 0 && !crate::platform::is_window_alive(hwnd);
                #[cfg(not(target_os = "windows"))]
                let win_gone = !alive;
                if win_gone && !state::TRAYED.load(Ordering::SeqCst) {
                    if state::CLOSE_TO_TRAY.load(Ordering::SeqCst) {
                        // close-to-tray without the close handler (e.g. Linux
                        // / macOS, or the window died another way): keep dsh
                        // running.
                        crate::debug::emit("close-to-tray: window gone, dsh keeps running");
                        // Stop the window's keep-alive so its webui server can
                        // shut down (this branch covers platforms without a
                        // WebView close handler, or a window that died another
                        // way) and let the supervisor free the window struct.
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
                let hwnd = state::WEBVIEW_HWND.load(Ordering::SeqCst);
                if hwnd != 0 {
                    crate::platform::focus_window(hwnd);
                }
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
