//! Browser-mode lifecycle: pid tracking, close detection, capture budget.
//!
//! Owns EVERY piece of browser-lifecycle state that used to be scattered
//! across `state::*` atomics (`BROWSER_PID`, `BROWSER_CHECKED`,
//! `BROWSER_WAS_SHOWN`, `BROWSER_CAPTURE_ATTEMPTS`) plus the capture-in-flight
//! guard from `window.rs`. The 50 ms supervisor loop asks one question per
//! tick - [`poll_close`] - and executes the returned decision; everything
//! else (latches, throttles, retry budgets, one-shot logs) is private here.
//!
//! Detection semantics preserved verbatim from the pre-refactor supervisor:
//! * pid captured: process-alive check decides;
//! * pid never captured: the `was_shown` latch separates "never connected"
//!   (show instant) from a reliable close (was_shown, then is_shown false);
//! * capture retries are throttled to one attempt / 2 s with an 8-attempt
//!   budget per tray cycle (reset by [`note_window_recreated`]).

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use crate::platform;

/// Capture retries: one attempt every [`CAPTURE_RETRY_INTERVAL`], so the
/// budget spans ~80 s — an external browser can take several seconds to
/// cold-start (AV scan, profile lock, loaded machine), and the capture must
/// still be pending when its window finally appears.
const CAPTURE_ATTEMPTS_LIMIT: u32 = 40;
const CAPTURE_RETRY_INTERVAL: Duration = Duration::from_secs(2);

static PID: AtomicUsize = AtomicUsize::new(0);
static CHECKED: AtomicBool = AtomicBool::new(false);
static WAS_SHOWN: AtomicBool = AtomicBool::new(false);
/// True once the CURRENT window was navigated away from the launcher page to
/// the dsh URL. After that navigation the browser's webui socket is gone for
/// good (the dsh page does not speak the webui protocol), so `is_shown`
/// reads false forever and the was_shown latch would classify the LIVE
/// browser as closed. Close detection for a navigated window relies on the
/// pid alone; the latch only guards the pre-navigation window.
static NAVIGATED_TO_DSH: AtomicBool = AtomicBool::new(false);
static CAPTURE_ATTEMPTS: AtomicU32 = AtomicU32::new(0);
static LAST_CAPTURE: Mutex<Option<Instant>> = Mutex::new(None);

/// Which lifecycle phase the supervisor loop is in - log wording differs.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Phase {
    /// dsh not handed off yet: a browser close means quit outright.
    Startup,
    /// dsh running: a browser close hands over to tray or quits.
    Supervising,
}

/// What the supervisor must DO about a detected browser close this tick.
pub(crate) enum CloseAction {
    /// Nothing detected (or still inside the never-connected grace window).
    None,
    /// Hand over to the tray; dsh keeps running.
    ToTray,
    /// Shut the launcher down and reap everything.
    Quit,
}

/// Per-tick outcome: an optional close action plus whether the caller should
/// fire [`window::capture_browser_pid`] on this tick (throttle decided here).
pub(crate) struct Tick {
    pub action: CloseAction,
    pub retry_capture: bool,
}

pub(crate) fn pid() -> u32 {
    PID.load(Ordering::SeqCst) as u32
}

/// Current browser pid (0 = none known). Read by shutdown paths that must
/// close/reap the external browser or persist its window geometry.
pub(crate) fn pid_for_teardown() -> u32 {
    pid()
}

/// Reset every piece of browser-lifecycle state for a fresh kernel boot
/// Reset every piece of browser-lifecycle state for a fresh kernel boot
/// (called from `state::reset_runtime_state`).
pub(crate) fn reset_runtime_state() {
    PID.store(0, Ordering::SeqCst);
    CHECKED.store(false, Ordering::SeqCst);
    WAS_SHOWN.store(false, Ordering::SeqCst);
    CAPTURE_ATTEMPTS.store(0, Ordering::SeqCst);
    if let Ok(mut g) = LAST_CAPTURE.lock() {
        *g = None;
    }
}

/// A freshly (re-)created browser-mode window: re-arm the shown-latch and
/// hand the pid capture a fresh retry budget for the new cycle. Called from
/// `show_window` on success and from `restore_from_tray`'s rebuild path.
pub(crate) fn note_window_recreated() {
    CHECKED.store(false, Ordering::SeqCst);
    WAS_SHOWN.store(false, Ordering::SeqCst);
    NAVIGATED_TO_DSH.store(false, Ordering::SeqCst);
    CAPTURE_ATTEMPTS.store(0, Ordering::SeqCst);
}

/// Record a captured browser pid (from `window::capture_browser_pid`).
pub(crate) fn set_pid(pid: usize) {
    PID.store(pid, Ordering::SeqCst);
}

/// Close-to-tray transition: forget the pid and clear every latch so the
/// next window starts detection from scratch. Clearing WAS_SHOWN here is
/// what stops a detected close from re-firing on every subsequent tick
/// after the window has already been freed.
pub(crate) fn note_closed_to_tray() {
    clear_pid();
    WAS_SHOWN.store(false, Ordering::SeqCst);
    NAVIGATED_TO_DSH.store(false, Ordering::SeqCst);
}

pub(crate) fn clear_pid() {
    PID.store(0, Ordering::SeqCst);
    CHECKED.store(false, Ordering::SeqCst);
}

/// The window was navigated to the dsh URL (launch flow or tray restore).
/// See [`NAVIGATED_TO_DSH`].
pub(crate) fn note_navigated_to_dsh() {
    NAVIGATED_TO_DSH.store(true, Ordering::SeqCst);
}

/// One supervisor tick for browser mode. `phase` selects log wording and -
/// during [`Phase::Startup`] - forces a detected close to be a full quit
/// (close-to-tray has nothing to hand over to before dsh is launched).
pub(crate) fn poll_close<F>(phase: Phase, window_id: usize, mut is_shown: F) -> Tick
where
    F: FnMut(usize) -> bool,
{
    let pid = pid();
    if pid != 0 {
        if !CHECKED.swap(true, Ordering::SeqCst) {
            crate::debug::emit(&format!(
                "browser supervisor active (pid {pid}, alive={})",
                platform::process_alive(pid)
            ));
        }
        if !platform::process_alive(pid) {
            return Tick {
                action: decide_on_close(phase),
                retry_capture: false,
            };
        }
        return Tick {
            action: CloseAction::None,
            retry_capture: false,
        };
    }

    // pid never captured: fall back to webui's is_shown with the was_shown
    // latch separating "never connected" from a reliable close. The latch is
    // only meaningful BEFORE the window navigated to dsh — afterwards the
    // webui socket is gone by design and `is_shown` reads false forever
    // while the browser is alive (see NAVIGATED_TO_DSH).
    let shown = is_shown(window_id);
    if shown {
        WAS_SHOWN.store(true, Ordering::SeqCst);
    } else if WAS_SHOWN.load(Ordering::SeqCst) && !NAVIGATED_TO_DSH.load(Ordering::SeqCst) {
        return Tick {
            action: decide_on_close(phase),
            retry_capture: false,
        };
    }

    // Still here: keep re-locating the pid (throttled, bounded budget) so
    // pid-based detection can take over if it ever lands.
    let retry = if CAPTURE_ATTEMPTS.load(Ordering::SeqCst) < CAPTURE_ATTEMPTS_LIMIT && capture_due()
    {
        CAPTURE_ATTEMPTS.fetch_add(1, Ordering::SeqCst);
        if CAPTURE_ATTEMPTS.load(Ordering::SeqCst) >= CAPTURE_ATTEMPTS_LIMIT {
            crate::debug::emit(give_up_msg(phase));
            false
        } else {
            crate::debug::emit(retry_msg(phase));
            true
        }
    } else {
        false
    };
    Tick {
        action: CloseAction::None,
        retry_capture: retry,
    }
}

fn decide_on_close(phase: Phase) -> CloseAction {
    match phase {
        // Startup: nothing to hand over to - quit outright.
        Phase::Startup => CloseAction::Quit,
        // Supervising: honour close-to-tray.
        Phase::Supervising => {
            if crate::ui::state::CLOSE_TO_TRAY.load(Ordering::SeqCst) {
                CloseAction::ToTray
            } else {
                CloseAction::Quit
            }
        }
    }
}

fn retry_msg(phase: Phase) -> &'static str {
    match phase {
        Phase::Startup => "run_loop: startup browser pid unknown; retrying capture",
        Phase::Supervising => "run_loop: browser pid unknown; retrying capture",
    }
}

fn give_up_msg(_phase: Phase) -> &'static str {
    "run_loop: giving up browser pid capture"
}

fn capture_due() -> bool {
    // Arms the throttle timestamp when it answers `true`: consecutive 50 ms
    // supervisor ticks must not burn the attempt budget in under a second.
    let mut last = LAST_CAPTURE.lock().unwrap_or_else(|p| p.into_inner());
    match *last {
        Some(t) if t.elapsed() < CAPTURE_RETRY_INTERVAL => false,
        _ => {
            *last = Some(Instant::now());
            true
        }
    }
}
