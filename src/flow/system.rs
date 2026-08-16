//! Flow 1 — check the host OS and CPU architecture.

use crate::error::Result;
use crate::platform;
use crate::progress::{self, StepStatus};

pub async fn run() -> Result<()> {
    progress::step("system", StepStatus::Running, t!("flow.system.running"));

    let os = platform::os_name();
    let arch = platform::arch_name();
    let home = platform::home_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    let shell = match platform::shell() {
        platform::Shell::PowerShell => "PowerShell",
        platform::Shell::Cmd => "cmd",
        platform::Shell::Bash => "bash",
        platform::Shell::Sh => "sh",
    };

    progress::log(t!("flow.system.os", os = os));
    progress::log(t!("flow.system.arch", arch = arch));
    progress::log(format!("Shell: {shell}"));
    if !home.is_empty() {
        progress::log(t!("flow.system.home", home = home));
    }

    progress::step("system", StepStatus::Done, format!("{os}/{arch}"));
    Ok(())
}
