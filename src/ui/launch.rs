//! The launch flow: (re)load the config, clean up stale dsh, run the startup
//! pipeline, navigate the window to dsh and supervise the child.

use std::sync::atomic::Ordering;

use crate::config;
use crate::flow;
use crate::mirror::MirrorConfig;
use crate::progress;
use crate::runtime;

use super::state;
use super::window;

/// (Re)load the config and run the startup pipeline on a worker thread.
pub fn launch_flow() {
    // Only one flow at a time.
    if state::FLOW_RUNNING.swap(true, Ordering::SeqCst) {
        return;
    }

    // The window was closed while it was being created; don't launch dsh just
    // to tear it down again.
    if state::SHUTDOWN_REQUESTED.load(Ordering::SeqCst) {
        state::FLOW_RUNNING.store(false, Ordering::SeqCst);
        return;
    }

    let cli_path = state::CLI_CONFIG_PATH.lock().unwrap().clone();

    // Populate the startup page BEFORE the worker thread starts so the very
    // first `get_state` poll already returns the full step list and the
    // resolved config. Previously this ran inside the worker behind the (up to
    // 10s) stale-dsh cleanup, so the page first rendered as an empty "title +
    // log box" and the flow steps / config block appeared only later.
    let loaded = config::load(cli_path.as_deref());
    *state::CONFIG_PATH.lock().unwrap() = loaded.path.clone();

    let config_json = serde_json::to_string(&loaded.config).unwrap_or_default();
    let path_str = loaded
        .path
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    progress::set_config(config_json, path_str, loaded.parse_error.clone());

    // Same localized step list `flow::run` builds (id, title) — the canonical
    // startup steps, shown immediately.
    let steps: Vec<(&'static str, String)> = flow::STEPS
        .iter()
        .map(|&(id, key)| (id, t!(key).to_string()))
        .collect();
    progress::reset(&steps);
    progress::clear_error();

    if let Some(err) = &loaded.parse_error {
        progress::log(t!("ui.launch.config_error", err = err.to_string()));
    }

    std::thread::spawn(move || {
        // Ask any stale dsh left over from a previous failed attempt to exit
        // via Ctrl+C — the only correct way to close dsh on Windows
        // (AttachConsole + GenerateConsoleCtrlEvent). dsh saves its session
        // log during a Ctrl+C shutdown, and its own shutdown logic force-exits
        // at most 5 seconds after the signal, so a 10s wait covers a healthy
        // shutdown plus that self-timeout with margin; the signal is re-sent
        // every 5s in case the first one was lost.
        //
        // If it is STILL alive after that, the signal never reached it. Never
        // start a new dsh next to it: two processes appending to the same
        // session log produce overlapping seq numbers ("corrupt session log:
        // seq gap"), which is permanent and unrecoverable — the chat history
        // can no longer be loaded. Also do NOT force-kill it silently: a hard
        // kill is destructive (it can interrupt dsh mid-commit of its session
        // log), so it requires the user's explicit confirmation via the
        // dedicated button on the startup page.
        if let Some(child) = crate::DSH_CHILD.lock().unwrap().take() {
            if !runtime::block_on(child.graceful_kill(10_000)) {
                let pid = child.pid().unwrap_or(0);
                progress::set_stale_pid(Some(pid));
                progress::set_error(t!("ui.launch.stale_no_exit", pid = pid));
                state::FLOW_RUNNING.store(false, Ordering::SeqCst);
                return;
            }
            progress::log(t!("ui.launch.stale_exited"));
        }

        // The cleanup above can take up to 10s; if the user closed the window
        // during it, stop here instead of launching dsh into the void.
        if state::SHUTDOWN_REQUESTED.load(Ordering::SeqCst) {
            state::FLOW_RUNNING.store(false, Ordering::SeqCst);
            return;
        }

        let mirror = MirrorConfig::resolve(&loaded.config);

        // Optional single-instance guard: when enabled, refuse to start dsh
        // if any dsh is already running on this machine (started manually or
        // by another dshl). Two processes appending to the same session log
        // produce overlapping seq numbers ("corrupt session log: seq gap") —
        // permanent and unrecoverable, so a hard refusal is the safe choice.
        // The stale-dsh cleanup above has already ensured OUR previous child
        // is gone; this catches everyone else's.
        if loaded.config.dsh.single_instance
            && let Some(pid) = crate::platform::dsh_instance_running()
        {
            progress::set_error(t!("ui.launch.single_instance", pid = pid));
            state::FLOW_RUNNING.store(false, Ordering::SeqCst);
            return;
        }

        // Run the startup pipeline, but abort it as soon as a shutdown is
        // requested: `flow::run` itself is not interruptible (it installs,
        // spawns and waits on subprocesses), so race it against a 100ms poll
        // of SHUTDOWN_REQUESTED inside one `block_on(tokio::select!)`.
        //
        // When the flag wins, dropping the `flow::run` future cancels whatever
        // step it was in the middle of — but a dsh it already spawned is NOT
        // lost: the spawn registers an Arc of the AsyncChild in
        // `crate::DSH_CHILD` immediately, so the handle survives the drop and
        // the child can be reaped gracefully below instead of being orphaned.
        let outcome = runtime::block_on(async {
            tokio::select! {
                result = flow::run(&loaded.config, &mirror) => Some(result),
                _ = async {
                    loop {
                        if state::SHUTDOWN_REQUESTED.load(Ordering::SeqCst) {
                            return;
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    }
                } => None,
            }
        });

        let Some(result) = outcome else {
            // Shutdown interrupted the pipeline. Reap any dsh that was already
            // spawned (see above) — nobody else will: `exit::shutdown`'s
            // kill_dsh() either ran before the spawn or finds the slot empty.
            // The 10s timeout matches the stale-dsh cleanup; don't drag the
            // exit out with the 30s used by the supervised-phase kill_dsh().
            if let Some(child) = crate::DSH_CHILD.lock().unwrap().take() {
                let pid = child.pid().unwrap_or(0);
                crate::debug::emit(&format!(
                    "shutdown during launch; flow aborted, stopping dsh (pid {pid})"
                ));
                runtime::block_on(child.graceful_kill(10_000));
            } else {
                crate::debug::emit("shutdown during launch; flow aborted (dsh not yet spawned)");
            }
            state::FLOW_RUNNING.store(false, Ordering::SeqCst);
            return;
        };

        match result {
            Ok(launch) => {
                if state::SHUTDOWN_REQUESTED.load(Ordering::SeqCst) {
                    // The window was closed while dsh was starting. Don't
                    // touch webui (`navigate`/`get_hwnd`) after the main
                    // thread may already be cleaning up — just leave the
                    // child tracked so `kill_dsh()` reaps it.
                    crate::debug::emit("shutdown requested during launch; skipping navigate");
                } else {
                    // Route the window to dsh and hand off to supervisor mode.
                    window::navigate_when_connected(&launch.url);
                    state::LAUNCHED.store(true, Ordering::SeqCst);

                    // In WebView mode, track the window handle so the
                    // supervisor can detect when the window is destroyed.
                    if !state::IS_BROWSER.load(Ordering::SeqCst) {
                        window::capture_webview_hwnd();
                    }

                    // Supervise dsh: drain its output until it exits. A clean
                    // exit (code 0) shuts the launcher down as before — unless
                    // a restart was requested (control plane), in which case
                    // dsh's exit is the signal to relaunch in place. An
                    // unexpected exit starts crash recovery instead (back to
                    // the startup page + a 5s auto-restart countdown).
                    match runtime::block_on(flow::launch::supervise(launch.child)) {
                        flow::launch::DshExit::Clean => {
                            if state::RESTART_REQUESTED.swap(false, Ordering::SeqCst) {
                                // dsh exited because the control plane asked us
                                // to restart it. Reset the flow gate and run the
                                // pipeline again; the launcher page is already
                                // showing (CRASH_NAVIGATE_PENDING).
                                state::FLOW_RUNNING.store(false, Ordering::SeqCst);
                                launch_flow();
                                return;
                            }
                            state::SHOULD_EXIT.store(true, Ordering::SeqCst);
                        }
                        flow::launch::DshExit::Crash(code) => {
                            state::RESTART_REQUESTED.store(false, Ordering::SeqCst);
                            // The user already closed the window (e.g. a
                            // non-tray exit); don't resurrect, just exit.
                            if state::SHUTDOWN_REQUESTED.load(Ordering::SeqCst) {
                                state::SHOULD_EXIT.store(true, Ordering::SeqCst);
                            } else {
                                super::crash::begin(code);
                            }
                        }
                    }
                }
            }
            Err(_) => {
                // The error is already rendered via progress::set_error. The
                // tracked child (if any) stays so `kill_dsh()` can reap it.
            }
        }

        state::FLOW_RUNNING.store(false, Ordering::SeqCst);
    });
}

/// Stop the tracked dsh child via Ctrl+C/SIGTERM and wait for it to exit on
/// its own (up to 30s). Ctrl+C is the correct way to close dsh: it commits
/// its session log during shutdown and its own shutdown logic force-exits at
/// most 5s after the signal, so the generous wait covers it and no force kill
/// ever follows.
pub fn kill_dsh() {
    if let Some(child) = crate::DSH_CHILD.lock().unwrap().take() {
        runtime::block_on(child.graceful_kill(30_000));
    }
}

/// Restart dsh in place: go back to the launcher page, stop the current dsh
/// child and run the startup pipeline again.
///
/// The supervised dsh cannot be relaunched directly — the launch worker thread
/// holds `FLOW_RUNNING` while it supervises the live child, so `launch_flow`
/// would bail. Instead we set [`state::RESTART_REQUESTED`] and ask the current
/// child to exit (Ctrl+C); its supervisor observes the clean exit and relaunches
/// (see the `DshExit::Clean` branch of the launch worker). The window is already
/// back on the launcher page (`CRASH_NAVIGATE_PENDING`), so the restart progress
/// is visible while the old dsh tears down and the new one boots.
///
/// When there is no supervised child to stop (dsh already dead), the flow gate
/// is free and `launch_flow()` starts directly. Safe to call from any thread.
pub fn request_restart() {
    state::CRASH_NAVIGATE_PENDING.store(true, Ordering::SeqCst);
    crate::debug::emit("control: restart requested");
    match crate::DSH_CHILD.lock().unwrap().clone() {
        Some(child) => {
            state::RESTART_REQUESTED.store(true, Ordering::SeqCst);
            child.signal_stop();
        }
        None => launch_flow(),
    }
}
