//! Synchronous command capture: run a command to completion and collect its
//! output, plus the shared [`Command`] preparation helpers.

use std::io;
use std::process::{Command, ExitStatus};

/// Result of a synchronously captured command.
#[derive(Debug, Clone)]
pub struct CommandResult {
    pub stdout: String,
    pub stderr: String,
    pub status: Option<ExitStatus>,
}

impl CommandResult {
    pub fn success(&self) -> bool {
        self.status.map(|s| s.success()).unwrap_or(false)
    }

    pub fn code(&self) -> Option<i32> {
        self.status.and_then(|s| s.code())
    }
}

/// Apply a list of `(key, value)` environment variables to a command.
pub fn with_env<'a>(cmd: &'a mut Command, env: &[(String, String)]) -> &'a mut Command {
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd
}

/// Configure a command so its child runs without a console window (Windows)
/// and, on Linux, dies with the launcher (`PR_SET_PDEATHSIG`).
///
/// Shared by [`run`] and [`crate::process::child::AsyncChild::spawn`]; not
/// part of the public API.
pub(crate) fn prepare_spawn(cmd: &mut Command) {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    #[cfg(target_os = "linux")]
    unsafe {
        use std::os::unix::process::CommandExt;
        cmd.pre_exec(|| {
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
            Ok(())
        });
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = cmd;
    }
}

/// Run a command to completion, capturing stdout/stderr (no shell).
pub fn run(cmd: &mut Command) -> io::Result<CommandResult> {
    prepare_spawn(cmd);
    let output = cmd.output()?;
    Ok(CommandResult {
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        status: Some(output.status),
    })
}
