//! dshl-native — the plugin-track (Track B) native addon.
//!
//! Built with napi-rs into a per-platform `.node` cdylib that the
//! `@dshl/control` plugin installs and calls directly.
//!
//! THIS DLL IS THE FULL KERNEL. It links against the same `dshl-core` rlib
//! that the installer binary (`dshl.exe` / dshl.app / dshl.deb / PKGBUILD)
//! uses, so it contains *every* capability of the launcher:
//!
//!   - the embedded startup webui window (webui.me WebView or external
//!     browser, the exact same UI/assets/state machine),
//!   - close-to-tray with a native system-tray icon,
//!   - the full setup pipeline: runtime probing, mirror resolution, dsh
//!     install/update/version pinning via bun/node/pnpm,
//!   - the supervisor event loop + dsh graceful teardown,
//!   - launcher-level single-instance,
//!   - OS-level actions (open-terminal / open-path / open-url),
//!   - restart / shutdown / switch-profile hooks,
//!   - embedded PTY terminal backend (portable-pty + WebSocket).
//!
//! Track A (exe installer) and Track B (plugin cdylib) differ ONLY in how
//! the kernel is entered:
//!   Track A binary → dshl_cli::run_cli() (blocks the calling thread until
//!                    the supervisor loop exits).
//!   Track B addon  → launch(RunOptions) spawns the kernel on a background
//!                    thread and returns a status handle; further #[napi]
//!                    calls (windowShow, trayHide, shutdown, restart, …)
//!                    drive the running kernel across threads.
//!
//! Both tracks share the same Rust source for every capability. The
//! historical std-only mirror of open-terminal/... in this crate has been
//! fully retired: the authoritative copies live in
//! `dshl_core::platform::actions` and are called here directly.
//!
//! Module layout:
//! - [`types`]: all `#[napi(object)]` struct definitions (mirrors of the
//!   dshl_cli / dshl_core types — napi-derive needs its own types for TS-def
//!   generation).
//! - [`kernel`]: `launch` / `is_kernel_running` / `launch_status` + the
//!   singleton `RUN_HANDLE` static.
//! - [`window`]: window_show / hide / is_visible / navigate.
//! - [`tray`]: tray_show / hide / set_icon / is_visible.
//! - [`supervisor`]: shutdown / restart (+ detached-restart fallback).
//! - [`platform`]: ping / platform_info / open_terminal / open_path / open_url.
//! - [`pty`]: terminal_spawn / list / kill / resize / write / ws_endpoint.

mod kernel;
mod platform;
mod pty;
mod supervisor;
mod tray;
pub mod types;
mod window;

// Public napi surface — re-export every `#[napi]` function so napi-derive's
// scan of `lib.rs` picks them up. (napi-derive walks the crate root, so every
// exported symbol must be reachable from here.)
pub use kernel::{is_kernel_running, launch, launch_status};
pub use platform::{open_path, open_terminal, open_url, ping, platform_info};
pub use pty::{
    terminal_kill, terminal_list, terminal_resize, terminal_spawn, terminal_write,
    terminal_ws_endpoint,
};
pub use supervisor::{restart, shutdown};
pub use tray::{tray_hide, tray_is_visible, tray_set_icon, tray_show};
pub use window::{window_hide, window_is_visible, window_navigate, window_show};
