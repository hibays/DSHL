//! Process helpers: an async child that streams output line by line, plus
//! synchronous capture and (on Windows) a hidden-console spawn so dsh can be
//! stopped gracefully with Ctrl+C.
//!
//! The module is split into small cohesive pieces (loose coupling):
//!
//! - [`capture`]: synchronous capture ([`run`]) / asynchronous capture
//!   ([`run_async`]) and [`Command`] preparation ([`with_env`] /
//!   `prepare_spawn`).
//! - [`child`]: [`AsyncChild`] — the async streaming child (tokio pipe readers
//!   + reaper, line queue, `Notify`).
//! - `win_proc` (Windows only): hidden-console `CreateProcessW` spawn and
//!   graceful Ctrl+C via `AttachConsole` / `GenerateConsoleCtrlEvent`, all
//!   through windows-rs 0.62.
//! - `win_job` (Windows only): the kill-on-close job object that reaps
//!   children when the launcher is terminated abruptly.
//!
//! [`Command`]: std::process::Command

pub mod capture;
pub mod child;

#[cfg(target_os = "windows")]
mod win_job;
#[cfg(target_os = "windows")]
mod win_proc;

pub use capture::{CommandResult, run, run_async, with_env};
pub use child::{AsyncChild, Output};
