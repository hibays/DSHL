//! UI + tray + supervisor control surface.
//!
//! Thin thread-safe shims over the already-public `dshl_core::ui` / `tray` /
//! `platform` entry points. These are the methods the napi wrapper
//! (`dshl-native`) reaches for when JS calls `windowShow()` / `trayHide()`
//! / `shutdown()` / `openTerminal()` etc. — keeping them in `dshl-cli`
//! (rather than having `dshl-native` import `dshl-core` modules directly)
//! gives the addon a single upstream dependency and lets `dshl-core`
//! keep its `ui::state` module crate-private.

/// Show the launcher window (creates it if not yet built, or restores it
/// from the tray). Mirrors the tray "restore" menu behaviour.
///
/// Returns `false` when the show was SKIPPED because a kernel boot is still
/// inside `ui::setup` (bounded wait expired) — callers must surface that
/// instead of reporting success for a click that did nothing.
pub fn window_show() -> bool {
    // A kernel boot holds CLI_LOCK exactly while `ui::setup` runs — the one
    // window in which a second concurrent setup would corrupt webui's
    // single-global state. Taking the SAME lock gives real mutual exclusion
    // (the boot releases it as soon as setup returns); a 10s bounded wait
    // covers slow first-boot WebView2 init, after which we skip rather than
    // risk showing over a wedged boot.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let _boot = loop {
        if let Some(guard) = crate::run::try_kernel_lock() {
            break guard;
        }
        if std::time::Instant::now() >= deadline {
            dshl_core::debug::emit(
                "window_show: kernel still inside ui::setup; skipping to avoid a concurrent setup",
            );
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    dshl_core::ui::window_show();
    true
}

/// Hide the launcher window. When close-to-tray is enabled this transitions
/// to the tray (window resources freed, tray icon visible, dsh keeps
/// running). When close-to-tray is disabled this is a no-op: hiding the
/// window without a tray to hand over to is equivalent to quitting, which
/// must be explicit via [`request_shutdown`].
pub fn window_hide() {
    dshl_core::ui::window_hide();
}

/// True iff the launcher window currently exists and is not in the tray.
pub fn window_is_visible() -> bool {
    dshl_core::ui::window_is_visible()
}

/// Navigate the launcher window to an arbitrary URL. Useful after
/// `launch()` if the caller already knows a dsh URL and wants to skip the
/// startup page.
pub fn window_navigate(url: String) {
    dshl_core::ui::window_navigate(&url);
}

pub fn tray_show() {
    dshl_core::tray::start();
}

pub fn tray_hide() {
    dshl_core::tray::shutdown();
}

pub fn tray_set_icon(dark: bool) {
    dshl_core::tray::set_icon(dark);
}

pub fn tray_is_visible() -> bool {
    dshl_core::tray::is_started()
}

pub fn request_shutdown() {
    dshl_core::ui::request_shutdown();
}

pub fn request_restart() {
    dshl_core::ui::request_restart();
}

/// Kill the supervised dsh child without tearing the window/supervisor down.
pub fn kill_dsh() {
    dshl_core::ui::kill_dsh();
}

/// True iff the kernel has finished the startup pipeline and the window is
/// showing (or has navigated to) the real dsh URL.
pub fn is_launched() -> bool {
    dshl_core::ui::is_launched()
}

/// OS shell actions. Mirrored into the shared runner so the napi wrapper can
/// expose them without re-implementing (they're already the authoritative
/// single-copy code in platform/actions.rs).
pub fn open_terminal(cwd: String, path: Option<String>) -> bool {
    let cwd = std::path::PathBuf::from(cwd);
    let path_os = path.as_deref().map(std::ffi::OsStr::new);
    dshl_core::platform::open_terminal(path_os, &cwd).is_ok()
}
pub fn open_path(path: String) -> bool {
    dshl_core::platform::open_path(std::path::Path::new(&path)).is_ok()
}
pub fn open_url(url: String) -> bool {
    dshl_core::platform::open_url(&url).is_ok()
}
