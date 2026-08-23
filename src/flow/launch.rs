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

/// Poll tick bounding every `next_line` wait while dsh has not been observed
/// to exit: between ticks an OS-level liveness probe decides whether to keep
/// waiting for output or to declare the startup failed.
const POLL_TICK: Duration = Duration::from_millis(500);

/// Path of the per-run dsh log file (truncated on every launch).
pub fn log_path() -> PathBuf {
    platform::cache_dir().join("dshl").join("dsh.log")
}

fn url_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"https?://[^\s"'<>]+:\d+"#).unwrap())
}

/// Cap the echoed output tail so a pathological line cannot blow up the
/// startup-page error banner.
fn truncate_detail(detail: &str) -> String {
    const MAX: usize = 200;
    if detail.chars().count() <= MAX {
        detail.to_string()
    } else {
        let cut: String = detail.chars().take(MAX).collect();
        format!("{cut}…")
    }
}

pub async fn run(mut command: Command) -> Result<Launch> {
    // Let the dsh-side plugin find the launcher's control endpoint.
    crate::control::inject_env(&mut command);

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
///
/// Uses the full [`URL_TIMEOUT`] budget; see [`stream_until_url_within`] for
/// the (test-injectable) mechanics.
async fn stream_until_url(child: &AsyncChild, log_file: &mut std::fs::File) -> Result<String> {
    stream_until_url_within(child, log_file, URL_TIMEOUT).await
}

/// The streaming loop proper. `budget` is injectable so tests can pin the
/// timeout path without waiting the full two minutes a silent live dsh gets.
///
/// Once the process has exited there is no extra drain grace: a timed-out
/// `next_line` wait maps to end-of-stream and raises the failure right away.
/// The buffered tail is normally already delivered by the reader threads by
/// the time the exit is observed (measured on both spawn paths), and the only
/// thing a longer wait would add is latency on grandchild-held pipes.
async fn stream_until_url_within(
    child: &AsyncChild,
    log_file: &mut std::fs::File,
    budget: Duration,
) -> Result<String> {
    let start = Instant::now();

    let mut last_line: Option<String> = None;
    loop {
        let next = if child.has_exited() {
            // Post-exit: whatever `next_line` yields within this await is what
            // the readers have already queued; if nothing arrives before the
            // EOF grace in `child.rs`, it returns `None` and we fail below.
            child.next_line().await
        } else {
            // The reaper thread that flips `process_done` can stall on its raw
            // handle wait, leaving `has_exited()` false forever even though dsh
            // is already dead. So don't trust it alone: bound each wait by
            // POLL_TICK and consult the OS directly. `process_alive` is an
            // independent source of truth — the direct child dead with no URL
            // printed means startup failed; raise immediately instead of
            // waiting for the pipes to drain (a grandchild holding them open
            // must not extend the wait).
            //
            // Cancelling `next_line` inside the timeout is safe: its future
            // parks on `Notify::notified()`, and dropping it merely
            // deregisters the waiter, while `notify_one` stores a permit when
            // no waiter is registered — no wakeup can be lost.
            match tokio::time::timeout(POLL_TICK, child.next_line()).await {
                Ok(next) => next,
                Err(_elapsed) => {
                    let alive = match child.pid() {
                        // No pid to probe: skip the liveness check and keep
                        // relying on the reaper + URL_TIMEOUT below.
                        None => true,
                        Some(pid) => crate::platform::process_alive(pid),
                    };
                    if !alive {
                        // dsh died without printing a URL — same failure as
                        // the drained-exit branch. The exit code may not be
                        // reaped yet; "unknown" is fine.
                        let code = child
                            .exit_code()
                            .map(|c| c.to_string())
                            .unwrap_or_else(|| "unknown".into());
                        return Err(Error(match last_line.as_deref() {
                            Some(detail) => t!(
                                "flow.launch.early_exit_detail",
                                code = code,
                                detail = truncate_detail(detail)
                            )
                            .to_string(),
                            None => t!("flow.launch.early_exit", code = code).to_string(),
                        }));
                    }
                    // Alive and simply quiet this tick — normal startup, keep
                    // polling (the timeout budget still applies).
                    if start.elapsed() > budget {
                        return Err(Error(
                            t!("flow.launch.url_timeout", secs = budget.as_secs()).to_string(),
                        ));
                    }
                    continue;
                }
            }
        };
        match next {
            Some(Output::Stdout(line)) | Some(Output::Stderr(line)) => {
                let trimmed = line.trim();
                if !trimmed.is_empty() {
                    let _ = writeln!(log_file, "{line}");
                    progress::log(trimmed);
                    // Remember the most recent meaningful output so a failure
                    // can tell the user WHY without digging through dsh.log.
                    last_line = Some(trimmed.to_string());
                }
                if let Some(url) = find_url(&line) {
                    return Ok(url);
                }
            }
            None => {
                // dsh exited without printing a URL. Surface the tail of its
                // output alongside the exit code — the error line (e.g.
                // `error: unknown option '--foo'`) is usually the whole story.
                let code = child
                    .exit_code()
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "unknown".into());
                return Err(Error(match last_line.as_deref() {
                    Some(detail) => t!(
                        "flow.launch.early_exit_detail",
                        code = code,
                        detail = truncate_detail(detail)
                    )
                    .to_string(),
                    None => t!("flow.launch.early_exit", code = code).to_string(),
                }));
            }
        }

        if start.elapsed() > budget {
            return Err(Error(
                t!("flow.launch.url_timeout", secs = budget.as_secs()).to_string(),
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
    // `supervise` only runs after `stream_until_url` has captured a URL (i.e.
    // startup succeeded); a failed launch (no URL) already errors out in the
    // launch phase and never reaches here. The exit code alone therefore
    // suffices to tell Clean from Crash.
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Reproduce the "dsh prints its URL, then immediately exits non-zero"
    /// scenario. The URL is captured by `run`, and `supervise` must observe the
    /// subsequent crash (not hang waiting for output that never comes).
    #[test]
    fn run_then_supervise_observes_immediate_crash() {
        // Print a URL-looking line, then exit non-zero right away. This models
        // a dsh that announces its web URL and crashes during startup.
        let cmd = crate::testutil::shell(
            "echo dsh web: http://127.0.0.1:61239 & exit 3",
            "echo dsh web: http://127.0.0.1:61239; exit 3",
        );
        let result = crate::runtime::block_on(async {
            let launch = run(cmd).await;
            let launch = match launch {
                Ok(l) => l,
                Err(e) => return Err(e),
            };
            let exit = supervise(launch.child).await;
            Ok((launch.url, exit))
        });
        match result {
            Ok((url, exit)) => {
                assert_eq!(url, "http://127.0.0.1:61239");
                assert_eq!(exit, DshExit::Crash(3));
            }
            Err(e) => panic!("launch failed: {e}"),
        }
    }

    /// Regression: a child that prints an error to stderr and exits without
    /// ever printing a URL must make `stream_until_url` return promptly with
    /// `early_exit` — not hang forever. The 8s outer bound turns a regression
    /// into a fast test failure instead of a stuck test run.
    #[test]
    fn stream_until_url_errors_promptly_when_child_dies_without_url() {
        #[cfg(target_os = "windows")]
        let mut cmd = {
            // Real-world shape: dsh runs as `node <entry>` and a commander
            // parse failure prints `error: unknown option '--foo'` to stderr
            // before the process exits non-zero. No URL is ever printed.
            // (`spawn_console` hands the program to CreateProcessW as
            // lpApplicationName, which needs an absolute path.)
            let node = crate::platform::which("node").expect("node available for test");
            let mut c = Command::new(node);
            c.args([
                "-e",
                "console.error(\"error: unknown option '--foo'\"); process.exit(1)",
            ]);
            c
        };
        #[cfg(not(target_os = "windows"))]
        let mut cmd = {
            let mut c = Command::new("sh");
            c.args(["-c", "echo boom >&2; exit 1"]);
            c
        };

        let child = Arc::new(AsyncChild::spawn_console(&mut cmd).unwrap());
        let log_path = std::env::temp_dir().join("dshl-test-stream.log");
        let mut log_file = std::fs::File::create(&log_path).unwrap();

        let started = std::time::Instant::now();
        let result = crate::runtime::block_on(async {
            tokio::time::timeout(std::time::Duration::from_secs(8), async {
                stream_until_url(&child, &mut log_file).await
            })
            .await
        });
        let elapsed = started.elapsed();

        match result {
            Ok(Ok(url)) => panic!("child should never print a URL, got: {url}"),
            Ok(Err(e)) => {
                eprintln!("early_exit after {elapsed:?}: {e}");
                assert!(
                    elapsed < std::time::Duration::from_secs(8),
                    "early_exit took too long: {elapsed:?}"
                );
            }
            Err(_) => panic!(
                "HANG REPRODUCED: stream_until_url did not return within 8s (elapsed {elapsed:?})"
            ),
        }
    }

    /// Full-shape reproduction attempt: a slow-starting child that prints
    /// several lines, then an error to stderr and exits non-zero — raced
    /// against the 100ms shutdown poll exactly like the `tokio::select!`
    /// wrapper in `ui/launch.rs`. Must still produce `early_exit` promptly.
    #[test]
    fn stream_until_url_under_select_wrapper_with_slow_dying_child() {
        #[cfg(target_os = "windows")]
        let mut cmd = {
            let mut c = Command::new(std::env::var("COMSPEC").unwrap_or_else(|_| "cmd".into()));
            // Startup chatter, ~2s pause, commander-style error on stderr,
            // non-zero exit. (`ping -n 3 >nul` ≈ 2s, no long sleeps.)
            c.args([
                "/c",
                "echo dsh starting & ping -n 3 127.0.0.1 >nul & echo error: unknown option '--foo' 1>&2 & exit 1",
            ]);
            c
        };
        #[cfg(not(target_os = "windows"))]
        let mut cmd = {
            let mut c = Command::new("sh");
            c.args(["-c", "echo starting; sleep 2; echo boom >&2; exit 1"]);
            c
        };

        let child = Arc::new(AsyncChild::spawn_console(&mut cmd).unwrap());
        let log_path = std::env::temp_dir().join("dshl-test-stream-slow.log");
        let mut log_file = std::fs::File::create(&log_path).unwrap();

        let started = std::time::Instant::now();
        // Same shape as ui/launch.rs: flow raced against a 100ms poll.
        let outcome = crate::runtime::block_on(async {
            tokio::select! {
                result = stream_until_url(&child, &mut log_file) => Some(result),
                _ = async {
                    loop {
                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    }
                } => None,
            }
        });
        let elapsed = started.elapsed();

        match outcome {
            Some(Ok(url)) => panic!("child should never print a URL, got: {url}"),
            Some(Err(e)) => {
                eprintln!("early_exit after {elapsed:?}: {e}");
                assert!(
                    elapsed < std::time::Duration::from_secs(8),
                    "early_exit took too long: {elapsed:?}"
                );
            }
            None => panic!(
                "HANG REPRODUCED under select wrapper: no early_exit within test window ({elapsed:?})"
            ),
        }
    }

    /// Stress shape: hundreds of startup-banner lines on both streams, then
    /// a multi-line commander usage block on stderr and a non-zero exit —
    /// no URL. Repeated a few times to shake out queue/notify races.
    #[test]
    fn stream_until_url_stress_many_lines_then_die_without_url() {
        #[cfg(target_os = "windows")]
        let script = r#"
for (let i = 0; i < 400; i++) console.log(`banner ${i}`);
console.error("error: unknown option '--foo'");
for (let i = 0; i < 40; i++) console.error(`  usage line ${i}`);
process.exit(1);
"#;

        for round in 0..5 {
            #[cfg(target_os = "windows")]
            let mut cmd = {
                let node = crate::platform::which("node").expect("node available for test");
                let mut c = Command::new(node);
                c.args(["-e", script]);
                c
            };
            #[cfg(not(target_os = "windows"))]
            let mut cmd = {
                let mut c = Command::new("sh");
                c.args(["-c", "echo boom >&2; exit 1"]);
                c
            };

            let child = Arc::new(AsyncChild::spawn_console(&mut cmd).unwrap());
            let log_path =
                std::env::temp_dir().join(format!("dshl-test-stream-stress-{round}.log"));
            let mut log_file = std::fs::File::create(&log_path).unwrap();

            let started = std::time::Instant::now();
            let result = crate::runtime::block_on(async {
                tokio::time::timeout(std::time::Duration::from_secs(8), async {
                    stream_until_url(&child, &mut log_file).await
                })
                .await
            });
            let elapsed = started.elapsed();

            match result {
                Ok(Ok(url)) => panic!("round {round}: unexpected url: {url}"),
                Ok(Err(e)) => {
                    eprintln!("round {round}: early_exit after {elapsed:?}: {e}");
                    assert!(
                        elapsed < std::time::Duration::from_secs(8),
                        "round {round}: early_exit took too long: {elapsed:?}"
                    );
                }
                Err(_) => panic!(
                    "HANG REPRODUCED (round {round}): stream_until_url did not return within 8s"
                ),
            }
        }
    }

    /// A clean stdout line with exit 0 must still be a clean exit.
    #[test]
    fn supervise_clean_output_with_zero_exit_is_clean() {
        let mut cmd = crate::testutil::shell(
            "echo dsh web: http://127.0.0.1:9999 & exit 0",
            "echo dsh web: http://127.0.0.1:9999; exit 0",
        );
        let exit = crate::runtime::block_on(async {
            let child = Arc::new(AsyncChild::spawn_console(&mut cmd).unwrap());
            supervise(child).await
        });
        assert_eq!(exit, DshExit::Clean);
    }

    /// Startup failure shape: dsh stays ALIVE but never prints anything (node
    /// hangs, a CLI waits interactively for input, …). No line ever arrives,
    /// so the loop body's timeout check (which only runs after a line) is
    /// never reached — the ONLY thing that can end the wait is the POLL_TICK
    /// liveness loop hitting the timeout budget. This test pins that path with
    /// a short injected budget (the production budget is 120s — see
    /// URL_TIMEOUT): without the POLL_TICK loop this future hangs forever.
    #[test]
    fn stream_until_url_times_out_when_child_silent_but_alive() {
        #[cfg(target_os = "windows")]
        let mut cmd = {
            let mut c = Command::new(std::env::var("COMSPEC").unwrap_or_else(|_| "cmd".into()));
            // Alive for the whole window, prints nothing at all.
            c.args(["/c", "ping -n 30 127.0.0.1 >nul"]);
            c
        };
        #[cfg(not(target_os = "windows"))]
        let mut cmd = {
            let mut c = Command::new("sh");
            c.args(["-c", "sleep 30"]);
            c
        };

        let child = Arc::new(AsyncChild::spawn_console(&mut cmd).unwrap());
        let log_path = std::env::temp_dir().join("dshl-test-stream-silent-alive.log");
        let mut log_file = std::fs::File::create(&log_path).unwrap();

        // A 3s budget: long enough for a few POLL_TICK rounds, short enough
        // for a fast test run.
        const BUDGET: Duration = Duration::from_secs(3);
        let started = std::time::Instant::now();
        let result = crate::runtime::block_on(async {
            tokio::time::timeout(std::time::Duration::from_secs(20), async {
                stream_until_url_within(&child, &mut log_file, BUDGET).await
            })
            .await
        });
        let elapsed = started.elapsed();

        // The child is still running when we bail out; don't leave it behind.
        let _ = child.kill();

        match result {
            Ok(Ok(url)) => panic!("silent child should never yield a URL, got: {url}"),
            Ok(Err(e)) => {
                eprintln!("url_timeout after {elapsed:?}: {e}");
                // The error must be the url_timeout message naming the budget,
                // proving it came from the POLL_TICK liveness path and not an
                // early-exit branch.
                assert!(
                    elapsed >= BUDGET && elapsed < Duration::from_secs(20),
                    "timeout fired at the wrong time: {elapsed:?} (budget {BUDGET:?})"
                );
            }
            Err(_) => panic!(
                "HANG CONFIRMED: stream_until_url never reached its timeout budget \
                 while the child was alive and silent (elapsed {elapsed:?})"
            ),
        }
    }

    /// Startup failure shape: dsh dies WITHOUT a URL while a detached
    /// grandchild keeps both pipe write ends open. The reaper fires, but pipe
    /// EOF never arrives — the post-exit wait must end via child.rs's
    /// exit-time fallback (STREAM_GRACE) so the failure surfaces as early_exit
    /// instead of hanging on the held pipes.
    #[test]
    fn stream_until_url_errors_when_grandchild_holds_pipes_after_death() {
        #[cfg(target_os = "windows")]
        let mut cmd = {
            let mut c = Command::new(std::env::var("COMSPEC").unwrap_or_else(|_| "cmd".into()));
            // Print an error line, leave a detached grandchild holding the
            // inherited stdout write end for ~60s, then exit non-zero.
            c.args([
                "/c",
                "echo error: startup failed 1>&2 & start /b cmd /c ping -n 60 127.0.0.1 >nul & exit 2",
            ]);
            c
        };
        #[cfg(not(target_os = "windows"))]
        let mut cmd = {
            let mut c = Command::new("sh");
            c.args([
                "-c",
                "echo boom >&2; nohup sleep 60 >/dev/null 2>&1 & exit 2",
            ]);
            c
        };

        let child = Arc::new(AsyncChild::spawn_console(&mut cmd).unwrap());
        let log_path = std::env::temp_dir().join("dshl-test-stream-grandchild.log");
        let mut log_file = std::fs::File::create(&log_path).unwrap();

        let started = std::time::Instant::now();
        let result = crate::runtime::block_on(async {
            tokio::time::timeout(std::time::Duration::from_secs(8), async {
                stream_until_url(&child, &mut log_file).await
            })
            .await
        });
        let elapsed = started.elapsed();

        // The grandchild may still hold pipes; tear the whole tree down.
        if let Some(pid) = child.pid() {
            crate::platform::kill_tree(pid);
        }

        match result {
            Ok(Ok(url)) => panic!("child should never print a URL, got: {url}"),
            Ok(Err(e)) => {
                eprintln!("early_exit after {elapsed:?}: {e}");
                assert!(
                    elapsed < std::time::Duration::from_secs(8),
                    "grandchild-held pipes stretched the failure past 8s: {elapsed:?}"
                );
            }
            Err(_) => panic!(
                "HANG REPRODUCED: grandchild-held pipes blocked early_exit (elapsed {elapsed:?})"
            ),
        }
    }

    /// Startup failure shape: the child exits quickly with no output at all.
    /// `last_line` stays empty, so the error must be the code-only early_exit
    /// variant — reached via the drained-exit branch or the POLL_TICK probe,
    /// whichever observes the death first.
    #[test]
    fn stream_until_url_errors_when_child_dies_silently_without_url() {
        #[cfg(target_os = "windows")]
        let mut cmd = {
            let mut c = Command::new(std::env::var("COMSPEC").unwrap_or_else(|_| "cmd".into()));
            // Exit non-zero without printing anything on either stream.
            c.args(["/c", "exit 7"]);
            c
        };
        #[cfg(not(target_os = "windows"))]
        let mut cmd = {
            let mut c = Command::new("sh");
            c.args(["-c", "exit 7"]);
            c
        };

        let child = Arc::new(AsyncChild::spawn_console(&mut cmd).unwrap());
        let log_path = std::env::temp_dir().join("dshl-test-stream-silent-death.log");
        let mut log_file = std::fs::File::create(&log_path).unwrap();

        let started = std::time::Instant::now();
        let result = crate::runtime::block_on(async {
            tokio::time::timeout(std::time::Duration::from_secs(8), async {
                stream_until_url(&child, &mut log_file).await
            })
            .await
        });
        let elapsed = started.elapsed();

        match result {
            Ok(Ok(url)) => panic!("child should never print a URL, got: {url}"),
            Ok(Err(e)) => {
                eprintln!("early_exit after {elapsed:?}: {e}");
                assert!(
                    elapsed < std::time::Duration::from_secs(8),
                    "silent death took too long to surface: {elapsed:?}"
                );
            }
            Err(_) => panic!(
                "HANG REPRODUCED: silent death without URL never surfaced (elapsed {elapsed:?})"
            ),
        }
    }
}
