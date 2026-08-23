//! Test-only helpers shared by unit tests across modules.

#![cfg(test)]

use std::process::Command;

/// Platform-appropriate one-shot shell command.
///
/// Windows: `%COMSPEC% /c <win>`; Unix: `sh -c <unix>`. The scripts are
/// per-platform ON PURPOSE — cmd and sh differ in separators (`&` vs `;`),
/// backgrounding (`start /b` vs `&`) and sleeps (`ping -n` vs `sleep`), so a
/// single portable string cannot express the same child behaviour.
///
/// Replaces the former copy-pasted
/// `Command::new(env::var("COMSPEC").unwrap_or("cmd"))` in every subprocess
/// test (which, being runtime-env based, mis-selected under WSL interop).
#[cfg_attr(not(test), allow(unused))]
pub(crate) fn shell(win: &str, unix: &str) -> Command {
    #[cfg(windows)]
    {
        let _ = unix;
        let mut c = Command::new(std::env::var("COMSPEC").unwrap_or_else(|_| "cmd".into()));
        c.args(["/c", win]);
        c
    }
    #[cfg(not(windows))]
    {
        let _ = win;
        let mut c = Command::new("sh");
        c.args(["-c", unix]);
        c
    }
}
