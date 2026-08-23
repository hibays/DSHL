//! [`AsyncChild`]: a child process whose stdout/stderr can be awaited line by
//! line.
//!
//! Console-less helpers are spawned through `tokio::process::Command` on every
//! platform (real async pipes; `CREATE_NO_WINDOW` on Windows, PDEATHSIG on
//! Linux come from `capture::to_tokio`). The Windows **console** path
//! ([`AsyncChild::spawn_console`], used for dsh) instead goes through a raw
//! `CreateProcessW` that creates the child's console already hidden — std and
//! tokio would pop a visible window and steal focus. Output is read
//! asynchronously on the shared tokio runtime and delivered through a shared
//! queue; a `Notify` wakes the awaiting consumer. Completion is driven by the
//! process exit, not by pipe EOF — a detached grandchild can inherit the pipe
//! write ends and keep them open after the process itself has exited, so
//! waiting for EOF would hang the stream forever.
//!
//! Windows specifics (hidden-console spawn flags, Ctrl+C signalling, job
//! objects) live in the private [`super::win_proc`] / [`super::win_job`]
//! modules; this module only calls their narrow entry points.

use std::collections::VecDeque;
use std::future::Future;
use std::io;
use std::pin::Pin;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::sync::Notify;

use crate::runtime;

#[cfg(target_os = "windows")]
use super::{win_job, win_proc};

/// One line of process output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Output {
    Stdout(String),
    Stderr(String),
}

/// How long to keep waiting for pipe EOF after the process has exited before
/// declaring the output finished anyway.
///
/// Normal children hit EOF within milliseconds of exiting. Only a detached
/// grandchild that inherited the pipe write ends keeps them open — and bytes
/// written after the process exits are not the process's output, so the stream
/// should not wait for them.
const STREAM_GRACE: Duration = Duration::from_millis(1500);

struct Inner {
    /// The spawned pid, kept separately so `pid()`/`kill()` never contend with
    /// the reaper.
    pid: u32,
    process: Mutex<Option<tokio::process::Child>>,
    lines: Mutex<VecDeque<Output>>,
    /// Exit code (`None` while running).
    code: Mutex<Option<i32>>,
    process_done: AtomicBool,
    /// When the process exited (`None` while running), for the EOF-grace path.
    process_exited_at: Mutex<Option<Instant>>,
    streams_remaining: AtomicUsize,
    streams_done: AtomicBool,
    notify: Notify,
}

impl Inner {
    /// Both the process has exited AND the output has been fully drained.
    ///
    /// Normally that means both pipes hit EOF. But a detached grandchild can
    /// inherit the pipe write ends and keep them open after the process exits,
    /// so EOF never arrives; in that case the stream is considered finished
    /// once a short grace has elapsed since the process exited.
    fn done(&self) -> bool {
        if !self.process_done.load(Ordering::SeqCst) {
            return false;
        }
        if self.streams_done.load(Ordering::SeqCst) {
            return true;
        }
        match *self.process_exited_at.lock().unwrap() {
            Some(at) => at.elapsed() >= STREAM_GRACE,
            None => false,
        }
    }
}

/// A child process whose stdout/stderr can be awaited line by line.
///
/// The pipes are read asynchronously on the tokio runtime; [`AsyncChild::next_line`]
/// returns `None` exactly once the process has exited and every output line has
/// been delivered.
pub struct AsyncChild {
    inner: Arc<Inner>,
}

/// Spawn a prepared tokio command regardless of the caller's async context.
///
/// tokio's Unix pipe registration needs a driver at `spawn()` time and panics
/// with "there is no reactor running" when called from a plain thread;
/// Windows tolerates its absence, which made sync-context spawns pass locally
/// but fail on Linux CI. Inside a runtime we spawn directly; outside one we
/// hop through the global runtime — the returned child's pipes stay bound to
/// it, which is fine because the global runtime lives for `'static`.
fn spawn_in_runtime_context(
    tcmd: &mut tokio::process::Command,
) -> io::Result<tokio::process::Child> {
    if tokio::runtime::Handle::try_current().is_ok() {
        tcmd.spawn()
    } else {
        crate::runtime::block_on(async { tcmd.spawn() })
    }
}

impl AsyncChild {
    /// Spawn with piped stdout/stderr (and no stdin).
    ///
    /// No console window (`CREATE_NO_WINDOW` on Windows). Used for short-lived
    /// helper commands that never need Ctrl+C signalling.
    pub fn spawn(cmd: &mut Command) -> io::Result<Self> {
        Self::spawn_inner(cmd)
    }

    /// Spawn with a hidden console + new process group (Windows) so the child
    /// can later be stopped gracefully via Ctrl+C. On Windows the console is
    /// created already hidden (raw `CreateProcessW`, `STARTF_USESHOWWINDOW |
    /// SW_HIDE`) so the child never flashes or steals focus; on other
    /// platforms falls back to [`spawn`].
    pub fn spawn_console(cmd: &mut Command) -> io::Result<Self> {
        #[cfg(target_os = "windows")]
        {
            Self::spawn_hidden(cmd)
        }
        #[cfg(not(target_os = "windows"))]
        {
            Self::spawn(cmd)
        }
    }

    /// Spawn a console-less helper on the tokio runtime.
    fn spawn_inner(cmd: &mut Command) -> io::Result<Self> {
        // std resolves bare program names against PATH; its spawn settings
        // (creation_flags, etc.) survive the conversion to tokio, which spawns
        // the wrapped std Command.
        let mut tcmd = super::capture::to_tokio(cmd);
        tcmd.stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null());
        // `spawn()` panics on Unix when no tokio runtime context is active
        // (Windows tolerates it) — the helper makes both behave the same.
        let mut child = spawn_in_runtime_context(&mut tcmd)?;
        #[cfg(target_os = "windows")]
        win_job::assign_raw(child.raw_handle().expect("spawned child has a handle"));
        let pid = child.id().expect("spawned child has an id");
        let stdout = child.stdout.take().expect("stdout is piped");
        let stderr = child.stderr.take().expect("stderr is piped");

        let inner = Arc::new(Inner {
            pid,
            process: Mutex::new(Some(child)),
            lines: Mutex::new(VecDeque::new()),
            code: Mutex::new(None),
            process_done: AtomicBool::new(false),
            process_exited_at: Mutex::new(None),
            streams_remaining: AtomicUsize::new(2),
            streams_done: AtomicBool::new(false),
            notify: Notify::new(),
        });

        runtime::spawn(read_lines(inner.clone(), stdout, true));
        runtime::spawn(read_lines(inner.clone(), stderr, false));

        // Reap the process on the runtime (tokio's async wait — no dedicated
        // thread). A second notify after the EOF grace lets a consumer stuck
        // on the Notify wake up even if the pipes never EOF (grandchild-held).
        runtime::spawn({
            let inner = inner.clone();
            async move {
                let taken = inner.process.lock().unwrap().take();
                let code = match taken {
                    Some(mut child) => child.wait().await.ok().and_then(|s| s.code()),
                    None => None,
                };
                *inner.code.lock().unwrap() = code;
                *inner.process_exited_at.lock().unwrap() = Some(Instant::now());
                inner.process_done.store(true, Ordering::SeqCst);
                inner.notify.notify_one();
                tokio::time::sleep(STREAM_GRACE).await;
                inner.notify.notify_one();
            }
        });

        Ok(Self { inner })
    }

    /// Spawn a child with a hidden console (Windows only): the raw
    /// `CreateProcessW` path in [`super::win_proc`] creates the console window
    /// already hidden (`STARTF_USESHOWWINDOW | SW_HIDE`), so the child never
    /// flashes and never steals focus — `std`/`tokio` cannot do this. The raw
    /// process handle is reaped on a blocking thread (tokio cannot wrap a raw
    /// handle), while the piped output is drained by blocking reader threads
    /// feeding the same shared queue as the async path.
    #[cfg(target_os = "windows")]
    fn spawn_hidden(cmd: &mut Command) -> io::Result<Self> {
        let spawned = win_proc::spawn_hidden_console(cmd)?;
        win_job::assign_raw(spawned.process);
        let pid = spawned.pid;

        let inner = Arc::new(Inner {
            pid,
            // The raw process handle is owned by the reaper thread; nothing
            // here waits on a tokio child.
            process: Mutex::new(None),
            lines: Mutex::new(VecDeque::new()),
            code: Mutex::new(None),
            process_done: AtomicBool::new(false),
            process_exited_at: Mutex::new(None),
            streams_remaining: AtomicUsize::new(2),
            streams_done: AtomicBool::new(false),
            notify: Notify::new(),
        });

        spawn_blocking_reader(inner.clone(), spawned.stdout, true);
        spawn_blocking_reader(inner.clone(), spawned.stderr, false);

        // The raw handle is a `*mut c_void` (not `Send`); move it as `usize`
        // so the reaper thread can own and close it.
        let handle = spawned.process as usize;
        std::thread::spawn({
            let inner = inner.clone();
            move || {
                let handle = handle as std::os::windows::io::RawHandle;
                let code = win_proc::wait_handle(handle);
                win_proc::close_handle(handle);
                *inner.code.lock().unwrap() = code;
                *inner.process_exited_at.lock().unwrap() = Some(Instant::now());
                inner.process_done.store(true, Ordering::SeqCst);
                inner.notify.notify_one();
                std::thread::sleep(STREAM_GRACE);
                inner.notify.notify_one();
            }
        });

        Ok(Self { inner })
    }

    /// Await the next stdout/stderr line. `None` once the process exited and
    /// all output has been drained.
    pub fn next_line(&self) -> NextLine<'_> {
        NextLine {
            inner: &self.inner,
            waiting: None,
        }
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
    pub async fn graceful_kill(&self, grace_ms: u64) -> bool {
        self.signal_stop();
        let start = Instant::now();
        let mut last_stop = start;
        let re_send = Duration::from_secs(5);
        while start.elapsed().as_millis() < grace_ms as u128 {
            if !crate::platform::process_alive(self.inner.pid) {
                return true;
            }
            // The first Ctrl+C can be lost (child busy / mid-console-init);
            // re-send periodically so a slow graceful shutdown still happens.
            if last_stop.elapsed() >= re_send {
                last_stop = Instant::now();
                self.signal_stop();
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
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

    /// True once the process itself has exited. Its output pipes may still be
    /// open (a detached grandchild can hold the write ends), so this is weaker
    /// than [`AsyncChild::next_line`] returning `None` — that additionally
    /// waits for both streams to drain (or the EOF grace to elapse).
    pub fn has_exited(&self) -> bool {
        self.inner.process_done.load(Ordering::SeqCst)
    }

    /// Drain all remaining lines, returning the exit code.
    pub async fn drain(self) -> Option<i32> {
        while self.next_line().await.is_some() {}
        self.exit_code()
    }
}

/// Push one read line into the shared queue and wake the consumer.
fn feed_line(inner: &Inner, line: &str, is_stdout: bool) {
    let trimmed = line.trim_end().to_string();
    let out = if is_stdout {
        Output::Stdout(trimmed)
    } else {
        Output::Stderr(trimmed)
    };
    inner.lines.lock().unwrap().push_back(out);
    inner.notify.notify_one();
}

/// Mark one stream as drained; wake the consumer when both pipes hit EOF.
fn stream_done(inner: &Inner) {
    if inner.streams_remaining.fetch_sub(1, Ordering::SeqCst) == 1 {
        inner.streams_done.store(true, Ordering::SeqCst);
        inner.notify.notify_one();
    }
}

/// Read one pipe asynchronously, feeding lines into the shared queue and
/// waking the consumer. Marks the streams done when both pipes hit EOF.
async fn read_lines(
    inner: Arc<Inner>,
    stream: impl AsyncRead + Unpin + Send + 'static,
    is_stdout: bool,
) {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line).await {
            Ok(0) => break,
            Ok(_) => feed_line(&inner, &line, is_stdout),
            Err(_) => break,
        }
    }
    stream_done(&inner);
}

/// Drain a blocking pipe (raw hidden-console spawn's stdio) line by line into
/// the shared queue — the same contract as [`read_lines`], but for the plain
/// `std::fs::File` handles `CreateProcessW` hands us (no tokio child to wait
/// on, so the reads run on plain threads).
#[cfg(target_os = "windows")]
fn spawn_blocking_reader(
    inner: Arc<Inner>,
    stream: impl std::io::Read + Send + 'static,
    is_stdout: bool,
) {
    std::thread::spawn(move || {
        use std::io::{BufRead, BufReader};
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) => break,
                Ok(_) => feed_line(&inner, &line, is_stdout),
                Err(_) => break,
            }
        }
        stream_done(&inner);
    });
}

/// Future returned by [`AsyncChild::next_line`].
///
/// The waiting state is a tokio `Notify` waiter; every wake re-checks the queue
/// (and the done flag) so a line that arrived before the waiter was armed is
/// never skipped. Safe: no custom wakers involved.
pub struct NextLine<'a> {
    inner: &'a Inner,
    waiting: Option<Pin<Box<dyn Future<Output = ()> + Send + 'a>>>,
}

impl Future for NextLine<'_> {
    type Output = Option<Output>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        loop {
            // Serve any pending lines first.
            if let Some(line) = self.inner.lines.lock().unwrap().pop_front() {
                return Poll::Ready(Some(line));
            }

            // The process exited and the output was drained (or the EOF grace
            // elapsed): finished.
            if self.inner.done() {
                return Poll::Ready(None);
            }

            // Ensure a waiter is registered on the notify before parking.
            if self.waiting.is_none() {
                self.waiting = Some(Box::pin(self.inner.notify.notified()));
            }
            if self.waiting.as_mut().unwrap().as_mut().poll(cx).is_ready() {
                // The notification was already pending (a permit stored before
                // we registered — e.g. a line notify racing this poll). The
                // permit is now consumed; drop the waiter and loop to re-check
                // the queue/done and register a FRESH waiter. Returning Pending
                // while `self.waiting` is None (or a consumed waiter) would be
                // a lost wakeup: a later notify_one() would find no waiter and
                // park us forever.
                self.waiting = None;
                continue;
            }
            return Poll::Pending;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn drains_despite_lingering_grandchild() {
        // A child that spawns a detached grandchild inheriting its stdout
        // write end, then exits. The grandchild keeps the pipe open, so EOF
        // never arrives — completion must be driven by the process exit, not
        // pipe EOF.
        #[cfg(windows)]
        let mut cmd = Command::new(std::env::var("COMSPEC").unwrap_or_else(|_| "cmd".into()));
        #[cfg(windows)]
        cmd.args(["/c", "echo hello & start /b cmd /c timeout /t 60 & exit"]);
        // Unix twin: a backgrounded sleep inherits the stdout write end and
        // plays the lingering grandchild.
        #[cfg(not(windows))]
        let mut cmd = Command::new("sh");
        #[cfg(not(windows))]
        cmd.args(["-c", "echo hello & sleep 60 & exit 0"]);
        let child = AsyncChild::spawn(&mut cmd).expect("spawn");
        let pid = child.pid().expect("pid");
        let result = crate::runtime::block_on(async {
            tokio::time::timeout(Duration::from_secs(5), async {
                let mut lines = Vec::new();
                while let Some(l) = child.next_line().await {
                    lines.push(l);
                }
                lines
            })
            .await
        });
        crate::platform::kill_tree(pid);
        match result {
            Ok(lines) => {
                eprintln!("lines: {lines:?}");
                assert!(
                    child.exit_code() == Some(0),
                    "exit code: {:?}",
                    child.exit_code()
                );
            }
            Err(_) => panic!("drain timed out — pipes never EOF even though the process exited"),
        }
    }

    #[test]
    fn drains_simple_child() {
        #[cfg(windows)]
        let mut cmd = Command::new(std::env::var("COMSPEC").unwrap_or_else(|_| "cmd".into()));
        #[cfg(windows)]
        cmd.args(["/c", "echo hello & echo world"]);
        #[cfg(not(windows))]
        let mut cmd = Command::new("sh");
        #[cfg(not(windows))]
        cmd.args(["-c", "echo hello; echo world"]);
        let child = AsyncChild::spawn(&mut cmd).expect("spawn");
        let result = crate::runtime::block_on(async {
            tokio::time::timeout(Duration::from_secs(5), async {
                let mut lines = Vec::new();
                while let Some(l) = child.next_line().await {
                    lines.push(l);
                }
                lines
            })
            .await
        });
        match result {
            Ok(lines) => {
                assert_eq!(
                    lines,
                    vec![
                        Output::Stdout("hello".into()),
                        Output::Stdout("world".into())
                    ]
                );
                assert!(
                    child.exit_code() == Some(0),
                    "exit code: {:?}",
                    child.exit_code()
                );
            }
            Err(_) => panic!("drain timed out — pipes never EOF / done never set"),
        }
    }
}
