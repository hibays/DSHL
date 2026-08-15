//! webui.me startup window.
//!
//! The window serves the embedded startup page through a virtual file
//! handler, exposes a few bound functions to the frontend, and is later
//! navigated to the dsh URL once the launcher succeeds.

pub mod assets;

use std::ffi::CStr;
use std::os::raw::c_void;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{LazyLock, Mutex};

use webui::webui;

use crate::config::{self, UiMode};
use crate::flow;
use crate::mirror::MirrorConfig;
use crate::progress;
use crate::runtime;

static WINDOW_ID: AtomicUsize = AtomicUsize::new(0);
static SHOULD_EXIT: AtomicBool = AtomicBool::new(false);
static FLOW_RUNNING: AtomicBool = AtomicBool::new(false);
/// True once dsh is up and the window has been navigated to it (supervisor phase).
static LAUNCHED: AtomicBool = AtomicBool::new(false);
/// Set by the SIGINT/SIGTERM handler (and the WebView close handler).
static SHUTDOWN_REQUESTED: AtomicBool = AtomicBool::new(false);
/// True when the startup window is an external browser (vs. embedded WebView).
static IS_BROWSER: AtomicBool = AtomicBool::new(false);
/// PID of the external browser window process (0 until captured).
static BROWSER_PID: AtomicUsize = AtomicUsize::new(0);
static BROWSER_CHECKED: AtomicBool = AtomicBool::new(false);
/// HWND of the embedded WebView window (0 until captured).
static WEBVIEW_HWND: AtomicUsize = AtomicUsize::new(0);
/// True once [`setup`] has finished creating and showing the window.
static SETUP_DONE: AtomicBool = AtomicBool::new(false);
/// True when the user closed the window while it was still being created.
static CLOSE_PENDING: AtomicBool = AtomicBool::new(false);
/// `close-to-tray` config: closing the window hides to the tray (Windows)
/// or keeps the launcher running without a window (Linux, window re-created
/// on restore).
static CLOSE_TO_TRAY: AtomicBool = AtomicBool::new(false);
/// True when the window is currently hidden/closed in tray mode.
static TRAYED: AtomicBool = AtomicBool::new(false);
/// True while a tray restore (window re-creation) is in progress, so a
/// double-click or menu item during the slow rebuild does not stack requests.
static RESTORING: AtomicBool = AtomicBool::new(false);

static CLI_CONFIG_PATH: LazyLock<Mutex<Option<PathBuf>>> = LazyLock::new(|| Mutex::new(None));
static CONFIG_PATH: LazyLock<Mutex<Option<PathBuf>>> = LazyLock::new(|| Mutex::new(None));
/// PID of a stale dsh that did not exit on Ctrl+C and is awaiting the user's
/// explicit confirmation before being force-killed (0 = none).
static STALE_PID: AtomicU32 = AtomicU32::new(0);

/// Serve the embedded startup assets from memory.
unsafe extern "C" fn vfs(filename: *const i8, length: *mut i32) -> *const c_void {
    // SAFETY: webui passes a valid NUL-terminated path string.
    let name = unsafe { CStr::from_ptr(filename) }.to_str().unwrap_or("");
    let path = name.split('?').next().unwrap_or("");

    let content: Option<(&str, &str)> = match path {
        "/" | "/index.html" | "index.html" => Some((assets::INDEX_HTML, "text/html")),
        "/styles.css" | "styles.css" => Some((assets::STYLES_CSS, "text/css")),
        "/app.js" | "app.js" => Some((assets::APP_JS, "application/javascript")),
        // Theme-aware mark: black by default, white in dark mode.
        "/dsh-black.svg" | "dsh-black.svg" => Some((assets::LOGO_SVG, "image/svg+xml")),
        _ => None,
    };

    if let Some((body, mime)) = content {
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: {mime}\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        // SAFETY: length is either null or a valid i32 pointer per the API.
        return unsafe { webui::malloc(&response, length) };
    }

    // Let webui serve unknown requests (webui.js itself).
    std::ptr::null()
}

fn get_state(e: webui::Event) {
    e.return_string(&progress::to_json());
}

fn exit_app(_e: webui::Event) {
    SHOULD_EXIT.store(true, Ordering::SeqCst);
    webui::exit();
}

fn retry(_e: webui::Event) {
    let stale = STALE_PID.load(Ordering::SeqCst);
    if stale != 0 && crate::platform::process_alive(stale) {
        progress::set_error(format!(
            "残留的 dsh 进程 (pid {stale}) 仍在运行。请点击「强制结束残留进程」结束它，或手动结束后再点重试。"
        ));
        return;
    }
    if stale != 0 {
        STALE_PID.store(0, Ordering::SeqCst);
        progress::set_stale_pid(None);
    }
    launch_flow();
}

/// Force-kill the stale dsh — only runs after the user clicks the dedicated
/// button on the startup page (explicit confirmation). The kill itself is
/// async so the webui thread is not blocked; on success the launch retries.
fn force_kill_stale(_e: webui::Event) {
    let pid = STALE_PID.load(Ordering::SeqCst);
    if pid == 0 {
        return;
    }
    std::thread::spawn(move || {
        crate::debug::emit(&format!(
            "user confirmed force-kill of stale dsh (pid {pid})"
        ));
        crate::platform::kill_tree(pid);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if !crate::platform::process_alive(pid) || std::time::Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        if crate::platform::process_alive(pid) {
            progress::set_error(format!(
                "强制结束失败：pid {pid} 仍然存活，请手动结束该进程。"
            ));
            return;
        }
        STALE_PID.store(0, Ordering::SeqCst);
        progress::set_stale_pid(None);
        progress::log(format!(
            "残留的 dsh 进程 (pid {pid}) 已被强制结束，重新启动…"
        ));
        launch_flow();
    });
}

fn open_config(_e: webui::Event) {
    let path = CONFIG_PATH
        .lock()
        .unwrap()
        .clone()
        .unwrap_or_else(config::default_config_path);
    if !path.exists() {
        let _ = config::write_template(&path);
    }
    let _ = crate::platform::open_path(&path);
}

/// WebView close handler.
///
/// Normally: remember the window geometry (unless maximized/fullscreen), then
/// ask the event loop to kill dsh and shut down, returning `true` to allow the
/// close. If the window is closed while [`setup`] is still creating the WebView2
/// (`show_wv` in progress), tearing it down now deadlocks webui's own show
/// wait loop, so we instead defer the close (return `false`) and re-apply it
/// once setup finishes.
unsafe extern "C" fn on_webview_close(window: usize) -> bool {
    crate::debug::emit(&format!("webview close handler fired (window id {window})"));
    if !SETUP_DONE.load(Ordering::SeqCst) {
        // The window is still being created (`show_wv` is mid-WebView2 init).
        // Letting webui tear the WebView down now deadlocks its own `show_wv`
        // wait loop, so defer the close: we re-apply it once setup finishes.
        CLOSE_PENDING.store(true, Ordering::SeqCst);
        crate::debug::emit("close during setup; deferring");
        return false;
    }
    // close-to-tray: once dsh is up, closing the window lets the WebView
    // (or browser) die for real — its processes exit and memory is freed —
    // while the launcher keeps dsh running in the background. The tray icon
    // re-creates the window on click; quit via the tray menu or Ctrl+C.
    // During startup there is nothing to keep alive, so the close still
    // exits.
    if CLOSE_TO_TRAY.load(Ordering::SeqCst) && LAUNCHED.load(Ordering::SeqCst) {
        remember_window_geometry(window);
        let hwnd = webui::Window::from_id(window).get_hwnd() as usize;
        if hwnd != 0 {
            crate::platform::tray::start();
            crate::platform::tray::hide_to_tray();
            // The window is destroyed below; clear the tracked HWND so the
            // supervisor loop does not mistake the stale handle for a live
            // window (which would re-trigger tray mode or shutdown), and
            // enter tray mode right here.
            WEBVIEW_HWND.store(0, Ordering::SeqCst);
            TRAYED.store(true, Ordering::SeqCst);
            crate::debug::emit("close-to-tray: window closed, dsh keeps running");
            return true;
        }
    }
    remember_window_geometry(window);
    request_shutdown();
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
        let window = webui::Window::from_id(WINDOW_ID.load(Ordering::SeqCst));

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
                BROWSER_PID.store(pid as usize, Ordering::SeqCst);
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
    WINDOW_ID.store(window.id, Ordering::SeqCst);
    window.set_file_handler(vfs);
    window.set_close_handler_wv(on_webview_close);
    // Favicon served to the page (and the browser tab in browser mode).
    window.set_icon(assets::LOGO_SVG, "image/svg+xml");
    window.bind("get_state", get_state);
    window.bind("exit_app", exit_app);
    window.bind("retry", retry);
    window.bind("force_kill_stale", force_kill_stale);
    window.bind("open_config", open_config);
    window
}

/// Create the window, register the file handler and bindings, and show it.
pub fn setup(cli_config_path: Option<PathBuf>) {
    *CLI_CONFIG_PATH.lock().unwrap() = cli_config_path.clone();

    // Read the configured UI mode and close-to-tray preference
    // (loads/generates dshl.toml if absent).
    let ui = config::load(cli_config_path.as_deref()).config.ui;
    let mode = ui.mode;
    CLOSE_TO_TRAY.store(ui.close_to_tray, Ordering::SeqCst);

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
            window.set_size((w as f64 / scale).round() as u32, (h as f64 / scale).round() as u32);
            window.set_position((x as f64 / scale).round() as u32, (y as f64 / scale).round() as u32);
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
            IS_BROWSER.store(false, Ordering::SeqCst);
            true
        } else {
            crate::debug::emit("WebView unavailable, falling back to an external browser");
            IS_BROWSER.store(true, Ordering::SeqCst);
            window.show("index.html")
        }
    } else {
        let ok = window.show("index.html");
        if ok {
            IS_BROWSER.store(true, Ordering::SeqCst);
            true
        } else {
            crate::debug::emit("browser unavailable, falling back to the embedded WebView");
            IS_BROWSER.store(false, Ordering::SeqCst);
            window.show_wv("index.html")
        }
    };

    if shown && IS_BROWSER.load(Ordering::SeqCst) {
        capture_browser_pid();
    } else if shown {
        // WebView mode: hold a keep-alive WebSocket so the window stays open
        // after it navigates to dsh (see `wskeep`).
        let port = window.get_port();
        crate::debug::emit(&format!("webview server port {port}"));
        if port != 0 {
            crate::wskeep::spawn(port as u16);
        }
    }
    if !shown {
        crate::debug::emit("window failed to open");
    }

    // Windows only: make the titlebar follow the OS dark mode (Win32 windows
    // stay light until they opt in via DWMWA_USE_IMMERSIVE_DARK_MODE) and swap
    // in the white "night" window icon on dark themes. The HWND may not be
    // valid the instant `show_wv` returns, so poll for it in the background.
    if shown && !IS_BROWSER.load(Ordering::SeqCst) {
        apply_window_theme_async();
    }

    // The window is up; close requests are safe to handle now. If the user
    // closed it during creation, apply that deferred close immediately.
    SETUP_DONE.store(true, Ordering::SeqCst);
    if CLOSE_PENDING.swap(false, Ordering::SeqCst) {
        crate::debug::emit("applying deferred close from setup");
        request_shutdown();
    }
}

/// (Re)load the config and run the startup pipeline on a worker thread.
pub fn launch_flow() {
    // Only one flow at a time.
    if FLOW_RUNNING.swap(true, Ordering::SeqCst) {
        return;
    }

    // The window was closed while it was being created; don't launch dsh just
    // to tear it down again.
    if SHUTDOWN_REQUESTED.load(Ordering::SeqCst) {
        FLOW_RUNNING.store(false, Ordering::SeqCst);
        return;
    }

    let cli_path = CLI_CONFIG_PATH.lock().unwrap().clone();

    std::thread::spawn(move || {
        // Ask any stale dsh left over from a previous failed attempt to exit
        // via Ctrl+C — the only correct way to close dsh on Windows
        // (AttachConsole + GenerateConsoleCtrlEvent). dsh saves its session
        // log during a Ctrl+C shutdown, and its own shutdown logic force-exits
        // at most 5 seconds after the signal, so a 10s wait covers a healthy
        // shutdown plus that self-timeout with margin; the signal is re-sent
        // every 5s in case the first one was lost.
        //
        // If it is STILL alive after that, the signal never reached it. Never
        // start a new dsh next to it: two processes appending to the same
        // session log produce overlapping seq numbers ("corrupt session log:
        // seq gap"), which is permanent and unrecoverable — the chat history
        // can no longer be loaded. Also do NOT force-kill it silently: a hard
        // kill is destructive (it can interrupt dsh mid-commit of its session
        // log), so it requires the user's explicit confirmation via the
        // dedicated button on the startup page.
        if let Some(child) = crate::DSH_CHILD.lock().unwrap().take() {
            if !child.graceful_kill(10_000) {
                let pid = child.pid().unwrap_or(0);
                STALE_PID.store(pid, Ordering::SeqCst);
                progress::set_stale_pid(Some(pid));
                progress::set_error(format!(
                    "残留的 dsh 进程 (pid {pid}) 未响应 Ctrl+C 退出请求。为避免两个 dsh 同时写入同一会话日志（聊天记录将永久损坏），本次启动已取消。请点击「强制结束残留进程」结束它，或手动结束后再点重试。"
                ));
                FLOW_RUNNING.store(false, Ordering::SeqCst);
                return;
            }
            progress::log("残留的 dsh 进程已退出，继续启动");
        }

        // The cleanup above can take up to 10s; if the user closed the window
        // during it, stop here instead of launching dsh into the void.
        if SHUTDOWN_REQUESTED.load(Ordering::SeqCst) {
            FLOW_RUNNING.store(false, Ordering::SeqCst);
            return;
        }

        let loaded = config::load(cli_path.as_deref());
        *CONFIG_PATH.lock().unwrap() = loaded.path.clone();

        let config_json = serde_json::to_string(&loaded.config).unwrap_or_default();
        let path_str = loaded
            .path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        progress::set_config(config_json, path_str, loaded.parse_error.clone());
        if let Some(err) = &loaded.parse_error {
            progress::log(format!("dshl.toml 解析错误：{err}"));
        }

        let mirror = MirrorConfig::resolve(&loaded.config);

        // Optional single-instance guard: when enabled, refuse to start dsh
        // if any dsh is already running on this machine (started manually or
        // by another dshl). Two processes appending to the same session log
        // produce overlapping seq numbers ("corrupt session log: seq gap") —
        // permanent and unrecoverable, so a hard refusal is the safe choice.
        // The stale-dsh cleanup above has already ensured OUR previous child
        // is gone; this catches everyone else's.
        if loaded.config.dsh.single_instance
            && let Some(pid) = crate::platform::dsh_instance_running()
        {
            progress::set_error(format!(
                "single-instance 已启用：检测到另一个 dsh 实例 (pid {pid}) 正在运行。为避免两个 dsh 同时写入同一会话日志（聊天记录将永久损坏），本次启动已取消。请先关闭现有的 dsh，或把 dshl.toml 的 single-instance 设为 false。"
            ));
            FLOW_RUNNING.store(false, Ordering::SeqCst);
            return;
        }

        match runtime::block_on(flow::run(&loaded.config, &mirror)) {
            Ok(launch) => {
                if SHUTDOWN_REQUESTED.load(Ordering::SeqCst) {
                    // The window was closed while dsh was starting. Don't
                    // touch webui (`navigate`/`get_hwnd`) after the main
                    // thread may already be cleaning up — just leave the
                    // child tracked so `kill_dsh()` reaps it.
                    crate::debug::emit("shutdown requested during launch; skipping navigate");
                } else {
                    // Route the window to dsh and hand off to supervisor mode.
                    navigate(&launch.url);
                    LAUNCHED.store(true, Ordering::SeqCst);

                    // In WebView mode, track the window handle so the
                    // supervisor can detect when the window is destroyed.
                    if !IS_BROWSER.load(Ordering::SeqCst) {
                        capture_webview_hwnd();
                    }

                    // Supervise dsh: drain its output until it exits. When it
                    // exits (or is killed), ask the event loop to shut down.
                    runtime::block_on(flow::launch::supervise(launch.child));
                }
                SHOULD_EXIT.store(true, Ordering::SeqCst);
            }
            Err(_) => {
                // The error is already rendered via progress::set_error. The
                // tracked child (if any) stays so `kill_dsh()` can reap it.
            }
        }

        FLOW_RUNNING.store(false, Ordering::SeqCst);
    });
}

/// Navigate the webui window to the dsh URL.
fn navigate(url: &str) {
    let id = WINDOW_ID.load(Ordering::SeqCst);
    webui::navigate(id, url);
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
        let window = webui::Window::from_id(WINDOW_ID.load(Ordering::SeqCst));
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
                crate::platform::tray::set_icon(now_dark);
            }
        }
    });
}

/// Capture the embedded WebView window handle (best-effort, background) so the
/// supervisor can detect when the window is actually destroyed.
fn capture_webview_hwnd() {
    std::thread::spawn(|| {
        let window = webui::Window::from_id(WINDOW_ID.load(Ordering::SeqCst));
        for _ in 0..40 {
            let hwnd = window.get_hwnd() as usize;
            if hwnd != 0 {
                WEBVIEW_HWND.store(hwnd, Ordering::SeqCst);
                crate::debug::emit(&format!("webview window hwnd {hwnd:#x}"));
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
        crate::debug::emit("failed to capture the webview window handle");
    });
}

/// Re-create the launcher window after it was closed to tray: re-apply the
/// saved geometry, show the WebView, navigate back to dsh, and re-capture
/// the HWND. Shared by the tray "restore" menu and single-instance
/// activation.
fn restore_from_tray() {
    // Restore fires only once per request: while the (slow) window rebuild
    // is running, further double-clicks or menu items are ignored. If the
    // rebuild fails, the guard is released so the user can retry.
    if RESTORING.swap(true, Ordering::SeqCst) {
        crate::debug::emit("restore: already in progress, ignoring request");
        return;
    }
    crate::debug::emit("restore window from tray");
    // webui cannot revive a closed window (show_wv on it hangs/fails), so
    // build a brand-new one and re-apply the theme for its HWND.
    let window = create_window();
    if let Some(state) = load_window_state() {
        let (w, h, x, y) = clamp_geometry(&state);
        window.set_size(w, h);
        window.set_position(x, y);
    }
    if window.show_wv("index.html") {
        if let Some(url) = progress::snapshot().url {
            window.navigate(&url);
        }
        // webui closes a WebView ~1.5s after its bridge disconnects (the
        // WEBUI_RELOAD_TIMEOUT) unless at least one client stays connected.
        // The startup keep-alive belonged to the old window, so this fresh
        // window needs its own WebSocket or it vanishes right after
        // navigating to dsh.
        let port = window.get_port();
        if port != 0 {
            crate::wskeep::spawn(port as u16);
        }
        // Capture the new HWND synchronously so the supervisor loop sees a
        // live handle immediately (the async capture would lag behind and
        // the stale-zero HWND could look like "no window").
        let hwnd = window.get_hwnd() as usize;
        if hwnd != 0 {
            WEBVIEW_HWND.store(hwnd, Ordering::SeqCst);
            apply_window_theme_async();
        }
        TRAYED.store(false, Ordering::SeqCst);
        // Window is live again; allow a future restore cycle (close to tray
        // again and double-click once more).
        RESTORING.store(false, Ordering::SeqCst);
        crate::debug::emit("restore: window re-created");
        // A freshly re-created window is not automatically the foreground
        // window; focus it so single-instance activation and tray restore
        // both bring dsh to the front.
        if hwnd != 0 {
            crate::platform::focus_window(hwnd);
        }
    } else {
        crate::debug::emit("restore: show_wv failed");
        // Release the guard so another double-click/menu item can retry.
        RESTORING.store(false, Ordering::SeqCst);
    }
}

/// Ask the launcher to shut down (called from the SIGINT/SIGTERM handler).
pub fn request_shutdown() {
    SHUTDOWN_REQUESTED.store(true, Ordering::SeqCst);
}

/// Run the webui event loop until shutdown, then clean up dsh and webui.
pub fn run_loop() {
    crate::debug::emit("run_loop: started");

    // close-to-tray enabled: create the tray icon right away, not only on
    // the first window close, so the user knows the launcher lives in the
    // tray (and can quit via its menu) from the very start. Idempotent —
    // the close handler's later start() is a no-op.
    if CLOSE_TO_TRAY.load(Ordering::SeqCst) {
        crate::platform::tray::start();
    }

    loop {
        let alive = webui::wait_async();

        // Success path: the supervisor finished (dsh exited) or an explicit
        // exit was requested.
        if SHOULD_EXIT.load(Ordering::SeqCst) {
            crate::debug::emit("run_loop: SHOULD_EXIT");
            break;
        }
        // Ctrl+C / SIGTERM / WebView window close.
        if SHUTDOWN_REQUESTED.load(Ordering::SeqCst) {
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
        if crate::platform::tray::quit_requested() {
            crate::debug::emit("run_loop: tray quit requested");
            request_shutdown();
        }

        // Startup phase: if the window is gone before dsh was handed off,
        // treat it as "user closed the launcher" and stop.
        if !LAUNCHED.load(Ordering::SeqCst) && !alive {
            crate::debug::emit("run_loop: startup window gone");
            break;
        }

        // Supervisor phase: detect the window that shows dsh going away.
        if LAUNCHED.load(Ordering::SeqCst) {
            if IS_BROWSER.load(Ordering::SeqCst) {
                // Browser mode: track the external browser process. Closing it
                // shuts down and reaps dsh.
                let pid = BROWSER_PID.load(Ordering::SeqCst);
                if pid != 0 {
                    if !BROWSER_CHECKED.swap(true, Ordering::SeqCst) {
                        crate::debug::emit(&format!(
                            "browser supervisor active (pid {pid}, alive={})",
                            crate::platform::process_alive(pid as u32)
                        ));
                    }
                    if !crate::platform::process_alive(pid as u32) {
                        crate::debug::emit("browser window closed; shutting down");
                        request_shutdown();
                    }
                }
            } else {
                // WebView mode: the embedded window shows dsh. Detect the
                // window actually being destroyed (user close, or webui's
                // bridge-drop cleanup) via its HWND, rather than webui's
                // `connected` state (which flips on navigate too).
                let hwnd = WEBVIEW_HWND.load(Ordering::SeqCst);
                #[cfg(windows)]
                let win_gone = hwnd != 0 && !crate::platform::is_window_alive(hwnd);
                #[cfg(not(windows))]
                let win_gone = !alive;
                if win_gone && !TRAYED.load(Ordering::SeqCst) {
                    if CLOSE_TO_TRAY.load(Ordering::SeqCst) {
                        // close-to-tray without the close handler (e.g. Linux,
                        // or the window died another way): keep dsh running.
                        crate::debug::emit("close-to-tray: window gone, dsh keeps running");
                        TRAYED.store(true, Ordering::SeqCst);
                    } else {
                        crate::debug::emit("webview window closed; shutting down");
                        request_shutdown();
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
        if crate::platform::tray::restore_requested() {
            if TRAYED.load(Ordering::SeqCst) {
                restore_from_tray();
            } else {
                crate::debug::emit("restore requested but window visible; focusing instead");
                let hwnd = WEBVIEW_HWND.load(Ordering::SeqCst);
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
            if TRAYED.load(Ordering::SeqCst) {
                restore_from_tray();
            } else {
                let hwnd = WEBVIEW_HWND.load(Ordering::SeqCst);
                if hwnd != 0 {
                    crate::platform::focus_window(hwnd);
                }
            }
        }

        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    crate::platform::tray::shutdown();
    crate::debug::emit("run_loop: calling webui::clean()");
    webui::clean();
    crate::debug::emit("run_loop: webui::clean() returned");
    kill_dsh();
    crate::debug::emit("run_loop: exiting");
}

/// Stop the tracked dsh child via Ctrl+C/SIGTERM and wait for it to exit on
/// its own (up to 30s). Ctrl+C is the correct way to close dsh: it commits
/// its session log during shutdown and its own shutdown logic force-exits at
/// most 5s after the signal, so the generous wait covers it and no force kill
/// ever follows.
pub fn kill_dsh() {
    if let Some(child) = crate::DSH_CHILD.lock().unwrap().take() {
        child.graceful_kill(30_000);
    }
}
