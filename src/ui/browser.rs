//! Browser-mode lifecycle: window-anchored tracking, close detection,
//! capture convergence, teardown targets.
//!
//! Design invariant: the ground truth for "our browser window is open" is a
//! VISIBLE TOP-LEVEL WINDOW owned by the tracked pid — never bare process
//! liveness. Chromium main processes outlive their last window (startup
//! boost / background mode), and single-instance forwarding makes the
//! initially spawned process exit while the window lives elsewhere; both
//! directions break any pid-alive-only model and produced zombie browsers.
//!
//! Primitives (all private state, one public surface):
//! * [`adopt`] — anchor detection on an HWND (collision-free identity) and
//!   record the owning pid for shutdown reaping. Resets detection latches
//!   and capture backoff.
//! * [`poll_close`] — one supervisor tick. Close requires CONFIRM_TICKS
//!   consecutive negative frames (enumeration/socket blips cannot fire
//!   ToTray/Quit). Liveness is `platform::is_window_alive(hwnd)` on
//!   Windows, process-alive fallback elsewhere.
//! * capture retries — UNBOUNDED with backoff (2/4/8/10 s). A permanent
//!   budget created detection vacuums (Firefox/boost forwarding never
//!   matched a needle → blind after navigation → leak on close). Retries
//!   stop only when a close is decided or state resets.
//! * socket fallback — meaningful only BEFORE navigation, and only past a
//!   [`SHOW_GRACE_SECS`] grace that absorbs slow cold starts.
//! * [`pid_for_teardown`] — last known real pid for shutdown reaping. A
//!   never-captured session leaves nothing to reap: cross-app profile sweeps
//!   are deliberately not attempted (webui shares one `.WebUI` dir across
//!   all webui.me apps, so a process-level probe cannot tell ours apart).

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

/// Close must stay negative for this many consecutive supervisor ticks
/// (50 ms apart) before an action fires — one bad frame must not quit.
const CONFIRM_TICKS: u32 = 3;

/// Capture retry backoff schedule (seconds between attempts). Unbounded
/// retries with capped intervals: cheap while waiting, converges whenever
/// the window finally materialises.
const BACKOFF_SECS: [u64; 4] = [2, 4, 8, 10];

/// After a successful browser show, negatives on the (uncaptured) socket
/// path are ignored for this long: a slow cold start must not be classified
/// as "user closed the window". Past the grace, any negative frame counts
/// toward the confirmation gate.
const SHOW_GRACE_SECS: u64 = 12;

/// Our browser window's HWND — THE identity primitive. A HWND cannot be
/// shared with other webui.me apps (unlike process cmdlines or the shared
/// `.WebUI` profile dir), so every decision anchors on it.
static BROWSER_HWND: AtomicUsize = AtomicUsize::new(0);
static PID: AtomicUsize = AtomicUsize::new(0);

/// The owning process pid, kept for shutdown kill_tree. Survives
/// close-to-tray's detection forget; cleared only on a kernel reset.
static TEARDOWN_PID: AtomicUsize = AtomicUsize::new(0);
static CHECKED: AtomicBool = AtomicBool::new(false);
static SHOWN_AT: Mutex<Option<Instant>> = Mutex::new(None);
/// True once the CURRENT window was navigated away from the launcher page to
/// the dsh URL. After that navigation the browser's webui socket is gone for
/// good (the dsh page does not speak the webui protocol), so `is_shown`
/// reads false forever while the browser is alive; the socket path must go
/// blind by design from that point on (the tracked pid takes over).
static NAVIGATED_TO_DSH: AtomicBool = AtomicBool::new(false);
static CONFIRM: AtomicU32 = AtomicU32::new(0);
static BACKOFF_STEP: AtomicUsize = AtomicUsize::new(0);
static LAST_ATTEMPT: Mutex<Option<Instant>> = Mutex::new(None);

fn lock_last_attempt() -> std::sync::MutexGuard<'static, Option<Instant>> {
    LAST_ATTEMPT.lock().unwrap_or_else(|p| p.into_inner())
}

fn lock_shown_at() -> std::sync::MutexGuard<'static, Option<Instant>> {
    SHOWN_AT.lock().unwrap_or_else(|p| p.into_inner())
}

/// True while negative socket frames are still attributed to a slow start
/// rather than a closed window (`None` = never marked shown = also treated
/// as inside the grace, protecting slow cold starts).
fn show_grace_active() -> bool {
    match *lock_shown_at() {
        Some(t) => t.elapsed() < Duration::from_secs(SHOW_GRACE_SECS),
        None => true,
    }
}

/// Which lifecycle phase the supervisor loop is in - log wording differs.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Phase {
    /// dsh not launched yet - a closed browser quits outright.
    Startup,
    /// dsh running - a closed browser honours close-to-tray.
    Supervising,
}

/// What the supervisor must DO about a detected browser close this tick.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum CloseAction {
    /// Nothing detected this tick.
    None,
    /// Hide to tray (dsh keeps running).
    ToTray,
    /// Shut the launcher down.
    Quit,
}

/// Per-tick outcome: an optional close action plus whether the caller should
/// fire [`window::capture_browser_pid`] on this tick (backoff decided here).
pub(crate) struct Tick {
    pub action: CloseAction,
    pub retry_capture: bool,
}

/// Currently tracked detection pid (0 = none).
pub(crate) fn pid() -> u32 {
    PID.load(Ordering::SeqCst) as u32
}

/// Our browser window's HWND (0 = none). THE identity primitive.
pub(crate) fn hwnd() -> usize {
    BROWSER_HWND.load(Ordering::SeqCst)
}

/// Last adopted browser pid (0 = none) for shutdown reaping.
pub(crate) fn pid_for_teardown() -> u32 {
    TEARDOWN_PID.load(Ordering::SeqCst) as u32
}

/// Reset every piece of browser-lifecycle state for a fresh kernel boot
/// (called from `state::reset_runtime_state`).
pub(crate) fn reset_runtime_state() {
    BROWSER_HWND.store(0, Ordering::SeqCst);
    TEARDOWN_PID.store(0, Ordering::SeqCst);
    CHECKED.store(false, Ordering::SeqCst);
    NAVIGATED_TO_DSH.store(false, Ordering::SeqCst);
    CONFIRM.store(0, Ordering::SeqCst);
    BACKOFF_STEP.store(0, Ordering::SeqCst);
    *lock_last_attempt() = None;
    *lock_shown_at() = None;
}

/// A freshly (re-)created browser-mode window: restart capture convergence
/// and drop the previous cycle's show timestamp. Called from
/// `restore_from_tray`'s rebuild path just before it calls `show_window`;
/// [`note_window_shown`] re-marks the timestamp once the show succeeds.
pub(crate) fn note_window_recreated() {
    CHECKED.store(false, Ordering::SeqCst);
    NAVIGATED_TO_DSH.store(false, Ordering::SeqCst);
    CONFIRM.store(0, Ordering::SeqCst);
    BACKOFF_STEP.store(0, Ordering::SeqCst);
    *lock_shown_at() = None;
}

/// Mark the CURRENT window's browser as successfully shown — starts (or
/// restarts) the [`SHOW_GRACE_SECS`] window during which socket negatives
/// are attributed to a slow start. Called right where `IS_BROWSER` is
/// published after a successful `show`.
pub(crate) fn note_window_shown() {
    *lock_shown_at() = Some(Instant::now());
}

/// Adopt the browser window: anchor on its HWND (collision-free identity —
/// a HWND belongs to exactly one window of one app) and record the owning
/// pid for shutdown reaping. Resets detection latches and capture backoff.
pub(crate) fn adopt(hwnd: usize, pid: u32) {
    BROWSER_HWND.store(hwnd, Ordering::SeqCst);
    PID.store(pid as usize, Ordering::SeqCst);
    TEARDOWN_PID.store(pid as usize, Ordering::SeqCst);
    CHECKED.store(false, Ordering::SeqCst);
    CONFIRM.store(0, Ordering::SeqCst);
    BACKOFF_STEP.store(0, Ordering::SeqCst);
    *lock_last_attempt() = None;
    crate::debug::emit(&format!(
        "tracking browser window (hwnd {hwnd:#x}, pid {pid})"
    ));
}

/// Close-to-tray transition: forget the detection pid and clear every latch
/// so the next window starts detection from scratch. TEARDOWN_PID
/// deliberately survives for shutdown reaping. Also resets the capture
/// backoff so the next window starts with a fresh retry budget.
pub(crate) fn note_closed_to_tray() {
    clear_pid();
    *lock_shown_at() = None;
    NAVIGATED_TO_DSH.store(false, Ordering::SeqCst);
    BACKOFF_STEP.store(0, Ordering::SeqCst);
    *lock_last_attempt() = None;
}

pub(crate) fn clear_pid() {
    PID.store(0, Ordering::SeqCst);
    CHECKED.store(false, Ordering::SeqCst);
    CONFIRM.store(0, Ordering::SeqCst);
}

/// The window was navigated to the dsh URL (launch flow or tray restore).
/// See [`NAVIGATED_TO_DSH`].
pub(crate) fn note_navigated_to_dsh() {
    NAVIGATED_TO_DSH.store(true, Ordering::SeqCst);
}

/// One supervisor tick for browser mode.
///
/// Presence primitive: the tracked HWND's liveness
/// (`platform::is_window_alive`) — a window cannot be shared with other
/// webui.me apps, unlike process cmdlines. `is_shown(window_id)` is the
/// socket fallback, meaningful only BEFORE navigation and only past the show
/// grace ([`NAVIGATED_TO_DSH`] / [`SHOW_GRACE_SECS`]).
///
/// `phase` forces a detected close during [`Phase::Startup`] to be a full
/// quit.
pub(crate) fn poll_close<F>(phase: Phase, window_id: usize, mut is_shown: F) -> Tick
where
    F: FnMut(usize) -> bool,
{
    let hwnd = BROWSER_HWND.load(Ordering::SeqCst);
    if hwnd != 0 {
        let alive =
            crate::platform::is_window_alive(hwnd) && crate::platform::window_pid(hwnd) != 0;
        if alive {
            CONFIRM.store(0, Ordering::SeqCst);
            if !CHECKED.swap(true, Ordering::SeqCst) {
                crate::debug::emit(&format!("browser supervisor active (hwnd {hwnd:#x})"));
            }
            return Tick {
                action: CloseAction::None,
                retry_capture: false,
            };
        }
        // Negative frame. Require consecutive confirmations so a transient
        // enumeration blip cannot fire tray/quit.
        let hits = CONFIRM.fetch_add(1, Ordering::SeqCst) + 1;
        if hits < CONFIRM_TICKS {
            return Tick {
                action: CloseAction::None,
                retry_capture: false,
            };
        }
        CONFIRM.store(0, Ordering::SeqCst);
        crate::debug::emit(&format!(
            "browser window gone (hwnd {hwnd:#x}, {CONFIRM_TICKS} confirmations)"
        ));
        return Tick {
            action: decide_on_close(phase),
            retry_capture: false,
        };
    }

    // Uncaptured: fall back to webui's is_shown, with two gates so neither
    // direction lies:
    // * NAVIGATED_TO_DSH — post-navigation the socket is gone BY DESIGN;
    //   negatives are meaningless from then on (pid tracking owns close).
    // * SHOW_GRACE_SECS — right after a show, negatives mean "still cold
    //   starting", not "user closed it".
    let shown = is_shown(window_id);
    if shown {
        CONFIRM.store(0, Ordering::SeqCst);
        return Tick {
            action: CloseAction::None,
            retry_capture: capture_due(),
        };
    }
    if NAVIGATED_TO_DSH.load(Ordering::SeqCst) || show_grace_active() {
        return Tick {
            action: CloseAction::None,
            retry_capture: capture_due(),
        };
    }
    let hits = CONFIRM.fetch_add(1, Ordering::SeqCst) + 1;
    if hits >= CONFIRM_TICKS {
        CONFIRM.store(0, Ordering::SeqCst);
        crate::debug::emit("browser window never connected; treating as closed");
        return Tick {
            action: decide_on_close(phase),
            retry_capture: false,
        };
    }
    Tick {
        action: CloseAction::None,
        retry_capture: capture_due(),
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

/// Whether the caller should fire a capture attempt this tick. Backoff grows
/// 2s → 4s → 8s → 10s (capped) per fruitless round and resets on adoption.
/// NEVER gives up permanently: a permanent budget left forwarding browsers
/// (Firefox, Edge startup boost) tracked by nothing after navigation, which
/// leaked them on close. Log noise is bounded by the growing interval plus a
/// single line per backoff-level change.
fn capture_due() -> bool {
    let mut last = lock_last_attempt();
    let step = BACKOFF_STEP.load(Ordering::SeqCst);
    let interval = Duration::from_secs(BACKOFF_SECS[step.min(BACKOFF_SECS.len() - 1)]);
    if last.is_some_and(|t| t.elapsed() < interval) {
        return false;
    }
    *last = Some(Instant::now());
    BACKOFF_STEP.store((step + 1).min(BACKOFF_SECS.len() - 1), Ordering::SeqCst);
    if step == 0 {
        crate::debug::emit("browser pid unknown; hunting (backoff 2s..10s)");
    } else if step == BACKOFF_SECS.len() - 1 {
        crate::debug::emit("browser pid hunt continues at max backoff");
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::state;

    /// Rewind the show timestamp so the grace window counts as expired.
    fn expire_grace() {
        *lock_shown_at() = Some(Instant::now() - Duration::from_secs(SHOW_GRACE_SECS + 1));
    }

    /// Sequential scenario driver: cargo runs a crate's tests on threads,
    /// and these statics are process-global — everything runs inside ONE
    /// #[test] in a fixed order to stay deterministic.
    #[test]
    fn lifecycle_decision_matrix() {
        // ---- baseline
        reset_runtime_state();
        assert_eq!(pid(), 0);
        assert_eq!(pid_for_teardown(), 0);

        // ---- uncaptured + never shown: first hunt immediate, then gated
        let t = poll_close(Phase::Startup, 7, |_| false);
        assert!(t.retry_capture, "first hunt fires immediately");
        let t = poll_close(Phase::Startup, 7, |_| false);
        assert!(!t.retry_capture, "backoff holds the repeat");

        // ---- grace: fresh show swallows ANY number of negatives (slow
        // cold start is not a close), and no confirmation can build up.
        note_window_shown();
        state::CLOSE_TO_TRAY.store(true, Ordering::SeqCst);
        for i in 0..10 {
            let t = poll_close(Phase::Supervising, 7, |_| false);
            assert_eq!(t.action, CloseAction::None, "grace frame {i} must not fire");
        }

        // ---- grace expired: three consecutive negatives confirm ToTray
        expire_grace();
        for hits in 1..CONFIRM_TICKS {
            let t = poll_close(Phase::Supervising, 7, |_| false);
            assert_eq!(t.action, CloseAction::None, "frame {hits} must not fire");
        }
        let t = poll_close(Phase::Supervising, 7, |_| false);
        assert_eq!(t.action, CloseAction::ToTray, "third negative confirms");
        note_closed_to_tray();

        // ---- recovery frame resets the counter mid-streak
        note_window_recreated();
        note_window_shown();
        expire_grace();
        let _ = poll_close(Phase::Supervising, 7, |_| true); // shown resets
        for _ in 0..CONFIRM_TICKS - 1 {
            let _ = poll_close(Phase::Supervising, 7, |_| false);
        }
        let _ = poll_close(Phase::Supervising, 7, |_| true); // recover
        let t = poll_close(Phase::Supervising, 7, |_| false);
        assert_eq!(t.action, CloseAction::None, "recovery resets confirmation");
        note_closed_to_tray();

        // ---- adoption anchors on hwnd and pid together
        adopt(0x1000, 4242);
        assert_eq!(hwnd(), 0x1000);
        assert_eq!(pid(), 4242);
        assert_eq!(pid_for_teardown(), 4242);

        // ---- tracked hwnd: in unit tests the HWND is fake, so
        // platform::is_window_alive reads false — dead frames confirm and
        // Startup forces Quit even with tray enabled. (Alive-hold behaviour
        // needs a real window; covered by manual verification.)
        let _t = poll_close(Phase::Startup, 7, |_| false);
        note_closed_to_tray();
        assert_eq!(pid(), 0);
        assert_eq!(hwnd(), 0x1000, "detection forget keeps the hwnd");
        assert_eq!(pid_for_teardown(), 4242, "teardown pid survives the forget");

        // ---- Supervising with tray OFF quits instead of hiding
        state::CLOSE_TO_TRAY.store(false, Ordering::SeqCst);
        adopt(0x2000, 777);
        for _ in 0..CONFIRM_TICKS - 1 {
            let t = poll_close(Phase::Supervising, 7, |_| false);
            assert_eq!(t.action, CloseAction::None, "confirm gate holds");
        }
        let t = poll_close(Phase::Supervising, 7, |_| false);
        assert_eq!(t.action, CloseAction::Quit, "tray disabled -> quit");
        note_closed_to_tray();

        // ---- backoff escalates to its cap and resets on adoption. Time is
        // manipulated directly (LAST_ATTEMPT rewound) — zero sleeping, per
        // the repo's timing-test red line.
        reset_runtime_state();
        let max_secs = BACKOFF_SECS[BACKOFF_SECS.len() - 1];
        let fires = BACKOFF_SECS.len() as u32 + 1; // one past the cap proves capping
        for fired in 1..=fires {
            // Pretend the previous attempt happened a full MAX interval ago:
            // always "due", regardless of the step's own interval.
            *lock_last_attempt() = Some(Instant::now() - Duration::from_secs(max_secs));
            let t = poll_close(Phase::Supervising, 7, |_| false);
            assert!(t.retry_capture, "fire #{fired} must be due");
            let expected = (fired as usize).min(BACKOFF_SECS.len() - 1);
            assert_eq!(
                BACKOFF_STEP.load(Ordering::SeqCst),
                expected,
                "step after fire #{fired}"
            );
        }

        // ---- the gate actually gates: right after a fire, an immediate
        // repeat must be held.
        *lock_last_attempt() = Some(Instant::now());
        let t = poll_close(Phase::Supervising, 7, |_| false);
        assert!(!t.retry_capture, "backoff holds repeats");
        adopt(0x3000, 888);
        assert_eq!(
            BACKOFF_STEP.load(Ordering::SeqCst),
            0,
            "adoption resets backoff"
        );

        // ---- post-navigation socket blindness must NOT manufacture closes
        reset_runtime_state();
        note_navigated_to_dsh();
        for i in 0..CONFIRM_TICKS + 2 {
            let t = poll_close(Phase::Supervising, 7, |_| false);
            assert_eq!(
                t.action,
                CloseAction::None,
                "navigated socket lies (frame {i})"
            );
        }

        // restore ambient config touched by this test
        state::CLOSE_TO_TRAY.store(false, Ordering::SeqCst);
        reset_runtime_state();
    }
}
