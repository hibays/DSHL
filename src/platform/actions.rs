//! OS-level user actions (open a file in the file manager, …).

use std::path::Path;
use std::process::Command;

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
