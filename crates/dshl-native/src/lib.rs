//! dshl-native — the plugin-track (Backend B) native addon.
//!
//! Built with napi-rs into a per-platform `.node` dll that the `@dshl/control`
//! plugin installs and calls directly. This is the dual-track counterpart to
//! the launcher binary: Track A ships the full dshl app; this Track B addon
//! hands the pure-JS plugin the same OS-level primitives (open-terminal,
//! open-path, open-url, platform info) without needing a running dshl process.
//!
//! Deliberately self-contained (std only): it must NOT link the webui C
//! library, so the addon stays small and installable on machines that only run
//! dsh plus the plugin. The command choices mirror dshl-core's
//! `platform/actions.rs` and `platform/detect.rs` exactly so both tracks stay
//! in sync.

use std::path::Path;
use std::process::{Command, Stdio};

use napi_derive::napi;

/// Options for opening a terminal: the working directory and an optional
/// `PATH` to prepend so dsh / node / bun resolve inside the new terminal.
#[napi(object)]
pub struct OpenTerminalOptions {
    pub cwd: String,
    pub path: Option<String>,
}

/// Host platform facts, mirroring dshl-core's `platform/detect.rs`.
#[napi(object)]
pub struct PlatformInfo {
    pub os: String,
    pub arch: String,
    pub shell: String,
}

/// Describe the host platform (os / arch / shell).
#[napi]
pub fn platform_info() -> PlatformInfo {
    PlatformInfo {
        os: os_name().to_string(),
        arch: arch_name().to_string(),
        shell: shell_name().to_string(),
    }
}

/// Open a file or directory in the OS file manager / default viewer.
///
/// Mirrors dshl-core `platform::actions::open_path`.
#[napi]
pub fn open_path(path: String) -> bool {
    let p = Path::new(&path);
    match os() {
        Os::Windows => {
            if p.is_dir() {
                let mut cmd = Command::new("explorer");
                cmd.arg(p);
                spawn(cmd)
            } else {
                let mut cmd = Command::new("explorer");
                cmd.arg(format!("/select,{}", p.display()));
                spawn(cmd)
            }
        }
        Os::Macos => {
            let mut cmd = Command::new("open");
            cmd.arg(p);
            spawn(cmd)
        }
        Os::Linux => {
            let mut cmd = Command::new("xdg-open");
            cmd.arg(p);
            spawn(cmd)
        }
    }
}

/// Open a URL in the system default browser.
///
/// Mirrors dshl-core `platform::actions::open_url`.
#[napi]
pub fn open_url(url: String) -> bool {
    match os() {
        // `explorer.exe` would treat a URL as a path to select in a file
        // manager, and `cmd /C start "" "<url>"` mangles the quoted URL. The
        // `url.dll,FileProtocolHandler` verb is the canonical way to open a
        // URL: it bypasses cmd entirely, so a `&` in a query string is never
        // parsed as a command separator.
        Os::Windows => {
            let mut cmd = Command::new("rundll32");
            cmd.args(["url.dll,FileProtocolHandler", &url]);
            spawn(cmd)
        }
        Os::Macos => {
            let mut cmd = Command::new("open");
            cmd.arg(&url);
            spawn(cmd)
        }
        Os::Linux => {
            let mut cmd = Command::new("xdg-open");
            cmd.arg(&url);
            spawn(cmd)
        }
    }
}

/// Open a terminal window running in `cwd` with `path` prepended to `PATH`
/// (the resolved dsh runtime dirs), so the terminal feels like the dsh
/// environment. Detached: the caller spawns it and never waits on the process.
///
/// Mirrors dshl-core `platform::actions::open_terminal`.
#[napi]
pub fn open_terminal(options: OpenTerminalOptions) -> bool {
    let cwd = Path::new(&options.cwd);
    let path = options.path.as_deref().map(std::ffi::OsStr::new);
    match os() {
        // `cmd /C start` creates a NEW console for powershell and connects its
        // standard handles to it, so the terminal owns its window; cmd gets
        // nulled stdio and exits immediately. Same shape as dshl-core.
        Os::Windows => {
            let mut cmd = Command::new("cmd.exe");
            cmd.args(["/C", "start", "", "powershell.exe", "-NoExit", "-NoLogo"]);
            if let Some(p) = path {
                cmd.env("PATH", p);
            }
            cmd.current_dir(cwd)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            spawn(cmd)
        }
        Os::Macos => {
            let _ = path;
            let mut cmd = Command::new("open");
            cmd.args(["-a", "Terminal"]).arg(cwd);
            spawn(cmd)
        }
        Os::Linux => {
            for program in [
                "x-terminal-emulator",
                "gnome-terminal",
                "konsole",
                "xfce4-terminal",
            ] {
                let mut cmd = Command::new(program);
                cmd.current_dir(cwd);
                if let Some(p) = path {
                    cmd.env("PATH", p);
                }
                if spawn(cmd) {
                    return true;
                }
            }
            false
        }
    }
}

fn spawn(mut cmd: Command) -> bool {
    cmd.spawn().is_ok()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Os {
    Windows,
    Linux,
    Macos,
}

fn os() -> Os {
    if cfg!(target_os = "windows") {
        Os::Windows
    } else if cfg!(target_os = "macos") {
        Os::Macos
    } else {
        Os::Linux // best-effort: treat other unix-likes as linux
    }
}

fn os_name() -> &'static str {
    match os() {
        Os::Windows => "windows",
        Os::Linux => "linux",
        Os::Macos => "macos",
    }
}

fn arch_name() -> &'static str {
    std::env::consts::ARCH
}

fn shell_name() -> &'static str {
    match os() {
        Os::Windows => "powershell",
        Os::Macos | Os::Linux => "bash",
    }
}
