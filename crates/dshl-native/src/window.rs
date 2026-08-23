//! Launcher window controls — show / hide / navigate.
//!
//! All four are thin napi shims over the corresponding `dshl_cli::window_*`
//! entry points, which in turn delegate to `dshl_core::ui::window_*`. Keeping
//! them in their own napi module lets the JS-side `.d.ts` group them
//! separately from kernel / tray / supervisor controls.

use napi_derive::napi;

/// Show the launcher window (creates it if not yet built, or restores it
/// from the tray). Mirrors the tray "restore" menu behaviour.
///
/// Returns `false` when the request was skipped because the kernel is still
/// booting (see `dshl_cli::window_show`) — JS surfaces this instead of a
/// success toast.
#[napi]
pub fn window_show() -> bool {
    dshl_cli::window_show()
}

/// Hide the launcher window. When close-to-tray is enabled this transitions
/// to the tray (window resources freed, tray icon visible, dsh keeps
/// running). When close-to-tray is disabled this is a no-op: hiding the
/// window without a tray to hand over to is equivalent to quitting, which
/// must be explicit via `shutdown()`.
#[napi]
pub fn window_hide() {
    dshl_cli::window_hide();
}

/// True iff the launcher window currently exists and is not in the tray.
#[napi]
pub fn window_is_visible() -> bool {
    dshl_cli::window_is_visible()
}

/// Navigate the launcher window to an arbitrary URL. Useful after
/// `launch()` if the caller already knows a dsh URL and wants to skip the
/// startup page.
#[napi]
pub fn window_navigate(url: String) {
    dshl_cli::window_navigate(url);
}
