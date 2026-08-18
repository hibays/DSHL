//! Launcher window lifecycle: creation, setup, close handling, geometry
//! persistence, theme watching and window-handle tracking.
//!
//! This module owns everything about *the window itself*; the launch flow
//! ([`super::launch`]) and the event loop ([`super::supervisor`]) talk to it
//! through a few narrow entry points ([`setup`], [`restore_from_tray`],
//! [`navigate`], [`capture_webview_hwnd`]).

use std::path::PathBuf;
use std::sync::atomic::Ordering;

use webui::webui;

use super::assets;
use super::bindings;
use super::exit;
use super::state;
use super::vfs;
use crate::config::{self, UiMode};
use crate::progress;
use crate::tray;

/// WebView close handler.
///
/// Normally: remember the window geometry (unless maximized/fullscreen), then
/// ask the event loop to kill dsh and shut down, returning `true` to allow the
/// close. If the window is closed while [`setup`] is still creating the
/// WebView2 (`show_wv` in progress), tearing it down now deadlocks webui's own
/// show wait loop, so we instead defer the close (return `false`) and re-apply
/// it once setup finishes.
unsafe extern "C" fn on_webview_close(window: usize) -> bool {
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
        remember_window_geometry(window);
        let hwnd = webui::Window::from_id(window).get_hwnd() as usize;
        // Windows needs the WebView HWND as the tray anchor sanity check;
        // other platforms have no HWND concept (get_hwnd returns 0) yet
        // still want the close to hand over to the tray.
        if hwnd != 0 || !cfg!(target_os = "windows") {
            // Stop this window's keep-alive so its webui server can shut down
            // (the server keeps running while any client is connected). The
            // window struct itself is freed by the supervisor loop promptly
            // after this close (see `state::PENDING_DESTROY`), so it is not
            // held in memory while trayed.
            if let Some(keepalive) = state::KEEPALIVE.lock().unwrap().take() {
                keepalive.stop();
            }
            tray::start();
            tray::hide_to_tray();
            // The window is destroyed below; clear the tracked HWND so the
            // supervisor loop does not mistake the stale handle for a live
            // window (which would re-trigger tray mode or shutdown), and
            // enter tray mode right here.
            state::WEBVIEW_HWND.store(0, Ordering::SeqCst);
            // Defer the webui struct/server cleanup to the supervisor loop
            // (never call webui_destroy from inside the close handler — it
            // would free the window while webui is mid-close).
            state::PENDING_DESTROY.store(window, Ordering::SeqCst);
            state::TRAYED.store(true, Ordering::SeqCst);
            crate::debug::emit("close-to-tray: window closed, dsh keeps running");
            return true;
        }
    }
    remember_window_geometry(window);
    exit::request_shutdown();
    true
}

/// Persisted window geometry.
#[derive(serde::Serialize, serde::Deserialize)]
struct WindowState {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
}

fn window_state_path() -> PathBuf {
    crate::platform::cache_dir()
        .join("dshl")
        .join("window-state.json")
}

fn load_window_state() -> Option<WindowState> {
    let text = std::fs::read_to_string(window_state_path()).ok()?;
    let state: WindowState = serde_json::from_str(&text).ok()?;
    // Guard against corrupt/hand-edited values that would make webui create
    // an absurdly large (or degenerate) window at startup.
    if state.width < 200 || state.width > 10_000 || state.height < 150 || state.height > 10_000 {
        return None;
    }
    Some(state)
}

/// Capture the WebView window geometry and persist it, skipping maximized /
/// fullscreen states so those are never restored.
/// Clamp a saved window geometry to something sane for the current screen.
///
/// Upper bounds are the smaller of the webui hard limits and the actual
/// screen resolution (so a window saved on a 4K screen does not exceed a
/// 1080p screen, and position never pushes the window off-screen). Returns
/// `(width, height, x, y)` in physical pixels, ready for `set_size`/
/// `set_position`.
fn clamp_geometry(state: &WindowState) -> (u32, u32, u32, u32) {
    // webui *enforces* these limits in its C source (webui.c):
    //   #define WEBUI_MAX_WIDTH (3840) / WEBUI_MAX_HEIGHT (2160)
    //   #define WEBUI_MAX_X (3000) / WEBUI_MAX_Y (1800)
    // and `webui_set_size`/`webui_set_position` silently return (refusing
    // the value) when the input falls outside — the window then keeps its
    // default size. So clamping here is not cosmetic: values outside these
    // ranges are simply dropped by webui. We additionally clamp to the real
    // screen size so a geometry saved on a bigger monitor still fits.
    let (sw, sh) = crate::platform::screen_size();
    let max_w = if sw > 0 { sw.min(3840) } else { 3840 };
    let max_h = if sh > 0 { sh.min(2160) } else { 2160 };
    let w = state.width.clamp(100, max_w);
    let h = state.height.clamp(100, max_h);
    // Keep the top-left corner on screen: x/y must leave room for the
    // window itself (and never go negative).
    let x_max = (max_w as i32 - w as i32).max(0);
    let y_max = (max_h as i32 - h as i32).max(0);
    let x = state.x.clamp(0, x_max) as u32;
    let y = state.y.clamp(0, y_max) as u32;
    (w, h, x, y)
}

fn remember_window_geometry(window: usize) {
    let hwnd = webui::Window::from_id(window).get_hwnd() as usize;
    crate::debug::emit(&format!("remember geometry: hwnd {hwnd:#x}"));
    if hwnd == 0 {
        return;
    }
    let Some(rect) = crate::platform::window_rect(hwnd) else {
        crate::debug::emit("remember geometry: window_rect returned None");
        return;
    };
    crate::debug::emit(&format!(
        "remember geometry: {}x{} @ ({},{}) maximized={}",
        rect.width, rect.height, rect.x, rect.y, rect.maximized
    ));
    if rect.maximized {
        // Don't persist a maximized / fullscreen geometry.
        return;
    }
    if rect.width < 200 || rect.height < 150 {
        // Ignore degenerate sizes.
        return;
    }
    persist_window_state(rect.x, rect.y, rect.width, rect.height);
}

/// Write the persisted window geometry (skipping degenerate values).
fn persist_window_state(x: i32, y: i32, width: u32, height: u32) {
    if width < 200 || height < 150 {
        return;
    }
    let state = WindowState {
        x,
        y,
        width,
        height,
    };
    let path = window_state_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string(&state) {
        match std::fs::write(&path, json) {
            Ok(()) => crate::debug::emit(&format!("persist geometry: wrote {}", path.display())),
            Err(e) => crate::debug::emit(&format!("persist geometry: write failed: {e}")),
        }
    }
}

/// Capture the external browser's main process id (best-effort, background).
///
/// The browser is launched by webui with `--app=http://localhost:<port>` in
/// its command line, so we locate it by the window's server port. Once the
/// pid is known, its window geometry is sampled continuously and persisted
/// (webui has no browser-side close handler to hook, so we poll instead).
fn capture_browser_pid() {
    std::thread::spawn(|| {
        let window = webui::Window::from_id(state::WINDOW_ID.load(Ordering::SeqCst));

        // Wait for the window server to assign its port.
        let mut port = 0usize;
        for _ in 0..40 {
            port = window.get_port();
            if port != 0 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
        if port == 0 {
            crate::debug::emit("window server port never assigned");
            return;
        }

        let needle = format!("--app=http://localhost:{port}");
        for _ in 0..10 {
            if let Some(pid) = crate::platform::find_process_by_cmdline(&needle) {
                state::BROWSER_PID.store(pid as usize, Ordering::SeqCst);
                crate::debug::emit(&format!("browser window pid {pid} (port {port})"));
                track_browser_geometry(pid);
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
        crate::debug::emit("failed to locate the browser window pid");
    });
}

/// Sample the external browser window's geometry every second and persist it,
/// so the next launch restores the position/size the user last left the
/// window in. Exits when the browser process dies (the last sample is already
/// on disk by then). Best-effort: only implemented on Windows.
fn track_browser_geometry(pid: u32) {
    let mut last: Option<(i32, i32, u32, u32)> = None;
    loop {
        if !crate::platform::process_alive(pid) {
            crate::debug::emit("track geometry: browser process exited");
            return;
        }
        if let Some(hwnd) = crate::platform::find_hwnd_by_pid(pid)
            && let Some(rect) = crate::platform::window_rect(hwnd)
            && !rect.maximized
        {
            let key = (rect.x, rect.y, rect.width, rect.height);
            if last != Some(key) {
                last = Some(key);
                persist_window_state(rect.x, rect.y, rect.width, rect.height);
                crate::debug::emit(&format!(
                    "track geometry: {}x{} @ ({},{})",
                    rect.width, rect.height, rect.x, rect.y
                ));
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(1000));
    }
}

/// Create a launcher window with the file handler, close handler and all
/// bindings registered. Used both at startup ([`setup`]) and when a window
/// is re-created after being closed to the tray (webui cannot revive a
/// closed window, so restore builds a fresh one).
fn create_window() -> webui::Window {
    let window = webui::Window::new();
    state::WINDOW_ID.store(window.id, Ordering::SeqCst);
    window.set_file_handler(vfs::vfs);
    window.set_close_handler_wv(on_webview_close);
    // Favicon served to the page (and the browser tab in browser mode).
    window.set_icon(assets::LOGO_SVG, "image/svg+xml");
    bindings::register(&window);
    window
}

/// Create the window, register the file handler and bindings, and show it.
pub fn setup(cli_config_path: Option<PathBuf>) {
    *state::CLI_CONFIG_PATH.lock().unwrap() = cli_config_path.clone();

    // Read the configured UI mode and close-to-tray preference
    // (loads/generates dshl.toml if absent).
    let ui = config::load(cli_config_path.as_deref()).config.ui;
    let mode = ui.mode;
    state::CLOSE_TO_TRAY.store(ui.close_to_tray, Ordering::SeqCst);

    // Process UI events one at a time (our callbacks touch shared state).
    webui::set_config(webui::Config::UiEventBlocking, true);
    // Allow the keep-alive WebSocket (launcher) to coexist with the window's
    // client, and don't require the `webui_auth` cookie for it.
    webui::set_config(webui::Config::MultiClient, true);
    webui::set_config(webui::Config::UseCookies, false);

    let window = create_window();

    // The mode is a *preference*: try the preferred backend first and fall back
    // to the other one when it fails.
    let prefer_webview = match mode {
        UiMode::Browser => false,
        UiMode::Webview => true,
    };

    // Restore the last window position/size (before showing it), for both
    // backends. webui only accepts size 100..=3840 x 100..=2160 and position
    // 0..=3000 / 0..=1800; anything outside is silently dropped, so clamp.
    // The saved state is in physical pixels (captured by this DPI-aware
    // process), which is what WebView windows expect directly; external
    // browsers interpret `--window-position/--window-size` in logical pixels
    // (DIPs), so those values are divided by the DPI scale first.
    if let Some(state) = load_window_state() {
        let (w, h, x, y) = clamp_geometry(&state);
        // External browsers interpret `--window-position/--window-size` in
        // logical pixels (DIPs), while WebView windows expect physical
        // pixels; the saved state is physical, so divide for browser mode.
        let scale = crate::platform::dpi_scale();
        if !prefer_webview && scale > 0.0 {
            window.set_size(
                (w as f64 / scale).round() as u32,
                (h as f64 / scale).round() as u32,
            );
            window.set_position(
                (x as f64 / scale).round() as u32,
                (y as f64 / scale).round() as u32,
            );
        } else {
            window.set_size(w, h);
            window.set_position(x, y);
        }
    }

    let shown = if prefer_webview {
        crate::debug::emit("setup: calling show_wv");
        let ok = window.show_wv("index.html");
        crate::debug::emit(&format!("setup: show_wv returned {ok}"));
        if ok {
            state::IS_BROWSER.store(false, Ordering::SeqCst);
            true
        } else {
            crate::debug::emit("WebView unavailable, falling back to an external browser");
            state::IS_BROWSER.store(true, Ordering::SeqCst);
            window.show("index.html")
        }
    } else {
        let ok = window.show("index.html");
        if ok {
            state::IS_BROWSER.store(true, Ordering::SeqCst);
            true
        } else {
            crate::debug::emit("browser unavailable, falling back to the embedded WebView");
            state::IS_BROWSER.store(false, Ordering::SeqCst);
            window.show_wv("index.html")
        }
    };

    if shown {
        if state::IS_BROWSER.load(Ordering::SeqCst) {
            capture_browser_pid();
        } else {
            // Hold a keep-alive WebSocket (see `wskeep`) so the window stays
            // open after navigating to dsh: the navigation disconnects the
            // embedded WebView from webui's server, and without a live client
            // webui stops the server and closes the WebView ~1.5s later
            // (WEBUI_RELOAD_TIMEOUT). Browser mode needs no keep-alive — webui
            // does not terminate the external browser when its server stops,
            // and the launcher tracks the browser process directly.
            let port = window.get_port();
            crate::debug::emit(&format!("webui server port {port}"));
            if port != 0 {
                *state::KEEPALIVE.lock().unwrap() = Some(crate::wskeep::spawn(port as u16));
            }
        }
    }
    remember_launcher_url();
    if !shown {
        crate::debug::emit("window failed to open");
    }

    // Windows only: make the titlebar follow the OS dark mode (Win32 windows
    // stay light until they opt in via DWMWA_USE_IMMERSIVE_DARK_MODE) and swap
    // in the white "night" window icon on dark themes. The HWND may not be
    // valid the instant `show_wv` returns, so poll for it in the background.
    if shown && !state::IS_BROWSER.load(Ordering::SeqCst) {
        apply_window_theme_async();
    }

    // The window is up; close requests are safe to handle now. If the user
    // closed it during creation, apply that deferred close immediately.
    state::SETUP_DONE.store(true, Ordering::SeqCst);
    if state::CLOSE_PENDING.swap(false, Ordering::SeqCst) {
        crate::debug::emit("applying deferred close from setup");
        exit::request_shutdown();
    }
}

/// Navigate the webui window to the dsh URL.
pub(crate) fn navigate(url: &str) {
    let id = state::WINDOW_ID.load(Ordering::SeqCst);
    webui::navigate(id, url);
}

/// Record the launcher page URL (served by webui's own server) so crash
/// recovery can navigate the window back to the startup page.
fn remember_launcher_url() {
    let window = webui::Window::from_id(state::WINDOW_ID.load(Ordering::SeqCst));
    let port = window.get_port();
    if port != 0 {
        let url = format!("http://localhost:{port}/index.html");
        *state::LAUNCHER_URL.lock().unwrap() = url;
        crate::debug::emit(&format!(
            "launcher page url: http://localhost:{port}/index.html"
        ));
    }
}

/// Navigate the window back to the launcher (startup) page — used when dsh
/// exits unexpectedly so the crash-recovery banner can be shown.
pub(crate) fn navigate_to_launcher() {
    let url = state::LAUNCHER_URL.lock().unwrap().clone();
    crate::debug::emit(&format!("navigate back to launcher page ({url})"));
    if !url.is_empty() {
        navigate(&url);
    }
}

/// Apply the OS dark-mode look (titlebar + matching day/night window icon) to
/// the embedded WebView window, then keep following the system theme while
/// the window is alive.
///
/// Windows has no *automatic* dark titlebar for plain Win32 windows — the
/// browsers look native only because they call the same
/// `DWMWA_USE_IMMERSIVE_DARK_MODE` attribute right at window creation and
/// re-apply it when the OS theme changes (`WM_SETTINGCHANGE`). webui exposes
/// neither a hook before `show_wv` creates the window nor a WndProc hook,
/// so we approximate: poll for the HWND the moment the window exists (10ms
/// steps), apply the DWM attribute immediately, re-apply once after WebView2
/// has finished attaching (it can reset the chrome mid-init), and poll the
/// theme registry every second to re-apply on system theme changes.
fn apply_window_theme_async() {
    std::thread::spawn(|| {
        let window = webui::Window::from_id(state::WINDOW_ID.load(Ordering::SeqCst));
        // Day/night window icon variants (see `apply_window_theme`).
        let black_icon: &[u8] = include_bytes!("../../packing/windows/dsh.ico");
        let white_icon: &[u8] = include_bytes!("../../packing/windows/dsh-white.ico");

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
                // Keep the tray icon in sync with the OS theme.
                tray::set_icon(now_dark);
            }
        }
    });
}

/// Capture the embedded WebView window handle (best-effort, background) so the
/// supervisor can detect when the window is actually destroyed.
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

/// Apply the saved window geometry (see the setup-time comment for the
/// physical-vs-logical pixel difference). The actual backend is known here
/// (`state::IS_BROWSER`), so external browsers get the DPI-divided values
/// they interpret in logical pixels, WebView windows get physical pixels.
fn apply_saved_geometry(window: &webui::Window) {
    if let Some(saved) = load_window_state() {
        let (w, h, x, y) = clamp_geometry(&saved);
        if state::IS_BROWSER.load(Ordering::SeqCst) {
            let scale = crate::platform::dpi_scale();
            if scale > 0.0 {
                window.set_size(
                    (w as f64 / scale).round() as u32,
                    (h as f64 / scale).round() as u32,
                );
                window.set_position(
                    (x as f64 / scale).round() as u32,
                    (y as f64 / scale).round() as u32,
                );
                return;
            }
        }
        window.set_size(w, h);
        window.set_position(x, y);
    }
}

/// Re-create the launcher window after it was closed to tray: re-apply the
/// saved geometry, show the window (WebView or external browser, matching the
/// mode), navigate back to dsh, and re-capture the HWND / browser pid. Shared
/// by the tray "restore" menu and single-instance activation. With
/// `show_launcher` the window shows the startup page instead of dsh (crash
/// recovery).
pub(crate) fn restore_from_tray(show_launcher: bool) {
    // Restore fires only once per request: while the (slow) window rebuild
    // is running, further double-clicks or menu items are ignored. If the
    // rebuild fails, the guard is released so the user can retry.
    if state::RESTORING.swap(true, Ordering::SeqCst) {
        crate::debug::emit("restore: already in progress, ignoring request");
        return;
    }
    crate::debug::emit("restore window from tray");

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

    // webui cannot revive a closed window (show_wv/show on it hangs/fails), so
    // build a brand-new one and re-apply the theme for its HWND.
    let window = create_window();
    apply_saved_geometry(&window);
    let browser_mode = state::IS_BROWSER.load(Ordering::SeqCst);
    let shown = if browser_mode {
        crate::debug::emit("restore: calling show (browser mode)");
        window.show("index.html")
    } else {
        crate::debug::emit("restore: calling show_wv (webview mode)");
        window.show_wv("index.html")
    };

    // With the close handler still deferred, no close could have torn the
    // WebView down. Build out the fresh window and check whether the user
    // closed it in the meantime (deferred) before committing to "visible".
    let mut hwnd = 0usize;
    let mut keep = shown && !state::CLOSE_PENDING.load(Ordering::SeqCst);
    if keep {
        if !show_launcher && let Some(url) = progress::snapshot().url {
            window.navigate(&url);
        }
        // The fresh window has its own server port; record its launcher URL.
        remember_launcher_url();
        if browser_mode {
            // Re-locate the freshly opened browser process so future close
            // detection works (best-effort, async — the new browser is already
            // brought to the foreground by webui itself).
            state::BROWSER_CHECKED.store(false, Ordering::SeqCst);
            capture_browser_pid();
        } else {
            // Capture the new HWND synchronously so the supervisor loop sees a
            // live handle immediately (the async capture would lag behind and
            // the stale-zero HWND could look like "no window").
            hwnd = window.get_hwnd() as usize;
            if hwnd != 0 {
                state::WEBVIEW_HWND.store(hwnd, Ordering::SeqCst);
                apply_window_theme_async();
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
        // The fresh window needs its own keep-alive WebSocket (WebView mode
        // only) or webui closes the WebView ~1.5s after it navigates to dsh.
        if !browser_mode {
            let port = window.get_port();
            if port != 0 {
                *state::KEEPALIVE.lock().unwrap() = Some(crate::wskeep::spawn(port as u16));
            }
        }
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

    // The rebuild did not produce a window that should stay open: `show_wv`
    // failed, or the user closed it during the rebuild (deferred above). Free
    // the fresh window and go back to the tray so the next restore builds a
    // clean one.
    crate::debug::emit("restore: window not kept (failed or closed during rebuild)");
    // Free the fresh window's webui resources (struct/server/port) — it was
    // allocated by `create_window` even when `show_wv` failed.
    webui::destroy(window.id);
    state::WINDOW_ID.store(0, Ordering::SeqCst);
    state::WEBVIEW_HWND.store(0, Ordering::SeqCst);
    state::BROWSER_PID.store(0, Ordering::SeqCst);
    state::TRAYED.store(true, Ordering::SeqCst);
    state::SETUP_DONE.store(true, Ordering::SeqCst);
    state::RESTORING.store(false, Ordering::SeqCst);
}
