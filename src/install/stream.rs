//! Stream a command's output into the progress log.

use std::process::Command;

use crate::error::{Error, Result};
use crate::process::{AsyncChild, Output};
use crate::progress;

/// Stream a command's output into the progress log and fail on non-zero exit.
pub async fn run_streaming(mut cmd: Command, label: &str) -> Result<()> {
    let child =
        AsyncChild::spawn(&mut cmd).map_err(|e| Error(format!("failed to start {label}: {e}")))?;
    while let Some(line) = child.next_line().await {
        match line {
            Output::Stdout(l) => {
                let t = l.trim();
                if !t.is_empty() {
                    progress::log(t);
                }
            }
            Output::Stderr(l) => {
                let t = l.trim();
                if !t.is_empty() {
                    progress::log(t);
                }
            }
        }
    }
    match child.exit_code() {
        Some(0) => Ok(()),
        Some(code) => Err(Error(format!("{label} failed (exit {code})"))),
        None => Err(Error(format!("{label} exited without a status"))),
    }
}
