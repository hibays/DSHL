//! Functions bound to the startup page (webui `bind`).
//!
//! These are the frontend's only entry points into the launcher; they read
//! shared state ([`super::state`]), drive the launch flow
//! ([`super::launch`]) and surface progress via [`crate::progress`].

use webui::webui;

use super::launch::launch_flow;
use super::state;
use crate::config;
use crate::progress;

fn get_state(e: webui::Event) {
    e.return_string(&progress::to_json());
}

fn exit_app(_e: webui::Event) {
    // Funnel into the composed shutdown. The flags are set here (thread-safe);
    // the run_loop observes them on the main thread and drives the full
    // webui-canonical teardown (`webui::exit()` → `webui::clean()`, see
    // `super::exit`) — the same pattern webui's own examples use, and the
    // same path as tray-quit / Ctrl+C / window close.
    super::exit::request_shutdown();
}

fn retry(_e: webui::Event) {
    let stale = progress::stale_pid();
    if stale != 0 && crate::platform::process_alive(stale) {
        progress::set_error(t!("ui.bindings.stale_running", pid = stale));
        return;
    }
    if stale != 0 {
        progress::set_stale_pid(None);
    }
    launch_flow();
}

/// Force-kill the stale dsh — only runs after the user clicks the dedicated
/// button on the startup page (explicit confirmation). The kill itself is
/// async so the webui thread is not blocked; on success the launch retries.
fn force_kill_stale(_e: webui::Event) {
    let pid = progress::stale_pid();
    if pid == 0 {
        return;
    }
    std::thread::spawn(move || {
        crate::debug::emit(&format!(
            "user confirmed force-kill of stale dsh (pid {pid})"
        ));
        crate::platform::kill_tree(pid);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if !crate::platform::process_alive(pid) || std::time::Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        if crate::platform::process_alive(pid) {
            progress::set_error(t!("ui.bindings.kill_failed", pid = pid));
            return;
        }
        progress::set_stale_pid(None);
        progress::log(t!("ui.bindings.killed", pid = pid));
        launch_flow();
    });
}

fn open_config(_e: webui::Event) {
    let path = state::CONFIG_PATH
        .lock()
        .unwrap()
        .clone()
        .unwrap_or_else(config::default_config_path);
    if !path.exists() {
        let _ = config::write_template(&path);
    }
    let _ = crate::platform::open_path(&path);
}

/// 立即重启 from the crash-recovery banner: the countdown thread restarts dsh
/// right away (it is the single owner of the restart, so there is no race).
fn restart_now(_e: webui::Event) {
    state::CRASH_RESTART_NOW.store(true, std::sync::atomic::Ordering::SeqCst);
}

/// 取消 from the crash-recovery banner: stop the auto-restart (the user can
/// still restart manually via 重试).
fn cancel_restart(_e: webui::Event) {
    state::CRASH_CANCELLED.store(true, std::sync::atomic::Ordering::SeqCst);
}

/// Register all frontend bindings on a fresh window.
pub(crate) fn register(window: &webui::Window) {
    window.bind("get_state", get_state);
    window.bind("exit_app", exit_app);
    window.bind("retry", retry);
    window.bind("force_kill_stale", force_kill_stale);
    window.bind("open_config", open_config);
    window.bind("restart_now", restart_now);
    window.bind("cancel_restart", cancel_restart);
}
