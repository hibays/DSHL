//! System-tray support (`close-to-tray`).
//!
//! When `close-to-tray` is enabled, closing the launcher window hides it
//! instead of exiting so dsh keeps running in the background. A tray icon
//! restores the window on click and offers a small menu: restore or quit.
//! Quitting from the tray flags the launcher to shut down (the event loop
//! picks it up and goes through the normal Ctrl+C clean-shutdown path, which
//! stops dsh gracefully via SIGINT/SIGTERM — the same cross-platform
//! mechanism as everywhere else).
//!
//! The module is deliberately decoupled from [`crate::ui`]: it exposes a
//! tiny, platform-agnostic interface (same 7 functions on every OS) and each
//! platform has its own implementation:
//!
//! - [`windows`]: a hidden message-only window + `Shell_NotifyIconW`, all via
//!   the `windows` crate (windows-rs 0.62) — no hand-written FFI.
//! - [`linux`]: StatusNotifier via `dlopen`-ed libayatana-appindicator3 +
//!   GTK3, so the binary never hard-depends on the desktop libs; on systems
//!   without them `close-to-tray` degrades to close-to-exit with a log line.
//! - [`macos`]: a native `NSStatusItem` through `tray-icon` (AppKit backend,
//!   no hand-written objc FFI). The icon is an NSImage *template*, so macOS
//!   renders it in the menu-bar colour automatically in light and dark mode.
//!
//! The interface contract:
//!
//! - [`start`] makes the tray appear (idempotent; safe to call from any
//!   thread — each backend defers platform work to its own thread or to the
//!   main-thread polls).
//! - [`hide_to_tray`] is called by the window close handler when the window
//!   is allowed to close for real; the tray keeps dsh alive.
//! - [`quit_requested`] / [`restore_requested`] are polled by the UI event
//!   loop and fold platform events (menu clicks, icon clicks) into plain
//!   booleans.
//! - [`open_url_requested`] folds the "打开 dsh" menu item into a boolean the
//!   UI loop polls to open the dsh URL in the system default browser.
//! - [`set_icon`] swaps the day/night icon variant (no-op where the OS
//!   adapts automatically, e.g. macOS templates).
//! - [`shutdown`] removes the icon and stops background threads.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "linux")]
pub use linux::{
    hide_to_tray, open_url_requested, quit_requested, restore_requested, set_icon, shutdown, start,
};
#[cfg(target_os = "macos")]
pub use macos::{
    hide_to_tray, open_url_requested, quit_requested, restore_requested, set_icon, shutdown, start,
};
#[cfg(target_os = "windows")]
pub use windows::{
    hide_to_tray, open_url_requested, quit_requested, restore_requested, set_icon, shutdown, start,
};
