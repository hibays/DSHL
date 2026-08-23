//! Host-platform introspection + OS-level actions (open-terminal / open-path /
//! open-url).
//!
//! `ping` / `platform_info` are health-check surfaces that the plugin loader
//! and the HTTP `/_dsh/desktop/health` route use to verify the addon is
//! loaded. The `open_*` trio are thin delegations to the authoritative copies
//! in `dshl_core::platform::actions`, exposed so JS callers don't have to
//! boot the kernel just to open a folder or a terminal.

use napi_derive::napi;

use crate::types::{OpenTerminalOptions, PingInfo, PlatformInfo};

#[napi]
pub fn ping() -> PingInfo {
    PingInfo {
        pong: true,
        version: env!("CARGO_PKG_VERSION").to_string(),
    }
}

#[napi]
pub fn platform_info() -> PlatformInfo {
    use dshl_core::platform::Shell;
    let shell = match dshl_core::platform::shell() {
        Shell::PowerShell => "powershell",
        Shell::Cmd => "cmd",
        Shell::Bash => "bash",
        Shell::Sh => "sh",
    };
    PlatformInfo {
        os: dshl_core::platform::os_name().to_string(),
        arch: dshl_core::platform::arch_name().to_string(),
        shell: shell.to_string(),
    }
}

// ---------------------------------------------------------------------------
// OS-level actions — thin delegation so JS callers don't have to go through
// the kernel boot path when they just want open-terminal / open-path.
// ---------------------------------------------------------------------------

#[napi]
pub fn open_terminal(options: OpenTerminalOptions) -> bool {
    dshl_cli::open_terminal(options.cwd, options.path)
}

#[napi]
pub fn open_path(path: String) -> bool {
    dshl_cli::open_path(path)
}

#[napi]
pub fn open_url(url: String) -> bool {
    dshl_cli::open_url(url)
}
