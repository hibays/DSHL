//! webui.me startup window — the UI layer.
//!
//! The window serves the embedded startup page through a virtual file
//! handler, exposes a few bound functions to the frontend, and is later
//! navigated to the dsh URL once the launcher succeeds.
//!
//! The layer is split into small modules with explicit responsibilities (so
//! it stays loosely coupled and each piece is testable in isolation):
//!
//! - [`state`]: every piece of shared mutable state (atomics, paths) lives
//!   here; the other modules coordinate only through it.
//! - [`assets`] / [`vfs`]: the embedded startup page and the virtual file
//!   handler that serves it.
//! - [`bindings`]: the functions bound to the page (get_state, retry, …).
//! - [`window`]: window lifecycle — creation, setup, close handling,
//!   geometry persistence, theme watching, handle tracking, tray restore.
//! - [`launch`]: the launch flow — stale-dsh cleanup, config load, pipeline
//!   run, navigation, supervision of the dsh child.
//! - [`supervisor`]: the main event loop that ties everything together.
//!
//! The public entry points are [`setup`] (show the window), [`launch_flow`]
//! (start dsh in the background), [`run_loop`] (drive the event loop) and
//! [`request_shutdown`] (SIGINT/SIGTERM handler).

pub mod assets;

mod bindings;
mod crash;
mod exit;
mod launch;
mod state;
mod supervisor;
mod vfs;
mod window;

pub use exit::request_shutdown;
#[cfg(test)]
pub(crate) use exit::shutdown_requested;
pub use launch::{kill_dsh, launch_flow, request_restart};
pub use supervisor::run_loop;
pub use window::setup;
