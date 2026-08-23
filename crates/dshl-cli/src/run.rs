//! Shared kernel entry points — the heart of the dual-track design.
//!
//! `run_cli` (Track A binary) and `run_with_options` (Track B addon) both
//! drive the same `dshl_core::ui` pipeline (`setup` → `launch_flow` →
//! `run_loop`), differing only in how control is returned:
//!
//! * `run_cli` blocks the calling thread until the supervisor loop exits and
//!   returns CLI-only branches (`--help` etc.) as `RunOutcome` so the binary
//!   shell can translate them to exit codes without ever calling
//!   `std::process::exit`.
//! * `run_with_options` skips CLI parsing entirely, drives the pipeline on
//!   a managed background thread (so the Node event loop stays alive) and
//!   returns a `RunHandle` whose methods (in `super::control`) drive the
//!   running kernel across threads.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex, OnceLock};

use dshl_core::config;
use dshl_core::error::{Error, Result};
use dshl_core::ui;

use crate::handle::{RunHandle, new as new_handle};
use crate::options::{RunOptions, RunOutcome};
use crate::signal::{install_ctrlc, install_ctrlc_boxed, reset_runtime_state};

/// Idempotent, re-entrant locale + DPI init. Safe to call from the bin, from
/// the addon, and from within `run_with_options` — the OnceLock ensures the
/// rust_i18n macro backend + sys-locale probe run at most once per process.
static CORE_INIT_DONE: OnceLock<()> = OnceLock::new();

/// Serialises concurrent kernel boots (a `run_cli` on one thread and a
/// `run_with_options` on another never stack two `ui::setup` calls
/// concurrently — webui is single-globally).
static CLI_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

/// Try to take the kernel-boot lock without blocking.
///
/// A kernel boot holds [`CLI_LOCK`] for exactly the window in which a second,
/// concurrent `ui::setup` would corrupt webui's single-global state (napi
/// `windowShow()` right after `launch()` is the realistic caller). Callers
/// take this same lock instead of keeping a second, weaker flag in sync.
///
/// A poisoned lock is tolerated and recovered from (`into_inner`) — the
/// established pattern here; a boot that panicked mid-setup must not disable
/// window handling forever.
pub(crate) fn try_kernel_lock() -> Option<std::sync::MutexGuard<'static, ()>> {
    match CLI_LOCK.try_lock() {
        Ok(guard) => Some(guard),
        Err(std::sync::TryLockError::Poisoned(err)) => Some(err.into_inner()),
        Err(std::sync::TryLockError::WouldBlock) => None,
    }
}

pub fn ensure_core_init() {
    CORE_INIT_DONE.get_or_init(|| {
        dshl_core::platform::make_dpi_aware();
        dshl_core::i18n::init();
    });
}

/// Shared entry point for the Track A binary. Mirrors the historical `main()`
/// behaviour but never calls `std::process::exit` — CLI-only branches
/// (--help, --version, bad args, already running) are returned as
/// [`RunOutcome`] so the binary shell can translate them to exit codes.
///
/// Blocks the calling thread until the event loop exits (supervisor loop
/// returned + composed shutdown finished — same semantics as the old `main`).
pub fn run_cli() -> Result<std::result::Result<(), RunOutcome>> {
    ensure_core_init();
    // Serialise concurrent `run_cli` attempts (shouldn't happen in a binary,
    // but protects the plugin track's shared static state).
    let _guard = CLI_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    reset_runtime_state();

    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let mut cli = RunOptions {
        config: None,
        debug: false,
        enable_single_instance: true,
        enable_control_pipe: true,
        install_signal_handler: true,
    };
    // Manual flag walk — keep the same CLI contract as the historical main.
    let mut it = args.drain(..);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "-h" | "--help" => return Ok(Err(RunOutcome::HelpPrinted)),
            "-V" | "--version" => return Ok(Err(RunOutcome::VersionPrinted)),
            "-c" | "--config" => match it.next() {
                Some(path) => cli.config = Some(PathBuf::from(path)),
                None => {
                    return Ok(Err(RunOutcome::ArgsError(
                        "--config requires a value".into(),
                    )));
                }
            },
            "-d" | "--debug" | "-v" | "--verbose" => cli.debug = true,
            other => {
                return Ok(Err(RunOutcome::ArgsError(format!(
                    "unexpected argument '{other}'\n\n{}",
                    crate::USAGE
                ))));
            }
        }
    }

    apply_run_options(&cli);

    if cli.enable_single_instance
        && config::load(cli.config.as_deref())
            .config
            .ui
            .single_instance
    {
        if let Some(lock) = dshl_core::platform::single_instance::acquire() {
            // Hold the single-instance lock handle alive for the whole run.
            std::mem::forget(lock);
        } else {
            dshl_core::platform::single_instance::notify_activate();
            // Give the running instance a moment to bring its window forward.
            std::thread::sleep(std::time::Duration::from_millis(500));
            return Ok(Err(RunOutcome::AlreadyRunning));
        }
    }

    if cli.install_signal_handler {
        install_ctrlc();
    }

    // Launcher control-plane override: the addon track typically keeps this
    // OFF (it routes commands through napi instead), but legacy behaviour for
    // the CLI bin and tests is ON.
    if cli.enable_control_pipe
        && let Err(e) = dshl_core::control::start()
    {
        dshl_core::debug::emit(&format!("control server failed to start: {e}"));
    }

    ui::setup(cli.config);
    // Boot finished — release CLI_LOCK so control-plane shims (window_show)
    // can take it; launch_flow/run_loop perform no further setup.
    drop(_guard);
    ui::launch_flow();
    ui::run_loop();
    Ok(Ok(()))
}

/// Launch the full kernel on a dedicated background thread (so the Node event
/// loop can continue spinning alongside the webui event loop). Returns a
/// [`RunHandle`] whose methods control the running window / tray / dsh.
///
/// Idempotent: if the kernel is already running in this process, the
/// returned handle points at the existing instance (no duplicate windows).
pub fn run_with_options(opts: RunOptions) -> Result<RunHandle> {
    ensure_core_init();
    static HANDLE: LazyLock<Mutex<Option<RunHandle>>> = LazyLock::new(|| Mutex::new(None));
    let mut guard = HANDLE.lock().unwrap_or_else(|p| p.into_inner());
    if let Some(h) = guard.clone()
        && h.is_running()
    {
        return Ok(h);
    }

    // Hold the single-instance lock in a process-lifetime slot instead of
    // `mem::forget`-ing it: the plugin track boots and stops kernels in the
    // SAME process, and a forgotten file lock would block every relaunch
    // until process exit. Replacing the slot releases the previous kernel's
    // lock only after the next boot acquired a fresh one, so cross-process
    // exclusion is never widened.
    static INSTANCE_LOCK: std::sync::Mutex<Option<std::fs::File>> =
        const { std::sync::Mutex::new(None) };
    apply_run_options(&opts);

    if opts.enable_single_instance
        && config::load(opts.config.as_deref())
            .config
            .ui
            .single_instance
    {
        let mut slot = INSTANCE_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(lock) = dshl_core::platform::single_instance::acquire() {
            *slot = Some(lock);
        } else {
            drop(slot);
            dshl_core::platform::single_instance::notify_activate();
            return Err(Error("another dshl instance is already running".into()));
        }
    }

    let ctrlc_guard: Option<Box<dyn std::any::Any + Send + Sync>> = if opts.install_signal_handler {
        install_ctrlc_boxed()
    } else {
        None
    };

    if opts.enable_control_pipe
        && let Err(e) = dshl_core::control::start()
    {
        dshl_core::debug::emit(&format!("control server failed to start: {e}"));
    }

    let started = Arc::new(AtomicBool::new(false));
    let config_clone = opts.config.clone();
    let thread = Mutex::new(None);
    let handle = new_handle(started, thread, ctrlc_guard);
    let inner_t = handle.inner.clone();
    let jh = std::thread::Builder::new()
        .name("dshl-kernel".into())
        .spawn(move || {
            // Panic safety: `ui::setup/launch_flow/run_loop` run foreign code
            // (webui, dsh, user config). If any of them unwinds, the stores
            // below the pipeline are skipped and the handle would claim
            // "running" forever while no kernel exists. The drop guard resets
            // both during unwind; on normal completion it is defused first.
            struct KernelCleanup {
                started: Arc<AtomicBool>,
                done: bool,
            }
            impl Drop for KernelCleanup {
                fn drop(&mut self) {
                    if self.done {
                        return;
                    }
                    self.started.store(false, Ordering::SeqCst);
                    let mut g = HANDLE.lock().unwrap_or_else(|p| p.into_inner());
                    *g = None;
                    dshl_core::debug::emit("kernel thread ended abnormally; state reset");
                }
            }

            // Serialise kernel boots against the CLI lock too so a `run_cli`
            // on another thread and a `run_with_options` here never stack two
            // `ui::setup` calls concurrently (webui is single-globally).
            let cli_guard = CLI_LOCK.lock().unwrap_or_else(|p| p.into_inner());
            reset_runtime_state();
            inner_t.started.store(true, Ordering::SeqCst);
            let mut cleanup = KernelCleanup {
                started: Arc::clone(&inner_t.started),
                done: false,
            };
            ui::setup(config_clone);
            // Boot finished — release CLI_LOCK before the long-running
            // supervise loop (see run_cli for why).
            drop(cli_guard);
            ui::launch_flow();
            ui::run_loop();
            inner_t.started.store(false, Ordering::SeqCst);
            // Clear the cached handle so a later `run_with_options` boots a
            // fresh kernel instead of handing back a stale stopped one.
            let mut g = HANDLE.lock().unwrap_or_else(|p| p.into_inner());
            *g = None;
            drop(g);
            cleanup.done = true;
            drop(cleanup);
        })
        .map_err(|e| Error(format!("failed to spawn kernel thread: {e}")))?;
    *handle
        .inner
        .thread
        .lock()
        .unwrap_or_else(|p| p.into_inner()) = Some(jh);

    *guard = Some(handle.clone());
    Ok(handle)
}

/// Apply debug-logging options shared by both tracks: env (`DSHL_LOG`)
/// OR'ed with the explicit `--debug` flag.
fn apply_run_options(opts: &RunOptions) {
    let env_debug = std::env::var("DSHL_LOG")
        .map(|v| !v.trim().is_empty())
        .unwrap_or(false);
    dshl_core::debug::set_enabled(opts.debug || env_debug);
    if dshl_core::debug::enabled() {
        dshl_core::debug::emit("debug runtime logging enabled");
    }
}
