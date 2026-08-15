//! The main event loop and supervisor: runs webui's event loop, detects the
//! window/browser going away, handles tray requests and single-instance
//! activation, and performs the final clean shutdown.

use std::sync::atomic::Ordering;

use webui::webui;

use super::launch::kill_dsh;
use super::state;
use super::window;
use crate::tray;

/// Run the webui event loop until shutdown, then clean up dsh and webui.
pub fn run_loop() {
    crate::debug::emit("run_loop: started");

    // close-to-tray enabled: create the tray icon right away, not only on
    // the first window close, so the user knows the launcher lives in the
    // tray (and can quit via its menu) from the very start. Idempotent —
    // the close handler's later start() is a no-op.
    if state::CLOSE_TO_TRAY.load(Ordering::SeqCst) {
        tray::start();
    }

    loop {
        let alive = webui::wait_async();

        // Success path: the supervisor finished (dsh exited) or an explicit
        // exit was requested.
        if state::SHOULD_EXIT.load(Ordering::SeqCst) {
            crate::debug::emit("run_loop: SHOULD_EXIT");
            break;
        }
        // Ctrl+C / SIGTERM / WebView window close.
        if state::SHUTDOWN_REQUESTED.load(Ordering::SeqCst) {
            crate::debug::emit(&format!("run_loop: shutdown requested (alive={alive})"));
            kill_dsh();
            // Only stop the webui loop explicitly when the window is still up
            // (e.g. Ctrl+C in a console build). When the window already
            // closed, `wait_async()` above has already cleaned up the loop,
            // and calling `webui::exit()` again could touch freed WebView
            // state.
            if alive {
                crate::debug::emit("run_loop: calling webui::exit()");
                webui::exit();
                crate::debug::emit("run_loop: webui::exit() returned");
            }
            break;
        }
        // Tray menu "quit": go through the normal clean shutdown path.
        if tray::quit_requested() {
            crate::debug::emit("run_loop: tray quit requested");
            state::request_shutdown();
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
                        crate::debug::emit("browser window closed; shutting down");
                        state::request_shutdown();
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
                        state::TRAYED.store(true, Ordering::SeqCst);
                    } else {
                        crate::debug::emit("webview window closed; shutting down");
                        state::request_shutdown();
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
                window::restore_from_tray();
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
                window::restore_from_tray();
            } else {
                let hwnd = state::WEBVIEW_HWND.load(Ordering::SeqCst);
                if hwnd != 0 {
                    crate::platform::focus_window(hwnd);
                }
            }
        }

        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    tray::shutdown();
    crate::debug::emit("run_loop: calling webui::clean()");
    webui::clean();
    crate::debug::emit("run_loop: webui::clean() returned");
    kill_dsh();
    crate::debug::emit("run_loop: exiting");
}
