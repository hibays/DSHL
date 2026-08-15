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
