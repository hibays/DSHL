//! Launcher window lifecycle, split by concern.
//!
//! | file          | owns                                                        |
//! |---------------|-------------------------------------------------------------|
//! | [`setup`]     | bring a window up: backend decision, vanish watchdog, retry |
//! | [`tracking`]  | external-browser pid capture + geometry sampler             |
//! | [`navigate`]  | URL navigation (immediate / connect-gated)                  |
//! | [`restore`]   | tray-restore rebuild cycle                                  |
//! | [`controls`]  | public controls: show / hide / focus / visibility           |
//! | [`close`]     | the WebView close-handler policy                            |
//!
//! Shared creation wiring ([`create_window`], launcher-URL bookkeeping,
//! WebView-HWND capture) lives here in `mod.rs`. Everything else talks to
//! this module through the narrow entry points re-exported below.

mod close;
mod controls;
pub(crate) mod navigate;
mod restore;
mod setup;
pub(crate) mod tracking;

use std::sync::atomic::Ordering;

use webui::webui;

use crate::tray;
use crate::ui::{assets, bindings, state, vfs};

pub use close::on_webview_close;
pub use controls::{focus_current, hide, is_visible, show};
pub use navigate::{navigate, navigate_to_launcher, navigate_when_connected};
pub use restore::restore_from_tray;
pub use setup::setup;
pub(crate) use tracking::capture_browser_pid;

/// Create a launcher window with the file handler, close handler and all
/// bindings registered. Used both at startup ([`setup`]) and when a window is
/// re-created after being closed to the tray (webui cannot revive a closed
/// window, so restore builds a fresh one).
pub(super) fn create_window() -> webui::Window {
    let window = webui::Window::new();
    state::WINDOW_ID.store(window.id, Ordering::SeqCst);
    window.set_file_handler(vfs::vfs);
    window.set_close_handler_wv(on_webview_close);
    // Favicon served to the page (and the browser tab in browser mode).
    window.set_icon(assets::LOGO_SVG, "image/svg+xml");
    bindings::register(&window);
    window
}

/// Record the launcher page URL (served by webui's own server) so crash
/// recovery can navigate the window back to the startup page.
pub(super) fn remember_launcher_url() {
    let window = webui::Window::from_id(state::WINDOW_ID.load(Ordering::SeqCst));
    let port = window.get_port();
    if port != 0 {
        let url = format!("http://localhost:{port}/index.html");
        crate::debug::emit(&format!("launcher page url: {url}"));
        *state::LAUNCHER_URL.lock().unwrap() = url;
    }
}

/// Capture the embedded WebView window handle (best-effort, background) so
/// the supervisor can detect when the window is actually destroyed.
pub(crate) fn capture_webview_hwnd() {
    std::thread::spawn(|| {
        let window = webui::Window::from_id(state::WINDOW_ID.load(Ordering::SeqCst));
        for _ in 0..40 {
            let hwnd = window.get_hwnd() as usize;
            if hwnd != 0 {
                state::WEBVIEW_HWND.store(hwnd, Ordering::SeqCst);
                crate::debug::emit(&format!("webview window hwnd {hwnd:#x}"));
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
        crate::debug::emit("failed to capture the webview window handle");
    });
}

/// Apply the OS dark-mode look (titlebar + matching day/night window icon)
/// to the embedded WebView window, then keep following the system theme
/// while the window is alive.
///
/// Windows has no automatic dark titlebar for plain Win32 windows — browsers
/// look native only because they set DWMWA_USE_IMMERSIVE_DARK_MODE at
/// creation and re-apply on WM_SETTINGCHANGE. webui exposes neither a hook
/// before `show_wv` creates the window nor a WndProc hook, so we poll for
/// the HWND, apply immediately, re-apply once WebView2 settles, and watch
/// the theme registry every second. Also keeps the tray icon in sync.
pub(super) fn apply_window_theme_async() {
    std::thread::spawn(|| {
        let window = webui::Window::from_id(state::WINDOW_ID.load(Ordering::SeqCst));
        let black_icon: &[u8] = include_bytes!("../../../packing/windows/dsh.ico");
        let white_icon: &[u8] = include_bytes!("../../../packing/windows/dsh-white.ico");

        // 1. The Win32 window exists once show_wv is back; grab the HWND
        //    immediately (10ms steps, ~10s cap).
        let mut hwnd = 0usize;
        for _ in 0..1000 {
            hwnd = window.get_hwnd() as usize;
            if hwnd != 0 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        if hwnd == 0 {
            crate::debug::emit("apply_window_theme_async: hwnd never became available");
            return;
        }
        crate::debug::emit(&format!("apply_window_theme_async: hwnd {hwnd:#x}"));
        crate::platform::apply_window_theme(hwnd, black_icon, white_icon);

        // 2. WebView2 attaches asynchronously and can reset the window
        //    chrome; one re-apply after it settles keeps the titlebar dark.
        std::thread::sleep(std::time::Duration::from_millis(300));
        if crate::platform::is_window_alive(hwnd) {
            crate::platform::apply_window_theme(hwnd, black_icon, white_icon);
        }

        // 3. Follow the OS theme: poll the Personalize registry values and
        //    re-apply on change (webui gives us no WM_SETTINGCHANGE hook).
        let mut dark = crate::platform::is_dark_mode();
        loop {
            std::thread::sleep(std::time::Duration::from_secs(1));
            if !crate::platform::is_window_alive(hwnd) {
                crate::debug::emit("apply_window_theme_async: window closed, stopping");
                break;
            }
            let now_dark = crate::platform::is_dark_mode();
            if now_dark != dark {
                dark = now_dark;
                crate::debug::emit(&format!(
                    "apply_window_theme_async: system theme changed (dark={now_dark}), re-applying"
                ));
                crate::platform::apply_window_theme(hwnd, black_icon, white_icon);
                tray::set_icon(now_dark);
            }
        }
    });
}
