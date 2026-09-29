//! Shared, thread-safe progress/status state.
//!
//! The flow worker writes into this state; the webui main thread reads it back
//! as JSON whenever the startup page polls. Keeping it UI-agnostic lets the
//! flow code stay decoupled from webui.

use std::collections::VecDeque;
use std::sync::{LazyLock, Mutex};

use serde::Serialize;

/// Status of a single startup step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum StepStatus {
    Pending,
    Running,
    Done,
    Error,
    Skipped,
}

#[derive(Debug, Clone, Serialize)]
pub struct Step {
    pub id: String,
    pub title: String,
    pub status: StepStatus,
    pub message: String,
}

/// Crash-recovery banner: while dsh exited unexpectedly and an auto-restart is
/// pending, the seconds until it restarts (the page shows a countdown with
/// 立即重启 / 取消). `None` when no auto-restart is pending.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CrashState {
    pub code: i32,
    pub countdown: u8,
}

/// Result of the background update check (see [`crate::update_check`]).
///
/// `text` is the localized one-liner the page renders as a config row value,
/// so the frontend never has to assemble version strings itself.
#[derive(Debug, Clone, Serialize)]
pub struct UpdateInfo {
    pub latest: String,
    pub current: Option<String>,
    /// The registry's `latest` is newer than the installed copy.
    pub available: bool,
    /// The installed copy is newer than the registry's `latest` (a dist-tag
    /// moved backwards) — never reported as an update.
    pub ahead: bool,
    /// Unix seconds of the check that produced this entry.
    pub checked_at: u64,
    pub text: String,
}

/// Result of the launcher's OWN update check / staging
/// (see [`crate::self_update`]).
///
/// `text` is the localized one-liner the page renders as a config row value;
/// the frontend never assembles version strings itself.
#[derive(Debug, Clone, Serialize)]
pub struct SelfUpdateInfo {
    pub current: String,
    pub latest: Option<String>,
    /// A newer launcher exists (whether or not it is downloaded yet).
    pub available: bool,
    /// The archive is on disk and will be applied at the next start.
    pub staged: bool,
    /// Swapped in during this run — effective after a restart.
    pub applied: Option<String>,
    /// Release page, for the manual route.
    pub url: Option<String>,
    /// Unix seconds of the check that produced this entry.
    pub checked_at: u64,
    pub text: String,
}

/// Full UI snapshot.
#[derive(Debug, Clone, Serialize)]
pub struct State {
    pub steps: Vec<Step>,
    pub logs: VecDeque<String>,
    pub error: Option<String>,
    pub url: Option<String>,
    pub config_path: String,
    pub config_json: String,
    pub config_error: Option<String>,
    /// PID of a stale dsh awaiting the user's confirmation to force-kill.
    pub stale_pid: Option<u32>,
    /// Crash-recovery banner (dsh exited unexpectedly, auto-restart pending).
    pub crash: Option<CrashState>,
    /// Latest background update-check result (`None` before the first check).
    pub update: Option<UpdateInfo>,
    /// Launcher self-update state (`None` until a check ran or one is staged).
    pub self_update: Option<SelfUpdateInfo>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            steps: Vec::new(),
            logs: VecDeque::with_capacity(LOG_CAP),
            error: None,
            url: None,
            config_path: String::new(),
            config_json: String::new(),
            config_error: None,
            stale_pid: None,
            crash: None,
            update: None,
            self_update: None,
        }
    }
}

const LOG_CAP: usize = 500;

static STATE: LazyLock<Mutex<State>> = LazyLock::new(|| Mutex::new(State::default()));

/// Initialise the step list (id, title) and clear transient state.
pub fn reset(steps: &[(&'static str, String)]) {
    let mut state = STATE.lock().unwrap();
    state.steps = steps
        .iter()
        .map(|(id, title)| Step {
            id: id.to_string(),
            title: title.clone(),
            status: StepStatus::Pending,
            message: String::new(),
        })
        .collect();
    state.logs.clear();
    state.error = None;
    state.url = None;
    state.stale_pid = None;
    state.crash = None;
}

/// Update a step's status/message.
pub fn step(id: &str, status: StepStatus, message: impl Into<String>) {
    let message: String = message.into();
    let mut state = STATE.lock().unwrap();
    for s in state.steps.iter_mut() {
        if s.id == id {
            s.status = status;
            s.message = message.clone();
        }
    }
    crate::debug::emit(&format!("[step {id}] {status:?}: {message}"));
}

/// Append a log line (capped).
pub fn log(line: impl Into<String>) {
    let line = line.into();
    {
        let mut state = STATE.lock().unwrap();
        if state.logs.len() >= LOG_CAP {
            state.logs.pop_front();
        }
        state.logs.push_back(line.clone());
    }
    crate::debug::emit(&line);
}

/// Set a fatal/blocking error (rendered prominently in the UI).
pub fn set_error(message: impl Into<String>) {
    STATE.lock().unwrap().error = Some(message.into());
}

/// Record the PID of a stale dsh awaiting user confirmation to force-kill.
pub fn set_stale_pid(pid: Option<u32>) {
    STATE.lock().unwrap().stale_pid = pid;
}

/// Read the stale-dsh PID (0 = none). Single source of truth — the launcher
/// no longer keeps a parallel `AtomicU32` in `ui::state`.
pub fn stale_pid() -> u32 {
    STATE.lock().unwrap().stale_pid.unwrap_or(0)
}

/// Set the crash-recovery banner: dsh exited unexpectedly and will be
/// auto-restarted after `countdown` seconds (unless the user cancels).
pub fn set_crash(code: i32, countdown: u8) {
    STATE.lock().unwrap().crash = Some(CrashState { code, countdown });
}

/// Update the remaining countdown (`None` ends the banner).
pub fn set_crash_countdown(countdown: Option<u8>) {
    let mut state = STATE.lock().unwrap();
    if let Some(crash) = &mut state.crash {
        if let Some(secs) = countdown {
            crash.countdown = secs;
        } else {
            state.crash = None;
        }
    }
}

/// Publish the latest background update-check result.
///
/// Deliberately NOT cleared by [`reset`]: it is a session-level fact with a
/// 2-hour cadence, so clearing it on every relaunch would blank the row until
/// the next tick.
pub fn set_update(info: Option<UpdateInfo>) {
    STATE.lock().unwrap().update = info;
}

/// Publish the launcher's own update state (also survives [`reset`], so a
/// staged/applied update stays visible across a relaunch).
pub fn set_self_update(info: Option<SelfUpdateInfo>) {
    STATE.lock().unwrap().self_update = info;
}

pub fn clear_error() {
    STATE.lock().unwrap().error = None;
}

pub fn set_url(url: impl Into<String>) {
    STATE.lock().unwrap().url = Some(url.into());
}

/// Clear the dsh URL (e.g. when dsh exited — nothing is "running" anymore).
pub fn clear_url() {
    STATE.lock().unwrap().url = None;
}

/// Record the resolved config for display / config control.
pub fn set_config(config_json: String, path: String, error: Option<String>) {
    let mut state = STATE.lock().unwrap();
    state.config_json = config_json;
    state.config_path = path;
    state.config_error = error;
}

/// Clone the current state.
pub fn snapshot() -> State {
    STATE.lock().unwrap().clone()
}

/// Serialise the current state as JSON for the frontend.
pub fn to_json() -> String {
    serde_json::to_string(&*STATE.lock().unwrap()).unwrap_or_else(|_| "{}".into())
}
