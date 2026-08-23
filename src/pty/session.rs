//! Per-session state, shell/env helpers, and the public PTY API.
//!
//! Each spawned shell gets a [`Session`] entry in the global registry,
//! a reader thread (PTY master → broadcast), a control thread (resize/kill
//! via the master handle), and a child-wait thread (reap + cleanup).

use std::{
    collections::HashMap,
    env,
    io::{self, Read, Write},
    path::PathBuf,
    sync::{
        Arc, LazyLock, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{SystemTime, UNIX_EPOCH},
};

use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};
use tokio::sync::broadcast;
use uuid::Uuid;

use crate::platform::{Shell, shell};

use super::server::ensure_ws_server;
use super::types::*;

// ---------------------------------------------------------------------------
// Globals
// ---------------------------------------------------------------------------

type SessionsMap = HashMap<String, Arc<Session>>;
pub(super) static SESSIONS: LazyLock<Mutex<SessionsMap>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Lock the session registry, tolerating a poisoned mutex: the registry is a
/// plain map, so recovering the inner guard after another thread panicked is
/// strictly better than panicking here (which, on the napi track, would abort
/// the host Node process). Mirrors the `WS_SERVER` policy in server.rs.
pub(super) fn sessions() -> std::sync::MutexGuard<'static, SessionsMap> {
    SESSIONS.lock().unwrap_or_else(|p| p.into_inner())
}

pub(super) struct Session {
    pub id: String,
    pub pid: u32,
    pub shell: String,
    pub cwd: String,
    pub started_at_ms: u64,
    pub alive: AtomicBool,
    /// Broadcast channel for *outbound* PTY bytes → all currently connected WS
    /// subscribers on this session subscribe here.
    pub outbound: broadcast::Sender<Vec<u8>>,
    /// Inbound *control* (resize/kill) from any WS subscriber (last writer wins
    /// for resize, kill is idempotent). A plain std mpsc — every producer
    /// (WS handler, FFI resize/kill) sends synchronously and never awaits, so
    /// the tokio-unbounded + std-bridge rig is unnecessary.
    pub control_tx: mpsc::Sender<ControlMsg>,
    /// Direct stdin writer (bypasses WS for FFI callers).
    pub stdin_tx: Mutex<Box<dyn Write + Send>>,
    /// Forcibly drop the session entry when the child exits — kept alive by
    /// the wait thread.
    pub wait_handle: Mutex<Option<std::thread::JoinHandle<()>>>,
}

// ---------------------------------------------------------------------------
// Shell / env helpers
// ---------------------------------------------------------------------------

fn shell_command_for(preferred: Option<&str>) -> (String, Vec<String>) {
    if let Some(s) = preferred.filter(|s| !s.is_empty()) {
        return (s.to_string(), vec![]);
    }
    match shell() {
        Shell::PowerShell => ("powershell.exe".into(), vec!["-NoLogo".into()]),
        Shell::Cmd => ("cmd.exe".into(), vec![]),
        Shell::Bash => ("/bin/bash".into(), vec!["-l".into()]),
        Shell::Sh => ("/bin/sh".into(), vec!["-l".into()]),
    }
}

fn cwd_for(asked: Option<&str>) -> PathBuf {
    asked
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .or_else(|| env::current_dir().ok())
        .or_else(env::home_dir)
        // Never a compile-time path: env!("CARGO_MANIFEST_DIR") bakes the
        // BUILD machine's checkout into the release binary (breaking
        // reproducible builds and pointing at a directory that will not
        // exist on any user machine, turning this fallback into a
        // guaranteed spawn failure). temp_dir always exists.
        .unwrap_or_else(env::temp_dir)
}

fn resolved_path(prepend: &[String]) -> String {
    let orig = env::var_os("PATH").unwrap_or_default();
    let orig = orig.to_string_lossy().to_string();
    if prepend.is_empty() {
        return orig;
    }
    let sep = if cfg!(windows) { ";" } else { ":" };
    let mut parts: Vec<String> = prepend.iter().filter(|p| !p.is_empty()).cloned().collect();
    if !orig.is_empty() {
        parts.push(orig);
    }
    parts.join(sep)
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Create a new PTY session. Idempotently boots the WS server the first time.
pub fn spawn(opts: SpawnOptions) -> io::Result<SpawnResult> {
    let server = ensure_ws_server().map_err(io::Error::other)?;
    let cols = opts.cols.unwrap_or(100).max(1);
    let rows = opts.rows.unwrap_or(24).max(1);

    // On Windows, ConPTY (used by portable-pty) needs the current process to
    // be attached to a console before `openpty` will succeed — otherwise it
    // fails with HRESULT 0x80070006 (invalid handle) because the inherited
    // STD_INPUT_HANDLE is invalid. `AllocConsole` is a no-op if the process
    // already has one, and its return value is intentionally ignored.
    #[cfg(windows)]
    unsafe {
        // AllocConsole returns a BOOL we do not need to inspect: the API
        // fails harmlessly when the process is already attached to one.
        let _ = windows::Win32::System::Console::AllocConsole();
    }

    let pty = native_pty_system();
    let pair = pty
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(io::Error::other)?;

    let (shell, shell_args) = shell_command_for(opts.shell.as_deref());
    let cwd = cwd_for(opts.cwd.as_deref());

    let mut cmd = CommandBuilder::new(shell.clone());
    for a in shell_args {
        cmd.arg(a);
    }
    cmd.cwd(&cwd);

    let env_overrides = opts.env.clone().unwrap_or_default();
    let prepend = opts.prepend_path.unwrap_or_default();
    for (k, v) in &env_overrides {
        if k.eq_ignore_ascii_case("PATH") {
            continue;
        }
        cmd.env(k, v);
    }
    cmd.env(
        if cfg!(windows) { "Path" } else { "PATH" },
        resolved_path(&prepend),
    );

    let slave = pair.slave;
    let mut child = slave.spawn_command(cmd).map_err(io::Error::other)?;
    let pid = child.process_id().unwrap_or(0);

    let id = Uuid::new_v4().to_string();
    let (out_tx, _) = broadcast::channel::<Vec<u8>>(1024);
    let (ctrl_tx, ctrl_rx) = mpsc::channel::<ControlMsg>();

    let master: Box<dyn MasterPty + Send> = pair.master;
    let writer = master.take_writer().map_err(io::Error::other)?;
    let mut reader = master.try_clone_reader().map_err(io::Error::other)?;

    let alive = AtomicBool::new(true);
    let started_at_ms = unix_ms();

    let session = Arc::new(Session {
        id: id.clone(),
        pid,
        shell: shell.clone(),
        cwd: cwd.to_string_lossy().to_string(),
        started_at_ms,
        alive,
        outbound: out_tx.clone(),
        control_tx: ctrl_tx,
        stdin_tx: Mutex::new(Box::new(writer) as Box<dyn Write + Send>),
        wait_handle: Mutex::new(None),
    });

    // Reader thread: reads PTY master, publishes onto broadcast.
    {
        let session = Arc::clone(&session);
        std::thread::Builder::new()
            .name(format!("dshl-pty-r-{id}"))
            .spawn(move || {
                let mut buf = [0u8; 8192];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => {
                            let chunk = buf[..n].to_vec();
                            let _ = out_tx.send(chunk);
                        }
                        Err(ref e) if e.kind() == io::ErrorKind::Interrupted => continue,
                        Err(_) => break,
                    }
                }
                session.alive.store(false, Ordering::Release);
            })
            .map_err(io::Error::other)?;
    }

    // Control thread: owns the master handle (required for resize), drains
    // resize/kill control messages.
    {
        // The closure must NOT hold the Arc<Session>: the Session owns
        // control_tx, so a strong reference here keeps a Sender alive, which
        // keeps recv() from ever returning Err(Closed) — the thread (and the
        // PTY master handle it owns) would leak for every session whose child
        // exited naturally. Holding only the id lets the last Arc drop sever
        // the channel and end this thread.
        let session_id = id.clone();
        std::thread::Builder::new()
            .name(format!("dshl-pty-c-{id}"))
            .spawn(move || {
                // std mpsc recv returns Result<T, RecvError>; Ok means message arrived.
                let master = master;
                while let Ok(msg) = ctrl_rx.recv() {
                    match msg {
                        ControlMsg::Resize { cols, rows } => {
                            let _ = master.resize(PtySize {
                                rows,
                                cols,
                                pixel_width: 0,
                                pixel_height: 0,
                            });
                        }
                        ControlMsg::Kill => {
                            // `process_id()` can return None (spawn raced an
                            // early exit) and we stored 0. On Unix,
                            // `kill(0, sig)` does NOT mean "kill self" — it
                            // signals the CALLER'S WHOLE PROCESS GROUP, which
                            // in the plugin track is the host dsh/Node process
                            // and everything in its group. The Kill contract is
                            // "end this one PTY session"; a pid we cannot trust
                            // must degrade to a logged no-op (matching Windows,
                            // where OpenProcess(0) just fails), never escalate
                            // to a group-wide kill. Real "shut everything
                            // down" goes through control shutdown / tray exit.
                            if pid != 0 {
                                #[cfg(unix)]
                                {
                                    let _ = unsafe { libc::kill(pid as i32, libc::SIGTERM) };
                                }
                                #[cfg(windows)]
                                {
                                    windows_kill(pid);
                                }
                            } else {
                                crate::debug::emit(&format!(
                                    "pty kill: session {session_id} has no usable pid; refusing to signal"
                                ));
                            }
                            break;
                        }
                    }
                }
            })
            .map_err(io::Error::other)?;
    }

    // Child-wait thread: reaps the exit, then removes the session.
    let wait_handle = std::thread::Builder::new()
        .name(format!("dshl-pty-w-{id}"))
        .spawn({
            let session = Arc::clone(&session);
            move || {
                let _ = child.wait();
                session.alive.store(false, Ordering::Release);
                std::thread::sleep(std::time::Duration::from_millis(500));
                sessions().remove(&session.id);
            }
        })
        .map_err(io::Error::other)?;
    *session.wait_handle.lock().unwrap() = Some(wait_handle);

    sessions().insert(id.clone(), Arc::clone(&session));

    let ws_url = format!(
        "ws://127.0.0.1:{port}/_pty/{id}?token={t}",
        port = server.addr.port(),
        t = server.token
    );
    Ok(SpawnResult { id, pid, ws_url })
}

#[cfg(windows)]
fn windows_kill(pid: u32) {
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::System::Threading::{OpenProcess, PROCESS_TERMINATE, TerminateProcess};
    let Ok(h) = (unsafe { OpenProcess(PROCESS_TERMINATE, false, pid) }) else {
        return;
    };
    let _ = unsafe { TerminateProcess(HANDLE(h.0), 1) };
    let _ = unsafe { CloseHandle(HANDLE(h.0)) };
}

pub fn list() -> Vec<SessionInfo> {
    let guard = sessions();
    guard
        .values()
        .map(|s| SessionInfo {
            id: s.id.clone(),
            pid: s.pid,
            shell: s.shell.clone(),
            cwd: s.cwd.clone(),
            started_at_ms: s.started_at_ms,
            alive: s.alive.load(Ordering::Acquire),
        })
        .collect()
}

pub fn resize(id: &str, cols: u16, rows: u16) -> bool {
    let cols = cols.max(1);
    let rows = rows.max(1);
    let session = {
        let g = sessions();
        g.get(id).cloned()
    };
    let Some(s) = session else {
        return false;
    };
    s.control_tx.send(ControlMsg::Resize { cols, rows }).is_ok()
}

pub fn write(id: &str, bytes: &[u8]) -> bool {
    let session = {
        let g = sessions();
        g.get(id).cloned()
    };
    let Some(s) = session else {
        return false;
    };
    s.stdin_tx.lock().unwrap().write_all(bytes).is_ok()
}

pub fn kill(id: &str) -> bool {
    let session = {
        let g = sessions();
        g.get(id).cloned()
    };
    let Some(s) = session else {
        return false;
    };
    if s.control_tx.send(ControlMsg::Kill).is_ok() {
        sessions().remove(id);
        true
    } else {
        false
    }
}
