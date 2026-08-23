//! Launcher window lifecycle: creation, setup, close handling, geometry
//! persistence, theme watching and window-handle tracking.
//!
//! This module owns everything about *the window itself*; the launch flow
//! ([`super::launch`]) and the event loop ([`super::supervisor`]) talk to it
//! through a few narrow entry points ([`setup`], [`restore_from_tray`],
//! [`navigate`], [`capture_webview_hwnd`]).

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use webui::webui;

use super::assets;
use super::bindings;
use super::exit;
use super::geometry;
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
        geometry::remember_webview(window);
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
    geometry::remember_webview(window);
    exit::request_shutdown();
    true
}

/// Set while a browser-pid capture is polling, so concurrent triggers (the
/// post-show capture and the supervisor's startup retries) collapse into one
/// poll instead of racing: two finders would each store a pid and each start
/// their own geometry sampler (duplicate sampling, duplicate state writes).
static CAPTURE_IN_PROGRESS: AtomicBool = AtomicBool::new(false);

pub(crate) fn capture_browser_pid() {
    // A capture is already in progress — let it finish.
    if CAPTURE_IN_PROGRESS.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(|| {
        let found = locate_browser_pid();
        // Reset on every path (port never assigned, pid found, polls
        // exhausted, race lost) so later captures stay possible. This runs
        // before the blocking geometry sampler starts.
        CAPTURE_IN_PROGRESS.store(false, Ordering::SeqCst);
        if let Some(pid) = found {
            track_browser_geometry(pid);
        }
    });
}

/// Poll for the external browser window's pid, storing it in
/// [`state::BROWSER_PID`] once found. `None` when the window server port never
/// got assigned, the polls ran out, or another capture stored a pid first.
fn locate_browser_pid() -> Option<u32> {
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
        return None;
    }

    let needle = format!("--app=http://localhost:{port}");
    for _ in 0..10 {
        if let Some(pid) = crate::platform::find_process_by_cmdline(&needle) {
            // Lost the race: another capture already stored a pid (and started
            // its own geometry sampler) — don't store again.
            if super::browser::pid() != 0 {
                return None;
            }
            super::browser::set_pid(pid as usize);
            crate::debug::emit(&format!("browser window pid {pid} (port {port})"));
            return Some(pid);
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    crate::debug::emit("failed to locate the browser window pid");
    None
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
                geometry::persist(rect.x, rect.y, rect.width, rect.height);
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

/// Atomically create and show the launcher window, deciding the actual backend
/// once. This is the *single* show / backend-decision path shared by [`setup`]
/// and [`restore_from_tray`], eliminating the asymmetry where `setup` fell back
/// to the other backend when the preferred one failed but `restore` did not — a
/// WebView that became unavailable after startup (e.g. WebView2 breaking mid-
/// run) could then never be recovered from the tray. Both callers differ only in
/// pre/post bookkeeping that is specific to a first launch vs. a tray re-build;
/// everything about presenting the window lives here.
///
/// `prefer_browser` is the *preference* (config-derived in [`setup`], the last
/// decided backend in [`restore_from_tray`]): the preferred backend is tried
/// first, and on failure the other is used as a fallback. The backend that
/// actually ends up shown is published to `state::IS_BROWSER` — and only when
/// the window is genuinely shown (a failed show leaves the global untouched).
/// `navigate_back`, when `Some`, navigates the freshly-shown window to that URL
/// once it is up (the tray restore returns to dsh this way; `setup` passes
/// `None`). In WebView mode a keep-alive WebSocket is held open so the window
/// survives navigating to dsh; browser mode never spawns one (webui does not
/// terminate the external browser, and the launcher tracks its process).
///
/// Returns whether a window is really shown. On `false` the caller tears the
/// (possibly partially-built) window down / rolls back to the tray.
fn show_window(prefer_browser: bool, navigate_back: Option<String>) -> bool {
    let window = create_window();

    // Restore the last window position/size (before showing it), for both
    // backends. webui only accepts size 100..=3840 x 100..=2160 and position
    // 0..=3000 / 0..=1800; anything outside is silently dropped, so clamp.
    // The saved state is in physical pixels (captured by this DPI-aware
    // process), which is what WebView windows expect directly; external
    // browsers interpret `--window-position/--window-size` in logical pixels
    // (DIPs), so those values are divided by the DPI scale first.
    //
    // `apply_geometry` divides (or not) based on the `to_browser` flag.
    // We call it before show() with the *preferred* backend, then again on
    // the rebuilt window when the actual backend differs (a fallback rebuilds
    // the window — see below) — the unified geometry rule for both callers
    // (one backend decides the pixel interpretation), replacing the two
    // divergent versions setup and restore used to keep in step.
    // Geometry lives in `geometry` now — one shared store for both backends
    // (see module doc). `apply` loads, clamps inside webui's hard limits and
    // divides by the DPI scale for the browser backend.
    let apply_geometry = |window: &webui::Window, to_browser: bool| {
        geometry::apply(window, to_browser);
    };

    // Set geometry for the preferred backend before show() — webui reads
    // win->width/height/x/y during show() to build `--window-size` /
    // `--window-position` (browser) or initialise WebView2 (WebView).
    apply_geometry(&window, prefer_browser);

    // Decide which backend actually ended up being shown. `IS_BROWSER` is
    // deliberately NOT set inside these branches: a failed show would still run
    // that store and leave the global claiming "a browser / WebView is running"
    // while no window exists — the supervisor reads `IS_BROWSER` and would
    // misjudge the runtime (and geometry / keep-alive would run against a
    // window that is not there). Each branch returns `(shown, actual_browser,
    // window)` instead; the decision is published only when `shown` is true.
    // On total failure the pair is `(false, prefer_browser, window)` — the
    // first two values are unused then, and `IS_BROWSER` keeps whatever it
    // held before.
    //
    // A failed preferred show falls back by *rebuilding* the window from
    // scratch rather than re-showing the same webui window object with the
    // other backend: a failed show can tear down that window's server thread
    // as a side effect (observed intermittently when WebView2 is missing —
    // `_webui_make_window_reusable(1)` stops the server and frees the port),
    // so a fallback show on the same object would launch the other backend
    // against an already-dead port and hang until webui's 15s startup timeout
    // reports failure. The stale window still has to be destroyed to free its
    // webui resources (struct / server / port) before `create_window`
    // allocates a fresh one (bindings re-registered, new port, new server)
    // for the fallback show — but `webui::destroy` is itself a synchronous
    // slow operation (it waits for the window's server threads to wind down,
    // seconds in the worst case), so it must not sit in front of the
    // fallback launch: the fresh window is created and shown first, and the
    // dead one is destroyed on a background thread afterwards.
    let (shown, actual_browser, window) = if prefer_browser {
        crate::debug::emit("show_window: calling show (browser mode)");
        let ok = window.show("index.html");
        if ok {
            (true, true, window)
        } else {
            crate::debug::emit("browser unavailable, falling back to the embedded WebView");
            // The failed browser show may have killed this window's server;
            // rebuild before showing the WebView (rationale above).
            let old_id = window.id;
            state::WINDOW_ID.store(0, Ordering::SeqCst);
            let window = create_window();
            // Mirror of the WebView case: the pre-show value was logical
            // pixels (for the browser); convert back to raw physical pixels
            // so WebView2 opens at the intended size directly.
            apply_geometry(&window, false);
            let ok = window.show_wv("index.html");
            // The old window was never shown; destroying it is safe and runs
            // off-thread while the WebView is already coming up.
            std::thread::spawn(move || webui::destroy(old_id));
            (ok, false, window)
        }
    } else {
        crate::debug::emit("show_window: calling show_wv (webview mode)");
        let ok = window.show_wv("index.html");
        if ok {
            (true, false, window)
        } else {
            crate::debug::emit("WebView unavailable, falling back to an external browser");
            // The failed `show_wv` may have taken this window's server down
            // with it; rebuild so the browser does not target a dead port
            // (rationale above).
            let old_id = window.id;
            state::WINDOW_ID.store(0, Ordering::SeqCst);
            let window = create_window();
            // Re-apply geometry for the browser *before* show(): webui reads
            // win->width/height/x/y while building the browser's
            // `--window-size/--window-position`, so the pre-show value (raw
            // physical pixels, for WebView) must be converted to logical
            // pixels first. Otherwise the browser opens oversized and then
            // jumps to the correct size via resizeTo(), which is jarring and
            // can leave a wrong size persisted if the resize does not stick.
            apply_geometry(&window, true);
            let ok = window.show("index.html");
            // The old window was never shown; destroying it is safe and runs
            // off-thread while the browser is already coming up.
            std::thread::spawn(move || webui::destroy(old_id));
            (ok, true, window)
        }
    };

    if shown {
        // The window is really up: only now publish the decided backend, so
        // consumers (supervisor run_loop, tray restore, crash recovery) see a
        // value that matches an actually-existing window, and run the steps
        // that depend on the window being alive.
        state::IS_BROWSER.store(actual_browser, Ordering::SeqCst);
        // Tray restore navigates the fresh window back to dsh here; setup
        // passes `None` and stays on the launcher page.
        if let Some(url) = navigate_back {
            window.navigate(&url);
        }
        if actual_browser {
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
            // Windows only: make the titlebar follow the OS dark mode (Win32
            // windows stay light until they opt in via
            // DWMWA_USE_IMMERSIVE_DARK_MODE) and swap in the white "night"
            // window icon on dark themes. The HWND may not be valid the instant
            // `show_wv` returns, so poll for it in the background.
            apply_window_theme_async();
        }
        remember_launcher_url();
    }
    shown
}

/// Create the window, register the file handler and bindings, and show it.
pub fn setup(cli_config_path: Option<PathBuf>) {
    *state::CLI_CONFIG_PATH.lock().unwrap() = cli_config_path.clone();

    // Start the control plane (the `@dshl/control` plugin contract endpoint)
    // before any dsh can be spawned, so `DSHL_CONTROL_URL` is ready when the
    // dsh command is built. Best-effort: a bind failure only disables remote
    // control, never the launcher.
    if let Err(e) = crate::control::start() {
        crate::debug::emit(&format!("control server failed to start: {e}"));
    }

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

    // The mode is a *preference*: try the preferred backend first and fall back
    // to the other one when it fails. `setup` is the first-class launch that
    // decides the actual backend; every later path (tray restore, crash
    // recovery, show) reuses that decision — published to `state::IS_BROWSER`
    // inside `show_window` only once the window is genuinely shown — instead of
    // re-interpreting the config.
    let prefer_browser = match mode {
        UiMode::Browser => true,
        UiMode::Webview => false,
    };

    // `setup` stays on the launcher page; the launch flow navigates to dsh once
    // it is up.
    let shown = show_window(prefer_browser, None);

    if !shown {
        crate::debug::emit("window failed to open");
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
pub fn navigate(url: &str) {
    let id = state::WINDOW_ID.load(Ordering::SeqCst);
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
    if webui::is_shown(id) {
        webui::navigate(id, url);
    } else {
        // Never connected within the cap. Try anyway (costs nothing), but log
        // so the timeline shows why the page may not have switched.
        crate::debug::emit("navigate: browser never connected; sending navigation anyway");
        webui::navigate(id, url);
    }
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

/// Re-create the launcher window after it was closed to tray: show it (WebView
/// or external browser, matching the running mode with fallback), navigate back
/// to dsh, and re-capture the HWND. Shared by the tray "restore" menu and
/// single-instance activation. With `show_launcher` the window shows the
/// startup page instead of dsh (crash recovery).
pub fn restore_from_tray(show_launcher: bool) {
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

    // Navigate the rebuilt window back to dsh (unless this is the crash
    // recovery path, which shows the launcher page instead). `show_window`
    // re-applies the saved geometry and — crucially — falls back to the other
    // backend if the one running at startup can no longer open, closing the gap
    // where a WebView that died mid-run could never be restored from the tray.
    let navigate_back = if !show_launcher {
        progress::snapshot().url
    } else {
        None
    };
    let browser_mode = state::IS_BROWSER.load(Ordering::SeqCst);
    if browser_mode {
        // The browser is re-opened by `show_window`; mark a fresh probe pending
        // so the supervisor logs the newly located pid once capture lands.
        super::browser::note_window_recreated();
    }
    // `show_window` creates the window, applies geometry, shows with fallback,
    // navigates back, holds the WebView keep-alive and re-captures the browser
    // pid (publishing `IS_BROWSER` only on success).
    let shown = show_window(browser_mode, navigate_back);

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
    super::browser::clear_pid();
    state::TRAYED.store(true, Ordering::SeqCst);
    state::SETUP_DONE.store(true, Ordering::SeqCst);
    state::RESTORING.store(false, Ordering::SeqCst);
}

// ---------------------------------------------------------------------------
// Public high-level controls — the only surface the shared runner (dshl-cli)
// is allowed to touch. Everything below reads / writes `state::*` through
// `super::state` (crate-internal), never crossing the crate boundary.
// ---------------------------------------------------------------------------

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
        super::launch::launch_flow();
    } else {
        // Already visible — just focus.
        let hwnd = state::WEBVIEW_HWND.load(Ordering::SeqCst);
        if hwnd != 0 {
            crate::platform::focus_window(hwnd);
        }
    }
}

/// Hide the launcher window. When `close-to-tray` is enabled this transitions
/// to the tray (window resources freed, tray icon visible, dsh keeps
/// running). When `close-to-tray` is disabled this is a no-op: hiding the
/// window without a tray to hand over to is equivalent to quitting, which
/// must be explicit via [`super::request_shutdown`].
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
        crate::tray::start();
        crate::tray::hide_to_tray();
    }
}

/// True iff the launcher window currently exists and is not in the tray.
pub fn is_visible() -> bool {
    !state::TRAYED.load(Ordering::SeqCst) && state::WINDOW_ID.load(Ordering::SeqCst) != 0
}
