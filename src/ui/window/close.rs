//! The WebView close-handler policy.
//!
//! Normally: remember the window geometry (unless maximized/fullscreen), then
//! ask the event loop to kill dsh and shut down, returning `true` to allow the
//! close. If the window closes while [`crate::ui::window::setup`] is still
//! creating the WebView2 (`show_wv` in progress), tearing it down now
//! deadlocks webui's own show wait loop — the close is deferred (return
//! `false`) and re-applied once setup finishes.

use std::sync::atomic::Ordering;

use webui::webui;

use crate::tray;
use crate::ui::{exit, geometry, state};

/// WebView close handler.
pub unsafe extern "C" fn on_webview_close(window: usize) -> bool {
    crate::debug::emit(&format!("webview close handler fired (window id {window})"));
    if !state::SETUP_DONE.load(Ordering::SeqCst) {
        // The window is still being created (`show_wv` is mid-WebView2 init).
        // Letting webui tear the WebView down now deadlocks its own `show_wv`
        // wait loop, so defer the close: we re-apply it once setup finishes.
        state::CLOSE_PENDING.store(true, Ordering::SeqCst);
        crate::debug::emit("close during setup; deferring");
        return false;
    }
    // close-to-tray: once dsh is up, closing the window lets the WebView
    // (or browser) die for real — its processes exit and memory is freed —
    // while the launcher keeps dsh running in the background. The tray icon
    // re-creates the window on click; quit via the tray menu or Ctrl+C.
    // During startup there is nothing to keep alive, so the close still
    // exits.
    if state::CLOSE_TO_TRAY.load(Ordering::SeqCst) && state::LAUNCHED.load(Ordering::SeqCst) {
        geometry::remember_webview(window);
        let hwnd = webui::Window::from_id(window).get_hwnd() as usize;
        // Windows needs the WebView HWND as the tray anchor sanity check;
        // other platforms have no HWND concept (get_hwnd returns 0) yet
        // still want the close to hand over to the tray.
        if hwnd != 0 || !cfg!(target_os = "windows") {
            // Clear the tracked HWND so the supervisor loop does not mistake
            // the stale handle for a live window (which would re-trigger tray
            // mode or shutdown), then enter tray mode right here. The
            // keep-alive is stopped and PENDING_DESTROY is set inside
            // `enter_trayed_deferred`; the window struct itself is freed by
            // the supervisor loop promptly after this close.
            state::WEBVIEW_HWND.store(0, Ordering::SeqCst);
            state::enter_trayed_deferred(window);
            tray::start();
            tray::hide_to_tray();
            return true;
        }
    }
    geometry::remember_webview(window);
    exit::request_shutdown();
    true
}
