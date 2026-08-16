//! Flow 5 — spawn `dsh` as a managed child and capture the URL it prints.
//!
//! dsh runs as a supervised child (not detached): its stdout/stderr are
//! streamed line-by-line, mirrored to the log file and scanned for the
//! `http://…:port` URL. The child handle is returned so the launcher can
//! keep draining it and reap/kill it on shutdown.

use std::io::Write;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use regex::Regex;

use crate::error::{Error, Result};
use crate::flow::Launch;
use crate::platform;
use crate::process::{AsyncChild, Output};
use crate::progress::{self, StepStatus};

/// How long to wait for dsh to print its web URL.
const URL_TIMEOUT: Duration = Duration::from_secs(120);

/// Path of the per-run dsh log file (truncated on every launch).
pub fn log_path() -> PathBuf {
    platform::cache_dir().join("dshl").join("dsh.log")
}

fn url_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"https?://[^\s"'<>]+:\d+"#).unwrap())
}

pub async fn run(mut command: Command) -> Result<Launch> {
    progress::step("launch", StepStatus::Running, t!("flow.launch.starting"));

    let path = log_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let mut log_file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&path)
        .map_err(|e| {
            Error(
                t!(
                    "flow.launch.log_open_failed",
                    path = path.display().to_string(),
                    err = e
                )
                .to_string(),
            )
        })?;

    let child = Arc::new(
        AsyncChild::spawn_console(&mut command)
            .map_err(|e| Error(t!("flow.launch.spawn_failed", err = e).to_string()))?,
    );
    let pid = child.pid().unwrap_or(0);
    *crate::DSH_CHILD.lock().unwrap() = Some(child.clone());

    progress::log(t!("flow.launch.started", pid = pid));
    progress::log(t!(
        "flow.launch.log_path",
        path = path.display().to_string()
    ));

    let url = stream_until_url(&child, &mut log_file).await?;

    progress::set_url(url.clone());
    progress::step("launch", StepStatus::Done, format!("dsh web: {url}"));

    Ok(Launch { url, child })
}

/// Stream child output into the log until the URL is found (or timeout/exit).
async fn stream_until_url(child: &AsyncChild, log_file: &mut std::fs::File) -> Result<String> {
    let start = Instant::now();

    loop {
        match child.next_line().await {
            Some(Output::Stdout(line)) | Some(Output::Stderr(line)) => {
                let trimmed = line.trim();
                if !trimmed.is_empty() {
                    let _ = writeln!(log_file, "{line}");
                    progress::log(trimmed);
                }
                if let Some(url) = find_url(&line) {
                    return Ok(url);
                }
            }
            None => {
                // dsh exited without printing a URL.
                let code = child
                    .exit_code()
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "unknown".into());
                return Err(Error(t!("flow.launch.early_exit", code = code).to_string()));
            }
        }

        if start.elapsed() > URL_TIMEOUT {
            return Err(Error(
                t!("flow.launch.url_timeout", secs = URL_TIMEOUT.as_secs()).to_string(),
            ));
        }
    }
}

/// How the supervised dsh process ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DshExit {
    /// dsh exited cleanly (code 0) — the launcher shuts down as before.
    Clean,
    /// dsh exited unexpectedly: a non-zero exit code (e.g. a panic or an OS
    /// crash), or `-1` when it was killed by a signal / the code is unknown.
    Crash(i32),
}

/// Keep draining dsh's output into the log until it exits (supervisor duty),
/// then report how it ended.
pub async fn supervise(child: Arc<AsyncChild>) -> DshExit {
    let path = log_path();
    let mut log_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .ok();

    while let Some(line) = child.next_line().await {
        let text = match line {
            Output::Stdout(l) | Output::Stderr(l) => l,
        };
        if let Some(f) = log_file.as_mut() {
            let _ = writeln!(f, "{text}");
        }
        progress::log(&text);
    }

    // `next_line()` returns `None` only after the process has exited AND both
    // output streams have been drained, so the exit code is available here.
    match child.exit_code() {
        Some(0) => DshExit::Clean,
        Some(code) => DshExit::Crash(code),
        // Killed by a signal (Unix) or the code was never captured.
        None => DshExit::Crash(-1),
    }
}

fn find_url(line: &str) -> Option<String> {
    url_regex().find(line).map(|m| m.as_str().to_string())
}
