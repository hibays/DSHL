//! Host OS / architecture / shell detection.

use std::env;
use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Windows,
    Linux,
    Macos,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arch {
    X86_64,
    Aarch64,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shell {
    /// `powershell -NoProfile -Command`
    PowerShell,
    /// `cmd /C`
    Cmd,
    /// `bash -lc`
    Bash,
    /// `sh -c`
    Sh,
}

/// Detect the host OS.
pub fn os() -> Os {
    if cfg!(target_os = "windows") {
        Os::Windows
    } else if cfg!(target_os = "macos") {
        Os::Macos
    } else {
        Os::Linux // best-effort: treat other unix-likes as linux
    }
}

/// Detect the host CPU architecture.
pub fn arch() -> Arch {
    match env::consts::ARCH {
        "x86_64" => Arch::X86_64,
        "aarch64" => Arch::Aarch64,
        _ => Arch::Other,
    }
}

pub fn os_name() -> &'static str {
    match os() {
        Os::Windows => "windows",
        Os::Linux => "linux",
        Os::Macos => "macos",
    }
}

pub fn arch_name() -> &'static str {
    match arch() {
        Arch::X86_64 => "x86_64",
        Arch::Aarch64 => "aarch64",
        Arch::Other => env::consts::ARCH,
    }
}

/// The shell used to run script snippets (install scripts, nvm, …).
pub fn shell() -> Shell {
    match os() {
        Os::Windows => Shell::PowerShell,
        Os::Macos | Os::Linux => Shell::Bash,
    }
}

/// Build a `Command` that runs a snippet through the platform shell.
///
/// The returned command already carries the shell executable; callers append
/// the snippet as the single argument via `.arg(script)`.
pub fn shell_command() -> Command {
    match shell() {
        Shell::PowerShell => {
            let mut c = Command::new("powershell");
            c.args(["-NoProfile", "-NonInteractive", "-Command"]);
            c
        }
        Shell::Cmd => {
            let mut c = Command::new("cmd");
            c.arg("/C");
            c
        }
        Shell::Bash => {
            let mut c = Command::new("bash");
            c.args(["-lc"]);
            c
        }
        Shell::Sh => {
            let mut c = Command::new("sh");
            c.arg("-c");
            c
        }
    }
}
