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

#[cfg(test)]
mod tests {
    use super::*;

    /// A command that keeps producing output for ~2s, so the `next_line` drain
    /// is exercised over many wakeups.
    fn verbose_cmd() -> Command {
        #[cfg(windows)]
        {
            let mut cmd = Command::new("powershell");
            cmd.args([
                "-NoProfile",
                "-Command",
                // No per-line sleep: on the windows-11-arm runner each
                // Start-Sleep tick costs ~35ms and blew the 5s drain budget.
                // Rapid-fire still exercises one notify wakeup per line.
                "1..150 | ForEach-Object { Write-Output ('line ' + $_) }",
            ]);
            cmd
        }
        #[cfg(not(windows))]
        {
            let mut cmd = Command::new("sh");
            cmd.args([
                "-c",
                // Pure shell builtins: no per-line fork (a forked `sleep`
                // costs ~5-10ms under WSL/CI load and blew the 5s budget).
                "i=0; while [ $i -lt 300 ]; do echo \"line $i\"; i=$((i+1)); done",
            ]);
            cmd
        }
    }

    /// Regression: `bun add -g`'s output pattern used to hit a lost-wakeup
    /// race in `AsyncChild::next_line` — a `notify_one()` landing between two
    /// `next_line` calls left the consumer parked forever (the drain only
    /// finished when the outer timeout fired ~30s later). The drain must now
    /// complete well under a second.
    #[test]
    fn verbose_output_drain_completes_promptly() {
        let mut cmd = verbose_cmd();
        let result = crate::runtime::block_on(async {
            let child = AsyncChild::spawn(&mut cmd).expect("spawn");
            tokio::time::timeout(std::time::Duration::from_secs(5), async {
                let mut lines = Vec::new();
                while let Some(l) = child.next_line().await {
                    lines.push(l);
                }
                (lines, child)
            })
            .await
        });
        match result {
            Ok((lines, child)) => {
                assert_eq!(child.exit_code(), Some(0), "command should exit 0");
                assert!(
                    lines.len() >= 100,
                    "expected a few hundred drained lines, got {}",
                    lines.len()
                );
            }
            Err(_) => panic!("drain did not complete in 5s — lost-wakeup race is back"),
        }
    }
}
