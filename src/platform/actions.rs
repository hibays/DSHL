//! OS-level user actions (open a file in the file manager, …).

use std::path::Path;
use std::process::Command;

#[cfg(target_os = "windows")]
use std::process::Stdio;

use super::detect::Os;
use super::detect::os;

/// Open a file (selected in the file manager) or a directory in the OS shell.
pub fn open_path(path: &Path) -> std::io::Result<()> {
    match os() {
        Os::Windows => {
            if path.is_dir() {
                Command::new("explorer").arg(path).spawn().map(|_| ())
            } else {
                Command::new("explorer")
                    .arg(format!("/select,{}", path.display()))
                    .spawn()
                    .map(|_| ())
            }
        }
        Os::Macos => Command::new("open").arg(path).spawn().map(|_| ()),
        Os::Linux => Command::new("xdg-open").arg(path).spawn().map(|_| ()),
    }
}

/// Open a terminal window running in `cwd` with `path` prepended to `PATH`
/// (the resolved dsh runtime dirs), so the terminal feels like the dsh
/// environment: `dsh` / `node` / `bun` resolve without extra setup.
///
/// Detached: the launcher spawns it and never waits on the terminal process.
pub fn open_terminal(path: Option<&std::ffi::OsStr>, cwd: &Path) -> std::io::Result<()> {
    #[cfg(target_os = "windows")]
    {
        // Give the terminal its own interactive console window. `cmd /C start`
        // is the system's basic "open a detached program" mechanism: `start`
        // creates a NEW console for powershell and connects the console's
        // standard handles to it, so the terminal owns its stdin/stdout/stderr.
        // (Inheriting the launcher's redirected stdio would sink the shell's
        // output into dshl's own log — the terminal must own its window.)
        // cmd itself gets nulled stdio and exits immediately, leaving
        // powershell running detached in its own window.
        let mut cmd = Command::new("cmd.exe");
        cmd.args(["/C", "start", "", "powershell.exe", "-NoExit", "-NoLogo"]);
        if let Some(path) = path {
            cmd.env("PATH", path);
        }
        cmd.current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        cmd.spawn().map(|_| ())
    }
    #[cfg(target_os = "macos")]
    {
        let _ = path;
        Command::new("open")
            .arg("-a")
            .arg("Terminal")
            .arg(cwd)
            .spawn()
            .map(|_| ())
    }
    #[cfg(target_os = "linux")]
    {
        for program in [
            "x-terminal-emulator",
            "gnome-terminal",
            "konsole",
            "xfce4-terminal",
        ] {
            let mut cmd = Command::new(program);
            cmd.current_dir(cwd);
            if let Some(path) = path {
                cmd.env("PATH", path);
            }
            match cmd.spawn() {
                Ok(_) => return Ok(()),
                Err(_) => continue,
            }
        }
        Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "no terminal emulator found",
        ))
    }
}

/// Open a URL in the system default browser.
pub fn open_url(url: &str) -> std::io::Result<()> {
    match os() {
        // `explorer.exe` would treat a URL as a path to select in a file
        // manager, and `cmd /C start "" "<url>"` mangles the quoted URL (Rust
        // escapes the embedded quotes to `\"`, which start treats as part of
        // the filename). `url.dll,FileProtocolHandler` is the canonical way to
        // open a URL in the system default browser: it bypasses cmd entirely,
        // so a `&` in a query string is never parsed as a command separator.
        Os::Windows => Command::new("rundll32")
            .args(["url.dll,FileProtocolHandler", url])
            .spawn()
            .map(|_| ()),
        Os::Macos => Command::new("open").arg(url).spawn().map(|_| ()),
        Os::Linux => Command::new("xdg-open").arg(url).spawn().map(|_| ()),
    }
}
