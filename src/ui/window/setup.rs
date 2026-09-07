//! Bring a window up: backend decision, vanish watchdog, retry loop — plus
//! the one-time [`setup`] entry point that wires config → first window.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use webui::webui;

use crate::config::{self, UiMode};
use crate::ui::browser;
use crate::ui::exit;
use crate::ui::geometry;
use crate::ui::state;

use super::tracking;
use super::{create_window, remember_launcher_url};

static BROWSER_WATCH_STOP: AtomicBool = AtomicBool::new(false);
/// True once the watchdog saw OUR session browser's process alive at least
/// once. Classification depends on it:
/// * seen, then gone → the user closed the browser (honour the exit);
/// * never seen      → the webui server never came up (ephemeral-port
///   collision et al.) and no browser was ever spawned — retryable.
static BROWSER_WATCH_SAW: AtomicBool = AtomicBool::new(false);
/// Set together with an interrupting `webui::exit()` when a SEEN browser
/// disappears mid-wait.
static SPAWNED_BROWSER_GONE: AtomicBool = AtomicBool::new(false);

pub(crate) fn browser_watch_saw_browser() -> bool {
    BROWSER_WATCH_SAW.load(Ordering::SeqCst)
}

/// Create and show the launcher window on the CONFIGURED backend. The choice
/// is absolute — there is deliberately NO cross-backend fallback: a
/// browser-mode failure must never surprise the user with a WebView (and
/// vice versa). Failure returns `false`; [`setup`] aborts startup on it, and
/// runtime rebuilds ([`super::restore_from_tray`]) roll back to the tray.
///
/// Browser mode runs the vanish watchdog around the blocking `show()`:
/// webui waits up to ~15 s INSIDE this call for a WebSocket connection while
/// the supervisor loop is not running yet, so nothing else could react to an
/// early user close. The watchdog polls the spawned browser's process tree
/// every 300 ms and interrupts the wait via `webui::exit()` (documented
/// thread-safe) only where we are about to tear the whole process down
/// anyway (`allow_interrupt`, derived from `allow_fallback`).
///
/// Returns whether a window is really shown. On `false` the caller tears the
/// (possibly partially-built) window down / rolls back to the tray.
pub(super) fn show_window(
    prefer_browser: bool,
    navigate_back: Option<String>,
    initial_launch: bool,
) -> bool {
    let window = create_window();

    // Restore the last window position/size before showing it, for both
    // backends. webui only accepts size 100..=3840 x 100..=2160 and position
    // 0..=3000 / 0..=1800; anything outside is silently dropped, so clamp.
    // Saved values are physical pixels (this process is DPI-aware); external
    // browsers interpret --window-position/--window-size in logical pixels
    // (DIPs), so they are divided by the DPI scale first. webui reads these
    // during show(), so this MUST run before it.
    let apply_geometry =
        |window: &webui::Window, to_browser: bool| geometry::apply(window, to_browser);
    apply_geometry(&window, prefer_browser);

    // Browser-mode watchdog for the blocking show(). Probe needle is the
    // SESSION-UNIQUE `--app=http://localhost:<port>` command line — NEVER a
    // profile-dir match: webui hands every webui.me app the SAME shared
    // `.WebUI` profile, so a profile-based probe would also see (and reap!)
    // other apps' browsers.
    //
    // Interrupts (`webui::exit()`) are gated on initial_launch: setup aborts
    // the whole process on total failure, so poisoning webui there is
    // irrelevant; a runtime rebuild (tray restore) must keep webui usable
    // for every later restore, so it classifies silently and waits out the
    // natural timeout.
    let allow_interrupt = initial_launch;
    let allow_fallback = initial_launch;
    if prefer_browser {
        let window_id = window.id;
        std::thread::spawn(move || {
            let window = webui::Window::from_id(window_id);

            // The server assigns its port as soon as it starts listening,
            // before any browser exists — wait for it, then watch the exact
            // per-session cmdline.
            let mut port = 0usize;
            for _ in 0..20 {
                if BROWSER_WATCH_STOP.load(Ordering::SeqCst)
                    || state::SHUTDOWN_REQUESTED.load(Ordering::SeqCst)
                {
                    return;
                }
                port = window.get_port();
                if port != 0 {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(300));
            }
            if port == 0 {
                return; // server never started; show() will fail on its own
            }
            let needle = format!("--app=http://localhost:{port}");

            let mut misses = 0u32;
            for _ in 0..100 {
                if BROWSER_WATCH_STOP.load(Ordering::SeqCst)
                    || state::SHUTDOWN_REQUESTED.load(Ordering::SeqCst)
                {
                    return;
                }
                if crate::platform::find_process_by_cmdline(&needle).is_some() {
                    BROWSER_WATCH_SAW.store(true, Ordering::SeqCst);
                    misses = 0;
                } else if BROWSER_WATCH_SAW.load(Ordering::SeqCst) {
                    misses += 1;
                    if misses >= 2 {
                        SPAWNED_BROWSER_GONE.store(true, Ordering::SeqCst);
                        crate::debug::emit(
                            "session browser vanished during startup wait (2 consecutive misses)",
                        );
                        if allow_interrupt {
                            crate::debug::emit("interrupting the blocked show via webui::exit()");
                            webui::exit();
                        }
                        return;
                    }
                }
                std::thread::sleep(std::time::Duration::from_millis(300));
            }
        });
    }

    // `IS_BROWSER` is published by the caller ONLY when `shown` is true: a
    // failed show must leave no backend claim behind (the supervisor reads
    // it and would misjudge a window that does not exist).
    let (shown, actual_browser, window) = if prefer_browser {
        crate::debug::emit("show_window: calling show (browser mode)");
        let mut ok = window.show("index.html");
        BROWSER_WATCH_STOP.store(true, Ordering::SeqCst);
        if SPAWNED_BROWSER_GONE.load(Ordering::SeqCst) {
            crate::debug::emit("session browser closed during startup wait");
            ok = false;
        }
        if !ok {
            // A failed browser show means "no browser window came up" —
            // clean up and report failure so the caller exits/trays, instead
            // of silently switching backends. Reap OUR session instance only:
            // the port needle is unique to this launch, so this can never
            // touch another webui.me app's browser (no-op once user closed).
            crate::debug::emit("browser window did not come up; cleaning up");
            let port = window.get_port();
            if port != 0 {
                let needle = format!("--app=http://localhost:{port}");
                std::thread::spawn(move || {
                    if let Some(pid) = crate::platform::find_process_by_cmdline(&needle) {
                        crate::debug::emit(&format!(
                            "killing leftover browser from the failed show (pid {pid})"
                        ));
                        crate::platform::kill_tree(pid);
                    }
                });
            }
        }
        (ok, true, window)
    } else {
        crate::debug::emit("show_window: calling show_wv (webview mode)");
        let ok = window.show_wv("index.html");
        if !ok && allow_fallback {
            crate::debug::emit("WebView unavailable, falling back to an external browser");
            let old_id = window.id;
            state::WINDOW_ID.store(0, Ordering::SeqCst);
            let window = create_window();
            apply_geometry(&window, true);
            let ok = window.show("index.html");
            std::thread::spawn(move || webui::destroy(old_id));
            (ok, true, window)
        } else {
            if !ok {
                crate::debug::emit("embedded WebView did not come up");
            }
            (ok, false, window)
        }
    };

    if !shown {
        return false;
    }

    // The window is really up: publish the decided backend and run the steps
    // that depend on a live window.
    state::IS_BROWSER.store(actual_browser, Ordering::SeqCst);
    if actual_browser {
        browser::note_window_shown();

        // Navigate once a client is connected: webui drops a navigate fired
        // before any client connected, and after navigating to dsh the webui
        // socket is gone. Deliberately NO geometry re-apply here — Chromium
        // owns this window's placement memory and re-applies it right after
        // creation; a late resizeTo from us would only produce a visible
        // size jump. The pre-show --window-size covers the fresh-profile
        // case; afterwards Chromium's own memory restores what the user left.
        let id = window.id;
        std::thread::spawn(move || {
            let mut connected = false;
            for _ in 0..200 {
                if state::SHUTDOWN_REQUESTED.load(Ordering::SeqCst) {
                    return;
                }
                if webui::is_shown(id) {
                    connected = true;
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            if !connected {
                crate::debug::emit("browser connect wait elapsed; navigate may be dropped");
            }
            if let Some(url) = navigate_back {
                browser::note_navigated_to_dsh();
                webui::navigate(id, url.as_str());
            }
        });
        tracking::capture_browser_pid();
    } else {
        // WebView mode: hold a keep-alive WebSocket (see `wskeep`) so the
        // window stays open after navigating to dsh — the navigation
        // disconnects the embedded WebView from webui's server, and without
        // a live client webui stops the server and closes the WebView
        // ~1.5 s later (WEBUI_RELOAD_TIMEOUT).
        let port = window.get_port();
        crate::debug::emit(&format!("webui server port {port}"));
        if port != 0 {
            *state::KEEPALIVE.lock().unwrap() = Some(crate::wskeep::spawn(port as u16));
        }
        super::apply_window_theme_async();
        if let Some(url) = navigate_back {
            browser::note_navigated_to_dsh();
            window.navigate(&url);
        }
    }
    remember_launcher_url();
    true
}

/// Create the window, register the file handler and bindings, and show it.
pub fn setup(cli_config_path: Option<PathBuf>) {
    *state::CLI_CONFIG_PATH.lock().unwrap() = cli_config_path.clone();

    // Start the control plane before any dsh can be spawned, so
    // DSHL_CONTROL_URL is ready when the dsh command is built. Best-effort.
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

    // The configured backend is absolute — see show_window.
    let prefer_browser = match mode {
        UiMode::Browser => true,
        UiMode::Webview => false,
    };

    // Failure classification drives retry vs abort:
    // * watchdog never SAW a browser → webui's own server failed to bind
    //   (ephemeral-port collision: get_free_port tests-then-releases, and
    //   something else claimed the port before the real bind — error 10048).
    //   No browser was spawned, nothing user-visible happened: reset webui
    //   completely (clears its exit flag so the next attempt works) and
    //   retry with a fresh port, up to 2 extra attempts.
    // * browser was SEEN and then vanished → the user closed it: abort.
    let mut shown = show_window(prefer_browser, None, true);
    let mut attempt = 1u32;
    while !shown && attempt < 3 && !browser_watch_saw_browser() {
        attempt += 1;
        crate::debug::emit(&format!(
            "show attempt {attempt}: browser never spawned (server/port failure); \
             resetting webui and retrying"
        ));
        webui::exit();
        webui::clean();
        std::thread::sleep(std::time::Duration::from_millis(300));
        shown = show_window(prefer_browser, None, true);
    }

    if !shown {
        // Both backends failed, or the user closed the spawned browser during
        // its startup wait. Either way the launcher must NOT continue into a
        // headless dsh launch: raising SHUTDOWN_REQUESTED here makes every
        // launch-worker checkpoint abort (idempotent — nothing is running
        // yet).
        crate::debug::emit("window failed to open; aborting startup");
        exit::request_shutdown();
    }

    // The window is up; close requests are safe to handle now. If the user
    // closed it during creation, apply that deferred close immediately.
    state::SETUP_DONE.store(true, Ordering::SeqCst);
    if state::CLOSE_PENDING.swap(false, Ordering::SeqCst) {
        crate::debug::emit("applying deferred close from setup");
        exit::request_shutdown();
    }
}
