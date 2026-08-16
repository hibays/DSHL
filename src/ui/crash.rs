//! Crash recovery: when the supervised dsh exits unexpectedly (non-zero exit
//! code or killed by a signal), the launcher navigates back to the startup
//! page, shows a countdown banner (立即重启 / 取消) and auto-restarts dsh
//! after a few seconds unless the user intervenes.
//!
//! The window work stays on the main thread: the launch worker only sets
//! [`state::CRASH_NAVIGATE_PENDING`] and [`super::supervisor::run_loop`]
//! consumes it (navigating back to the launcher page, or restoring the tray
//! window showing it). The countdown timer lives on its own thread so it
//! keeps running even if the window is closed mid-countdown.
//!
//! Back-to-back crashes are serialized by a generation counter
//! ([`state::CRASH_GEN`]): a newer crash increments it, and any older
//! countdown thread sees the mismatch and exits without touching the newer
//! banner — so there is never more than one active countdown.

use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use super::launch::launch_flow;
use super::state;
use crate::progress;

/// Seconds before dsh is auto-restarted after an unexpected exit.
const CRASH_COUNTDOWN_SECS: u64 = 5;

/// Start crash recovery for a dsh that exited unexpectedly with `code`.
pub fn begin(code: i32) {
    let generation = state::CRASH_GEN.fetch_add(1, Ordering::SeqCst) + 1;
    crate::debug::emit(&format!(
        "dsh crashed (exit {code}); starting recovery (generation {generation})"
    ));

    state::CRASH_CANCELLED.store(false, Ordering::SeqCst);
    state::CRASH_RESTART_NOW.store(false, Ordering::SeqCst);

    // dsh is dead — drop its URL so every "dsh is running" affordance
    // (status badge, jump button, tray 打开 dsh) disappears until it is up
    // again.
    progress::clear_url();
    progress::set_crash(code, CRASH_COUNTDOWN_SECS as u8);
    progress::log(format!(
        "dsh 意外退出（exit {code}），{CRASH_COUNTDOWN_SECS} 秒后自动重启"
    ));
    // The countdown message lives in the banner (`state.crash`); keep the
    // prominent error text to just the fact, so the two don't read as a
    // duplicated paragraph.
    progress::set_error(format!("dsh 进程意外退出（exit {code}）。"));

    // The UI event loop (main thread) navigates back to the startup page —
    // or restores the tray window showing it — so the countdown is visible.
    state::CRASH_NAVIGATE_PENDING.store(true, Ordering::SeqCst);

    std::thread::spawn(move || countdown(generation, code));
}

/// The auto-restart countdown. Runs on its own thread; the startup page just
/// renders the remaining seconds from the shared progress state.
fn countdown(generation: u32, code: i32) {
    let deadline = Instant::now() + Duration::from_secs(CRASH_COUNTDOWN_SECS);

    loop {
        std::thread::sleep(Duration::from_millis(200));

        // A newer crash superseded this countdown: leave its banner alone and
        // just exit (the newer thread owns the flags now).
        if state::CRASH_GEN.load(Ordering::SeqCst) != generation {
            crate::debug::emit("crash recovery superseded by a newer crash");
            return;
        }
        // A launch flow is already running: the user clicked 重试, or an
        // earlier restart won the race. Our own restart would be an unexpected
        // re-launch after they moved on, so abort (the running flow re-supervises).
        if state::FLOW_RUNNING.load(Ordering::SeqCst) {
            crate::debug::emit("crash recovery: a launch flow is already running; aborting");
            finish(generation);
            return;
        }
        // The user is exiting the launcher (Ctrl+C, window close, or the 退出
        // button, which sets SHOULD_EXIT); don't restart dsh into the void.
        if state::SHUTDOWN_REQUESTED.load(Ordering::SeqCst)
            || state::SHOULD_EXIT.load(Ordering::SeqCst)
        {
            crate::debug::emit("crash recovery aborted: shutdown requested");
            finish(generation);
            return;
        }
        // The user cancelled the auto-restart (kept on the startup page, can
        // still restart manually via 重试).
        if state::CRASH_CANCELLED.load(Ordering::SeqCst) {
            if state::CRASH_GEN.load(Ordering::SeqCst) == generation {
                progress::set_error(format!(
                    "dsh 进程意外退出（exit {code}），已取消自动重启。可点击「重试」手动重启，或「退出」。"
                ));
            }
            crate::debug::emit("crash recovery cancelled by user");
            finish(generation);
            return;
        }
        // 立即重启 clicked, or the countdown expired with no action.
        if state::CRASH_RESTART_NOW.load(Ordering::SeqCst) || Instant::now() >= deadline {
            progress::log("dsh 意外退出，正在自动重启…");
            crate::debug::emit("crash recovery: auto-restarting dsh");
            finish(generation);
            launch_flow();
            return;
        }

        let left = deadline.saturating_duration_since(Instant::now()).as_secs() as u8;
        // Re-check before writing so a newer banner isn't clobbered by this
        // (superseded) thread's tick.
        if state::CRASH_GEN.load(Ordering::SeqCst) == generation {
            progress::set_crash_countdown(Some(left.max(1)));
        }
    }
}

/// Clear the crash flags and banner once the recovery decision is made. Only
/// acts if this generation is still current, so a superseded thread can never
/// touch a newer banner.
fn finish(generation: u32) {
    if state::CRASH_GEN.load(Ordering::SeqCst) == generation {
        state::CRASH_RESTART_NOW.store(false, Ordering::SeqCst);
        state::CRASH_CANCELLED.store(false, Ordering::SeqCst);
        progress::set_crash_countdown(None);
    }
}
