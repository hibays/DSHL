//! System-tray controls — show / hide / icon swap / visibility query.
//!
//! Thin napi shims over `dshl_cli::tray_*` (which themselves are thin
//! delegations to `dshl_core::tray::{start, shutdown, set_icon, is_started}`).

use napi_derive::napi;

/// Force-show the tray icon (even if close-to-tray is off). Normally the
/// kernel shows it automatically when close-to-tray is enabled.
#[napi]
pub fn tray_show() {
    dshl_cli::tray_show();
}

/// Hide and destroy the tray icon. Does NOT stop the kernel.
#[napi]
pub fn tray_hide() {
    dshl_cli::tray_hide();
}

/// Swap the tray icon to the dark-variant (true) or light-variant (false).
/// No-op on platforms whose tray icons adapt automatically (macOS template,
/// Linux appindicator desktop theme).
#[napi]
pub fn tray_set_icon(dark: bool) {
    dshl_cli::tray_set_icon(dark);
}

#[napi]
pub fn tray_is_visible() -> bool {
    dshl_cli::tray_is_visible()
}
