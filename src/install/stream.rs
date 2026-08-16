//! Stream a command's output into the progress log.

use std::process::Command;

use crate::error::{Error, Result};
use crate::process::{AsyncChild, Output};
use crate::progress;

/// Stream a command's output into the progress log and fail on non-zero exit.
pub async fn run_streaming(mut cmd: Command, label: &str) -> Result<()> {
    let child = AsyncChild::spawn(&mut cmd)
        .map_err(|e| Error(t!("stream.start_failed", label = label, err = e).to_string()))?;
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
        Some(code) => Err(Error(
            t!("stream.exit_failed", label = label, code = code).to_string(),
        )),
        None => Err(Error(t!("stream.no_status", label = label).to_string())),
    }
}
