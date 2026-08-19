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

/// Convert a prepared `std::process::Command` into a `tokio::process::Command`.
///
/// tokio's `From<Command>` conversion carries the program, arguments, env and
/// cwd but not the platform spawn hooks (hidden console on Windows, PDEATHSIG
/// on Linux), so they are re-applied here.
pub(crate) fn to_tokio(cmd: &mut Command) -> tokio::process::Command {
    let program = cmd.get_program().to_os_string();
    let mut tcmd = tokio::process::Command::from(std::mem::replace(cmd, Command::new(program)));
    #[cfg(target_os = "windows")]
    tcmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    #[cfg(target_os = "linux")]
    unsafe {
        tcmd.pre_exec(|| {
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
            Ok(())
        });
    }
    tcmd
}

/// Run a command to completion asynchronously, capturing stdout/stderr.
pub async fn run_async(cmd: &mut Command) -> io::Result<CommandResult> {
    let output = to_tokio(cmd).output().await?;
    Ok(CommandResult {
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        status: Some(output.status),
    })
}
