//! External-browser process tracking: pid capture and the geometry sampler.
//!
//! The launcher never owns the external browser; it *adopts* a pid only when
//! that pid currently owns a visible top-level window (single-instance
//! forwarders match the command-line needle for an instant and then die), and
//! samples its real rect afterwards so the next launch restores what the user
//! last left.

use std::sync::atomic::{AtomicBool, Ordering};

use webui::webui;

use crate::ui::{browser, geometry, state};

/// Set while a browser-pid capture is polling, so concurrent triggers (the
/// post-show capture and the supervisor's startup retries) collapse into one
/// poll instead of racing: two finders would each store a pid and each start
/// their own geometry sampler (duplicate sampling, duplicate state writes).
static CAPTURE_IN_PROGRESS: AtomicBool = AtomicBool::new(false);

pub(crate) fn capture_browser_pid() {
    // A capture is already in progress — let it finish.
    if CAPTURE_IN_PROGRESS.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(|| {
        let found = locate_browser_pid();
        // Reset on every path (port never assigned, pid found, polls
        // exhausted, race lost) so later captures stay possible. This runs
        // before the blocking geometry sampler starts.
        CAPTURE_IN_PROGRESS.store(false, Ordering::SeqCst);
        if let Some(pid) = found {
            track_browser_geometry(pid);
        }
    });
}

/// Poll for the external browser window's pid, handing it to
/// [`browser::adopt`] once found. `None` when the window server port never
/// got assigned, the polls ran out, or another capture stored a pid first.
fn locate_browser_pid() -> Option<u32> {
    let window = webui::Window::from_id(state::WINDOW_ID.load(Ordering::SeqCst));

    // Wait for the window server to assign its port.
    let mut port = 0usize;
    for _ in 0..40 {
        port = window.get_port();
        if port != 0 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    if port == 0 {
        crate::debug::emit("window server port never assigned");
        return None;
    }

    // Needle is the SESSION-UNIQUE `--app=http://localhost:<port>` command
    // line. Deliberately NO profile-dir fallback: webui hands EVERY
    // webui.me app the same shared `.WebUI` profile dir, so a profile probe
    // would also match (and adopt!) other apps' browsers — misattributing
    // their windows to us broke both focus and close detection.
    let needle = format!("--app=http://localhost:{port}");
    for _ in 0..10 {
        if let Some(pid) = crate::platform::find_process_by_cmdline(&needle) {
            // Lost the race: another capture already stored a pid (and
            // started its own geometry sampler) — don't store again.
            if browser::hwnd() != 0 {
                return None;
            }
            // The identity anchor is the HWND, not the process command line:
            // resolve the owning pid from the window we found.
            let hwnd = crate::platform::find_hwnd_by_pid(pid).unwrap_or(0);
            if hwnd == 0 {
                crate::debug::emit("capture rejected: no visible window yet; keep hunting");
                std::thread::sleep(std::time::Duration::from_millis(500));
                continue;
            }
            browser::adopt(hwnd, pid);
            crate::debug::emit(&format!("browser window pid {pid} (port {port})"));
            return Some(pid);
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    crate::debug::emit("failed to locate the browser window pid");
    None
}

/// Sample the external browser window's geometry every second and persist
/// it, so the next launch restores the position/size the user last left the
/// window in. Exits when the browser process dies (the last sample is
/// already on disk by then). Best-effort: only implemented on Windows.
///
/// Two guards prevent TRANSIENT states from poisoning the store — Edge opens
/// an `--app` window at its own per-profile remembered bounds and only later
/// applies our resizeTo/moveTo (and ignores --window-size entirely when the
/// launch joins an existing instance):
/// * SETTLE_SECS after adoption, nothing is persisted;
/// * a rect must be observed IDENTICAL on two consecutive samples before it
///   is written (a frame that lives for less than a second never lands).
fn track_browser_geometry(pid: u32) {
    const SETTLE_SECS: u64 = 5;
    let adopted_at = std::time::Instant::now();
    let mut last: Option<(i32, i32, u32, u32)> = None;
    let mut pending: Option<(i32, i32, u32, u32)> = None;
    loop {
        if !crate::platform::process_alive(pid) {
            crate::debug::emit("track geometry: browser process exited");
            return;
        }
        if let Some(hwnd) = crate::platform::find_hwnd_by_pid(pid)
            && let Some(rect) = crate::platform::window_rect(hwnd)
            && !rect.maximized
        {
            let key = (rect.x, rect.y, rect.width, rect.height);
            let settled = adopted_at.elapsed().as_secs() >= SETTLE_SECS;
            if settled && pending == Some(key) && last != Some(key) {
                last = Some(key);
                geometry::persist(rect.x, rect.y, rect.width, rect.height);
                crate::debug::emit(&format!(
                    "track geometry: {}x{} @ ({},{})",
                    rect.width, rect.height, rect.x, rect.y
                ));
            }
            pending = Some(key);
        } else {
            pending = None;
        }
        std::thread::sleep(std::time::Duration::from_millis(1000));
    }
}
