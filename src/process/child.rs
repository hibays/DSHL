//! [`AsyncChild`]: a child process whose stdout/stderr can be awaited line by
//! line.
//!
//! Two reader threads feed a shared queue while a third thread reaps the
//! process and, only after both streams have been fully drained, marks the
//! child as done. This makes the async contract sound: [`AsyncChild::next_line`]
//! returns `None` exactly once every output line has been delivered.
//!
//! Windows specifics (hidden-console spawn, Ctrl+C signalling, job objects)
//! live in the private [`super::win_proc`] / [`super::win_job`] modules; this
//! module only calls their narrow entry points.

use std::collections::VecDeque;
use std::future::Future;
use std::io::{self, BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

#[cfg(target_os = "windows")]
use super::{win_job, win_proc};

/// One line of process output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Output {
    Stdout(String),
    Stderr(String),
}

/// How the child process handle is held.
#[cfg(target_os = "windows")]
enum ProcessKind {
    Std(Child),
    /// Raw `HANDLE` stored as `usize` so it is `Send + Sync`.
    Raw(usize),
}
#[cfg(not(target_os = "windows"))]
enum ProcessKind {
    Std(Child),
}

struct Inner {
    /// The spawned pid, kept separately so `pid()`/`kill()` never contend with
    /// the reaper thread's `wait()`.
    pid: u32,
    process: Mutex<Option<ProcessKind>>,
    lines: Mutex<VecDeque<Output>>,
    done: Mutex<bool>,
    /// Exit code (`None` while running).
    code: Mutex<Option<i32>>,
    waker: Mutex<Option<Waker>>,
}

fn wake(inner: &Inner) {
    if let Some(w) = inner.waker.lock().unwrap().take() {
        w.wake();
    }
}

/// A child process whose stdout/stderr can be awaited line by line.
///
/// Two reader threads feed a shared queue while a third thread reaps the
/// process and, only after both streams have been fully drained, marks the
/// child as done. This makes the async contract sound: [`AsyncChild::next_line`]
/// returns `None` exactly once every output line has been delivered.
pub struct AsyncChild {
    inner: Arc<Inner>,
}

impl AsyncChild {
    /// Spawn with piped stdout/stderr (and no stdin).
    pub fn spawn(cmd: &mut Command) -> io::Result<Self> {
        super::capture::prepare_spawn(cmd);

        let mut child = cmd
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null())
            .spawn()?;

        // On Windows, assign the child to a kill-on-close job object so it is
        // reaped automatically if the launcher is terminated abruptly.
        #[cfg(target_os = "windows")]
        win_job::assign(&child);

        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();
        let pid = child.id();

        let inner = Arc::new(Inner {
            pid,
            process: Mutex::new(Some(ProcessKind::Std(child))),
            lines: Mutex::new(VecDeque::new()),
            done: Mutex::new(false),
            code: Mutex::new(None),
            waker: Mutex::new(None),
        });

        Self::start_readers(inner, stdout, stderr)
    }

    /// Spawn with a hidden console + new process group (Windows) so the child
    /// can later be stopped gracefully via Ctrl+C. Falls back to [`spawn`] on
    /// non-Windows.
    pub fn spawn_console(cmd: &mut Command) -> io::Result<Self> {
        #[cfg(target_os = "windows")]
        {
            let spawned = win_proc::spawn_hidden_console(cmd)?;
            win_job::assign_raw(spawned.process);
            let inner = Arc::new(Inner {
                pid: spawned.pid,
                process: Mutex::new(Some(ProcessKind::Raw(spawned.process as usize))),
                lines: Mutex::new(VecDeque::new()),
                done: Mutex::new(false),
                code: Mutex::new(None),
                waker: Mutex::new(None),
            });
            Self::start_readers(inner, spawned.stdout, spawned.stderr)
        }
        #[cfg(not(target_os = "windows"))]
        {
            Self::spawn(cmd)
        }
    }

    fn start_readers(
        inner: Arc<Inner>,
        stdout: impl std::io::Read + Send + 'static,
        stderr: impl std::io::Read + Send + 'static,
    ) -> io::Result<Self> {
        let h1 = spawn_reader(inner.clone(), stdout, true);
        let h2 = spawn_reader(inner.clone(), stderr, false);

        {
            let inner = inner.clone();
            std::thread::spawn(move || {
                // Take the process out of the mutex first so `pid()`/`kill()` are
                // never blocked while this thread waits.
                let taken = inner.process.lock().unwrap().take();
                let code = match taken {
                    Some(ProcessKind::Std(mut c)) => c.wait().ok().and_then(|s| s.code()),
                    #[cfg(target_os = "windows")]
                    Some(ProcessKind::Raw(h)) => {
                        win_proc::wait_handle(h as std::os::windows::io::RawHandle)
                    }
                    None => None,
                };
                // Drain both streams completely before declaring done.
                let _ = h1.join();
                let _ = h2.join();
                *inner.code.lock().unwrap() = code;
                *inner.done.lock().unwrap() = true;
                wake(&inner);
            });
        }

        Ok(Self { inner })
    }

    /// Await the next stdout/stderr line. `None` once the process exited and
    /// all output has been drained.
    pub fn next_line(&self) -> NextLine<'_> {
        NextLine { inner: &self.inner }
    }

    /// Process id of the spawned child.
    pub fn pid(&self) -> Option<u32> {
        Some(self.inner.pid)
    }

    /// Send a graceful stop signal: Ctrl+C on Windows, SIGTERM on Unix.
    pub fn signal_stop(&self) {
        #[cfg(target_os = "windows")]
        win_proc::send_ctrl_c(self.inner.pid);
        #[cfg(unix)]
        {
            // SAFETY: kill(pid, SIGTERM) sends a catchable termination signal.
            unsafe { libc::kill(self.inner.pid as libc::pid_t, libc::SIGTERM) };
        }
    }

    /// Force-kill the process (best-effort).
    pub fn kill(&self) -> io::Result<()> {
        crate::platform::kill_tree(self.inner.pid);
        Ok(())
    }

    /// Gracefully stop the child (Ctrl+C on Windows / SIGTERM on Unix) and
    /// wait up to `grace_ms` for it to exit on its own, re-sending the stop
    /// signal every few seconds while it is still alive.
    ///
    /// The process is **never** force-killed here: Ctrl+C is the correct way
    /// to close dsh — it commits its session log during shutdown and its own
    /// shutdown logic force-exits at most 5s after the signal. A forced kill
    /// could interrupt that write; and the permanent "corrupt session log:
    /// seq gap" damage comes from TWO processes appending to the same log, so
    /// callers must wait for the child to actually exit (or abort the launch)
    /// before starting a replacement. Returns `true` when the child exited
    /// within the grace period.
    pub fn graceful_kill(&self, grace_ms: u64) -> bool {
        self.signal_stop();
        let start = std::time::Instant::now();
        let mut last_stop = start;
        let re_send = std::time::Duration::from_secs(5);
        while start.elapsed().as_millis() < grace_ms as u128 {
            if !crate::platform::process_alive(self.inner.pid) {
                return true;
            }
            // The first Ctrl+C can be lost (child busy / mid-console-init);
            // re-send periodically so a slow graceful shutdown still happens.
            if last_stop.elapsed() >= re_send {
                last_stop = std::time::Instant::now();
                self.signal_stop();
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
        let alive = crate::platform::process_alive(self.inner.pid);
        if alive {
            crate::debug::emit(&format!(
                "graceful_kill: pid {} still alive after {grace_ms}ms; left running (no force kill)",
                self.inner.pid
            ));
        }
        !alive
    }

    /// Exit code once the process has finished (`None` while running).
    pub fn exit_code(&self) -> Option<i32> {
        *self.inner.code.lock().unwrap()
    }

    /// Drain all remaining lines, returning the exit code.
    pub async fn drain(self) -> Option<i32> {
        while self.next_line().await.is_some() {}
        self.exit_code()
    }
}

fn spawn_reader(
    inner: Arc<Inner>,
    stream: impl std::io::Read + Send + 'static,
    is_stdout: bool,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let reader = BufReader::new(stream);
        for line in reader.lines() {
            match line {
                Ok(l) => {
                    let out = if is_stdout {
                        Output::Stdout(l)
                    } else {
                        Output::Stderr(l)
                    };
                    inner.lines.lock().unwrap().push_back(out);
                    wake(&inner);
                }
                Err(_) => break,
            }
        }
    })
}

/// Future returned by [`AsyncChild::next_line`].
pub struct NextLine<'a> {
    inner: &'a Inner,
}

impl Future for NextLine<'_> {
    type Output = Option<Output>;

    fn poll(self: std::pin::Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let line = self.inner.lines.lock().unwrap().pop_front();
        if let Some(l) = line {
            return Poll::Ready(Some(l));
        }
        if *self.inner.done.lock().unwrap() {
            return Poll::Ready(None);
        }
        *self.inner.waker.lock().unwrap() = Some(cx.waker().clone());
        Poll::Pending
    }
}
