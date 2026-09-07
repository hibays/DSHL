//! URL navigation primitives.

use std::sync::atomic::Ordering;

use webui::webui;

use crate::ui::{browser, state};

/// Navigate the webui window to the dsh URL.
pub fn navigate(url: &str) {
    let id = state::WINDOW_ID.load(Ordering::SeqCst);
    if state::IS_BROWSER.load(Ordering::SeqCst) {
        browser::note_navigated_to_dsh();
    }
    webui::navigate(id, url);
}

/// Navigate the window to `url`, waiting for the UI to actually connect first
/// (browser mode; WebView is always connected by the time `show_wv` returns).
///
/// Why: webui's `webui_navigate()` silently *drops* the navigation packet when
/// no client is connected (webui.c: `if (!_webui_mutex_is_connected(...))
/// return;`). In browser mode an external browser can still be cold-starting
/// when dsh's URL is ready — a slow first launch (AV scan, profile lock,
/// machine under load) easily exceeds the moment `show()` returned. Firing
/// navigate into that gap loses it forever: the browser stays on the launcher
/// page while dsh is already up, and in non-tray mode the supervisor then sits
/// waiting for a browser that never closes — the launcher looks hung.
///
/// The wait is capped at 16s (a bit over webui's own 15s startup timeout): if
/// the browser never connects, navigating is pointless anyway and the
/// supervisor's existing close detection handles the aftermath.
pub fn navigate_when_connected(url: &str) {
    let id = state::WINDOW_ID.load(Ordering::SeqCst);
    if !state::IS_BROWSER.load(Ordering::SeqCst) {
        webui::navigate(id, url);
        return;
    }
    for _ in 0..160 {
        if state::SHUTDOWN_REQUESTED.load(Ordering::SeqCst) {
            crate::debug::emit("navigate: shutdown requested while waiting; dropped");
            return;
        }
        if webui::is_shown(id) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    if state::IS_BROWSER.load(Ordering::SeqCst) {
        browser::note_navigated_to_dsh();
    }
    if webui::is_shown(id) {
        webui::navigate(id, url);
    } else {
        // Never connected within the cap. Try anyway (costs nothing), but log
        // so the timeline shows why the page may not have switched.
        crate::debug::emit("navigate: browser never connected; sending navigation anyway");
        webui::navigate(id, url);
    }
}

/// Navigate the window back to the launcher (startup) page — used when dsh
/// exits unexpectedly so the crash-recovery banner can be shown.
pub fn navigate_to_launcher() {
    let url = state::LAUNCHER_URL.lock().unwrap().clone();
    crate::debug::emit(&format!("navigate back to launcher page ({url})"));
    if !url.is_empty() {
        navigate(&url);
    }
}
