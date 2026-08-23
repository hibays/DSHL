//! Signal handling + runtime-state reset helpers used by both tracks.
//!
//! `install_ctrlc` wires ctrlc v3 to `dshl_core::ui::request_shutdown` (the
//! same path the WebView close handler and tray "quit" menu funnel through),
//! so a Ctrl+C in either track produces the composed teardown.
//!
//! `reset_runtime_state` delegates to `dshl_core::ui::reset_runtime_state`
//! — the UI module owns its own atomics, the runner only triggers the reset
//! before each `ui::setup`.

/// Install the OS signal handler (Ctrl+C / SIGTERM) that asks the launcher
/// to shut down cleanly. Idempotent — a second install is a no-op inside
/// `ctrlc` v3.
pub(crate) fn install_ctrlc() {
    let _ = ctrlc::set_handler(dshl_core::ui::request_shutdown);
}

/// ctrlc v3 exposes no guard handle; installing twice is a no-op / warning
/// inside the crate, so we just install and return None. Kept as a Boxed
/// slot so a future upgrade to a handler that supports unregister drops
/// cleanly here.
pub(crate) fn install_ctrlc_boxed() -> Option<Box<dyn std::any::Any + Send + Sync>> {
    install_ctrlc();
    None
}

/// Reset every "sticky" runtime flag (shutdown requested, flow running, etc.)
/// so a second kernel boot in the same process doesn't inherit stale flags.
/// Delegates to [`dshl_core::ui::reset_runtime_state`] — the UI module owns
/// its own state, the runner only triggers the reset.
pub(crate) fn reset_runtime_state() {
    dshl_core::ui::reset_runtime_state();
}
