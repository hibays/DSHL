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
            if !child.graceful_kill(10_000) {
                let pid = child.pid().unwrap_or(0);
                state::STALE_PID.store(pid, Ordering::SeqCst);
                progress::set_stale_pid(Some(pid));
                progress::set_error(format!(
                    "残留的 dsh 进程 (pid {pid}) 未响应 Ctrl+C 退出请求。为避免两个 dsh 同时写入同一会话日志（聊天记录将永久损坏），本次启动已取消。请点击「强制结束残留进程」结束它，或手动结束后再点重试。"
                ));
                state::FLOW_RUNNING.store(false, Ordering::SeqCst);
                return;
            }
            progress::log("残留的 dsh 进程已退出，继续启动");
        }

        // The cleanup above can take up to 10s; if the user closed the window
        // during it, stop here instead of launching dsh into the void.
        if state::SHUTDOWN_REQUESTED.load(Ordering::SeqCst) {
            state::FLOW_RUNNING.store(false, Ordering::SeqCst);
            return;
        }

        let loaded = config::load(cli_path.as_deref());
        *state::CONFIG_PATH.lock().unwrap() = loaded.path.clone();

        let config_json = serde_json::to_string(&loaded.config).unwrap_or_default();
        let path_str = loaded
            .path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        progress::set_config(config_json, path_str, loaded.parse_error.clone());
        if let Some(err) = &loaded.parse_error {
            progress::log(format!("dshl.toml 解析错误：{err}"));
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
            progress::set_error(format!(
                "single-instance 已启用：检测到另一个 dsh 实例 (pid {pid}) 正在运行。为避免两个 dsh 同时写入同一会话日志（聊天记录将永久损坏），本次启动已取消。请先关闭现有的 dsh，或把 dshl.toml 的 single-instance 设为 false。"
            ));
            state::FLOW_RUNNING.store(false, Ordering::SeqCst);
            return;
        }

        match runtime::block_on(flow::run(&loaded.config, &mirror)) {
            Ok(launch) => {
                if state::SHUTDOWN_REQUESTED.load(Ordering::SeqCst) {
                    // The window was closed while dsh was starting. Don't
                    // touch webui (`navigate`/`get_hwnd`) after the main
                    // thread may already be cleaning up — just leave the
                    // child tracked so `kill_dsh()` reaps it.
                    crate::debug::emit("shutdown requested during launch; skipping navigate");
                } else {
                    // Route the window to dsh and hand off to supervisor mode.
                    window::navigate(&launch.url);
                    state::LAUNCHED.store(true, Ordering::SeqCst);

                    // In WebView mode, track the window handle so the
                    // supervisor can detect when the window is destroyed.
                    if !state::IS_BROWSER.load(Ordering::SeqCst) {
                        window::capture_webview_hwnd();
                    }

                    // Supervise dsh: drain its output until it exits. When it
                    // exits (or is killed), ask the event loop to shut down.
                    runtime::block_on(flow::launch::supervise(launch.child));
                }
                state::SHOULD_EXIT.store(true, Ordering::SeqCst);
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
        child.graceful_kill(30_000);
    }
}
