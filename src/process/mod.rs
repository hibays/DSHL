//! Process helpers: a native-async child that streams output line by line,
//! plus synchronous capture and (on Windows) a hidden-console spawn so dsh can
//! be stopped gracefully with Ctrl+C.
//!
//! The module is split into small cohesive pieces (loose coupling):
//!
//! - [`capture`]: synchronous capture ([`run`]) and [`Command`] preparation
//!   ([`with_env`] / `prepare_spawn`).
//! - [`child`]: [`AsyncChild`] — the async streaming child (reader threads +
//!   reaper thread, line queue, waker).
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

pub use capture::{CommandResult, run, with_env};
pub use child::{AsyncChild, Output};
