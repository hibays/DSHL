//! macOS status-bar tray: a native `NSStatusItem` via `tray-icon`.
//!
//! Design notes (this is deliberately different from the Windows/Linux
//! backends):
//!
//! - **Main thread only.** AppKit requires the status item to be created on
//!   the main thread. webui's own event loop runs on the main thread and
//!   [`crate::ui`] polls [`quit_requested`]/[`restore_requested`] from
//!   `run_loop`, so [`start`] only records intent and the icon is actually
//!   built on the first poll — which always happens on the main thread.
//!   This sidesteps the AppKit threading constraint without dispatching.
//! - **Template image.** The icon is registered with
//!   `with_icon_as_template(true)`: macOS renders the black alpha-mask image
//!   in the current menu-bar colour automatically, so light/dark themes need
//!   no icon swapping and [`set_icon`] is a no-op (kept for interface
//!   parity with the other platforms).
//! - **Event channels.** Menu clicks arrive through muda's global
//!   [`MenuEvent`] channel and icon clicks through [`TrayIconEvent`]'s. Both
//!   are drained inside the poll functions and folded into the same atomic
//!   flags the Windows/Linux trays expose, so the UI loop logic stays
//!   identical across platforms.
//! - The `TrayIcon` is intentionally leaked (not dropped): dropping it
//!   removes the status item, and the process exits right after `shutdown`
//!   anyway. `TrayIcon` is `!Send` (`Rc`-based), so it cannot live in a
//!   `static` — leaking is the clean way to keep it alive for the process
//!   lifetime.
//!
//! Click behaviour: a single left click restores the window (the launcher's
//! only real action); the menu (恢复窗口 / 退出) opens on right click.

use std::sync::atomic::{AtomicBool, Ordering};

use tray_icon::menu::{Menu, MenuEvent, MenuItem};
use tray_icon::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

/// Fixed menu-item ids (muda), so events can be matched without storing the
/// items themselves.
const MENU_RESTORE: &str = "dshl.restore";
const MENU_QUIT: &str = "dshl.quit";
const MENU_OPEN_URL: &str = "dshl.open_url";

static STARTED: AtomicBool = AtomicBool::new(false);
/// True once the status item exists (allows `start` to be retried after a
/// failure).
static BUILT: AtomicBool = AtomicBool::new(false);
static QUIT_REQUESTED: AtomicBool = AtomicBool::new(false);
static RESTORE_REQUESTED: AtomicBool = AtomicBool::new(false);
static OPEN_URL_REQUESTED: AtomicBool = AtomicBool::new(false);

/// Record intent only — the real creation happens on the main thread on the
/// next poll (see module docs). Idempotent.
pub fn start() {
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    crate::debug::emit("tray: start requested (macos; icon built on next main-thread poll)");
}

/// Build the `NSStatusItem` + menu. MUST run on the main thread (it does:
/// called from the `ui::run_loop` polls via [`poll`]).
fn ensure_started() {
    if BUILT.load(Ordering::SeqCst) {
        return;
    }
    let Some(icon) = build_icon() else {
        crate::debug::emit("tray: failed to decode tray icon");
        STARTED.store(false, Ordering::SeqCst);
        return;
    };
    let restore = MenuItem::with_id(MENU_RESTORE, "恢复窗口", true, None);
    let open_dsh = MenuItem::with_id(MENU_OPEN_URL, "打开 dsh", true, None);
    let quit = MenuItem::with_id(MENU_QUIT, "退出", true, None);
    let menu = Menu::new();
    if menu.append_items(&[&restore, &open_dsh, &quit]).is_err() {
        crate::debug::emit("tray: failed to build menu");
        STARTED.store(false, Ordering::SeqCst);
        return;
    }
    match TrayIconBuilder::new()
        .with_icon(icon)
        // Black + alpha as an NSImage template: macOS colours it to match
        // the menu bar in light AND dark mode automatically.
        .with_icon_as_template(true)
        .with_tooltip("DSHL · DeepSeek Harness")
        .with_menu(Box::new(menu))
        // Left click restores the window; right click opens the menu.
        .with_menu_on_left_click(false)
        .build()
    {
        Ok(tray) => {
            // Leak intentionally: the icon must live for the process
            // lifetime (dropping removes it), and `TrayIcon` is `!Send` so
            // it cannot live in a static.
            std::mem::forget(tray);
            BUILT.store(true, Ordering::SeqCst);
            crate::debug::emit("tray: active (close-to-tray)");
        }
        Err(e) => {
            crate::debug::emit(&format!("tray: creation failed: {e}"));
            STARTED.store(false, Ordering::SeqCst);
        }
    }
}

/// The embedded tray icon as raw RGBA pixels.
///
/// `packing/macos/tray-black.rgba` is a 32x32 RGBA raster generated with
/// ffmpeg from `assets/dsh-black.svg` (see README "图标"). Raw RGBA is
/// embedded as-is so no image-decoding crate is needed at runtime (tray-icon
/// converts to the NSImage format internally).
const TRAY_ICON_RGBA: &[u8] = include_bytes!("../../packing/macos/tray-black.rgba");
const TRAY_ICON_SIZE: u32 = 32;

fn build_icon() -> Option<tray_icon::Icon> {
    tray_icon::Icon::from_rgba(TRAY_ICON_RGBA.to_vec(), TRAY_ICON_SIZE, TRAY_ICON_SIZE).ok()
}

/// Main-thread poll: build the tray when first needed and drain both event
/// channels into the atomic flags. Called from [`quit_requested`] and
/// [`restore_requested`], which the UI event loop polls every ~50 ms.
fn poll() {
    ensure_started();
    // Menu events (muda channel).
    while let Ok(event) = MenuEvent::receiver().try_recv() {
        if event.id == MENU_RESTORE {
            RESTORE_REQUESTED.store(true, Ordering::SeqCst);
        } else if event.id == MENU_QUIT {
            QUIT_REQUESTED.store(true, Ordering::SeqCst);
        } else if event.id == MENU_OPEN_URL {
            OPEN_URL_REQUESTED.store(true, Ordering::SeqCst);
        }
    }
    // Status-item clicks: a left click (release) restores the window.
    while let Ok(event) = TrayIconEvent::receiver().try_recv() {
        if let TrayIconEvent::Click {
            button: MouseButton::Left,
            button_state: MouseButtonState::Up,
            ..
        } = event
        {
            RESTORE_REQUESTED.store(true, Ordering::SeqCst);
        }
    }
}

/// Called when the user closes the window; the status item stays, dsh keeps
/// running.
pub fn hide_to_tray() {
    crate::debug::emit("tray: window closed, keeping dsh running (close-to-tray)");
}

pub fn quit_requested() -> bool {
    poll();
    QUIT_REQUESTED.load(Ordering::SeqCst)
}

/// True when the user chose "restore window" (left click or menu item). The
/// launcher re-creates the WebView window.
pub fn restore_requested() -> bool {
    poll();
    RESTORE_REQUESTED.swap(false, Ordering::SeqCst)
}

/// True when the user chose "打开 dsh" from the menu — the launcher opens the
/// dsh URL in the system default browser.
pub fn open_url_requested() -> bool {
    poll();
    OPEN_URL_REQUESTED.swap(false, Ordering::SeqCst)
}

/// No-op: the icon is an NSImage template, so macOS adapts it to the current
/// menu-bar theme automatically. Kept for interface parity with the other
/// platforms.
pub fn set_icon(_dark: bool) {}

/// Nothing to clean up: the status item is intentionally leaked and the
/// process exits right after this.
pub fn shutdown() {}
