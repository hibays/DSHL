//! Periodic dsh update check (Rust side, every [`CHECK_INTERVAL`]).
//!
//! A single background task on the shared runtime asks the configured
//! registry for the latest `@deepseek-ai/dsh` and records whether the copy
//! dshl would run is behind. The result is published in three places:
//!
//! * `progress::State::update` — the startup page shows it (a kv row);
//! * the launcher log (only when the answer *changes*, plus failures);
//! * the control plane (`update-status` / `check-update`).
//!
//! # Deliberate properties
//!
//! * **Check only, never install.** Installing rewrites the very cache a
//!   running dsh executes from (Windows sharing violations — see the audit in
//!   `docs/`) and only the startup pipeline knows the cache is quiescent.
//!   The timer therefore *reports*; installation stays in `flow::prepare`.
//! * **The result feeds the startup resolver.** When the inline 3-second
//!   query fails at boot (flaky mirror, DNS hiccup), [`recent_latest`] hands
//!   back the last successful answer so auto-update does not silently stop
//!   working just because that one query timed out.
//! * **Bounded and self-cleaning**: the registry query keeps the 3s cap used
//!   on the startup path and the global-shim probe keeps 15s; both go through
//!   [`process::run_bounded`], which kills a child that overruns.
//! * **Offline is a normal outcome**: a failed tick keeps the previous status
//!   and logs one line.
//! * **No thread of its own**: the loop is a task on the shared tokio runtime
//!   (see [`crate::runtime`]), so it costs nothing while it sleeps.

use std::ffi::{OsStr, OsString};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use crate::config::{Config, DshMode, Pm};
use crate::flow::prepare;
use crate::mirror::MirrorConfig;
use crate::platform;
use crate::probe;
use crate::process;
use crate::progress;
use crate::version::FullVersion;

/// Steady-state cadence of the background check.
pub const CHECK_INTERVAL: Duration = Duration::from_secs(2 * 60 * 60);

/// Delay before the first background check.
///
/// The startup pipeline already performs an inline query, so this is not
/// needed for boot; five minutes keeps the timer from competing with the
/// launch (which may be installing) while still producing a visible result
/// within a normal session.
const FIRST_DELAY: Duration = Duration::from_secs(5 * 60);

/// Cap on the registry query (same budget as the startup path).
const QUERY_TIMEOUT: Duration = Duration::from_secs(3);

/// Cap on probing the user's global `dsh` for its version.
const PROBE_TIMEOUT: Duration = Duration::from_secs(15);

/// How long a recorded `latest` stays authoritative for the startup resolver.
///
/// Comfortably longer than [`CHECK_INTERVAL`], so a healthy session always has
/// a fresh answer available, while a launcher that sat idle for days does not
/// resurrect an ancient one.
const RECENT_TTL: Duration = Duration::from_secs(6 * 60 * 60);

/// Everything a tick needs, snapshotted from the launch pipeline.
struct Ctx {
    config: Config,
    mirror: MirrorConfig,
    /// Runtime PATH prefix (where dshl keeps the toolchain it installed).
    prefix: Vec<PathBuf>,
    /// The augmented `PATH` handed to the version-query subprocess.
    path: OsString,
}

/// Outcome of one successful check.
#[derive(Debug, Clone)]
pub struct Status {
    /// Latest version published under the registry's `latest` dist-tag.
    pub latest: FullVersion,
    /// The version dshl would run (cache copy, else the global `dsh`).
    pub current: Option<FullVersion>,
    /// `latest` is newer than `current`.
    pub available: bool,
    /// `current` is newer than `latest` (the registry moved backwards, e.g.
    /// a dist-tag was re-pointed). Reported so the user can see WHY the
    /// launcher keeps a newer cache copy instead of "updating" to an older
    /// release.
    pub ahead: bool,
    pub checked_at: SystemTime,
}

static CTX: Mutex<Option<Ctx>> = Mutex::new(None);
static STATUS: Mutex<Option<Status>> = Mutex::new(None);
static STARTED: OnceLock<()> = OnceLock::new();

/// Hand the timer the current launch context and start the loop (idempotent).
///
/// Called by the startup pipeline on every launch: the snapshot is refreshed
/// each time so a re-configured or re-launched launcher checks against what
/// it is actually running.
pub fn configure(config: &Config, mirror: &MirrorConfig, runtime: &crate::install::Runtime) {
    let ctx = Ctx {
        config: config.clone(),
        mirror: mirror.clone(),
        prefix: runtime.path_prefix(),
        path: runtime.augmented_path(),
    };
    *lock_ctx() = Some(ctx);
    start();
}

/// Start the background loop once per process.
pub fn start() {
    STARTED.get_or_init(|| {
        crate::runtime::spawn(async move {
            tokio::time::sleep(FIRST_DELAY).await;
            loop {
                let _ = check_once().await;
                tokio::time::sleep(CHECK_INTERVAL).await;
            }
        });
    });
}

/// Run one check. `None` when no context has been configured yet, or when the
/// registry could not be reached (the previous status is then kept).
pub async fn check_once() -> Option<Status> {
    let (pm, prefix, path, env, mode) = {
        let guard = lock_ctx();
        let ctx = guard.as_ref()?;
        (
            ctx.config.dsh.pm,
            ctx.prefix.clone(),
            ctx.path.clone(),
            ctx.mirror.npm_env(),
            ctx.config.dsh.mode,
        )
    };

    // A failed query must NOT overwrite a good status with "no update
    // available" — it just leaves the last known answer standing.
    let Some(latest) = query_latest(pm, &prefix, &path, &env).await else {
        progress::log(t!("update.failed"));
        return None;
    };

    let current = resolve_current(mode, &prefix).await;
    let (available, ahead) = classify(&latest, current.as_ref());
    let status = Status {
        latest,
        current,
        available,
        ahead,
        checked_at: SystemTime::now(),
    };

    let previous = status_snapshot();
    let changed = previous
        .as_ref()
        .is_none_or(|p| p.available != status.available || p.latest != status.latest);
    store(&status);
    if changed {
        progress::log(update_text(&status));
    }
    Some(status)
}

/// A version string for "what the launcher would run" (cache copy first, then
/// the user's global `dsh`).
///
/// The cache is checked first because that is the copy dshl installs and
/// updates — i.e. the one an update would change. `global` mode has no cache
/// by design, so it reports the global.
async fn resolve_current(mode: DshMode, prefix: &[PathBuf]) -> Option<FullVersion> {
    match mode {
        // Private mode only ever runs the cache copy.
        DshMode::Private => prepare::cached_version(),
        // Hybrid prefers the user's global dsh — that is what actually runs
        // whenever it satisfies the version requirement. Reporting the cache
        // first would announce an "update" to a version the launcher is
        // already running.
        DshMode::Hybrid => match probe_global(prefix).await {
            Some(global) => Some(global),
            None => prepare::cached_version(),
        },
        DshMode::Global => probe_global(prefix).await,
    }
}

/// Version of the user's global `dsh` shell, bounded and failure-tolerant.
async fn probe_global(prefix: &[PathBuf]) -> Option<FullVersion> {
    match tokio::time::timeout(PROBE_TIMEOUT, probe::dsh_in(prefix)).await {
        Ok(tool) if tool.found => FullVersion::parse(&tool.raw),
        _ => None,
    }
}

/// `(available, ahead)` for a `latest`/`current` pair. Pure — unit-tested.
///
/// With nothing installed yet there is nothing to update TO, so an empty
/// `current` is not reported as an available update (the launch pipeline is
/// about to install `latest` anyway).
fn classify(latest: &FullVersion, current: Option<&FullVersion>) -> (bool, bool) {
    match current {
        Some(current) => (latest > current, current > latest),
        None => (false, false),
    }
}

/// The registry `latest` version, if a check succeeded within [`RECENT_TTL`].
///
/// Used by `flow::prepare` as a fallback when its own inline query fails.
pub fn recent_latest() -> Option<FullVersion> {
    let status = status_snapshot()?;
    is_recent(status.checked_at, SystemTime::now(), RECENT_TTL).then_some(status.latest)
}

/// Is a recorded check still authoritative? Pure — unit-tested.
///
/// A timestamp in the future (clock skew) counts as NOT recent: the value is
/// only a fallback, so being conservative costs at most one extra query.
fn is_recent(checked_at: SystemTime, now: SystemTime, ttl: Duration) -> bool {
    now.duration_since(checked_at)
        .map(|age| age <= ttl)
        .unwrap_or(false)
}

/// Forget the recorded status.
///
/// Called after the launcher installs dsh itself: the published "有新版本"
/// row would otherwise keep claiming an update is pending until the next tick.
pub fn invalidate() {
    *STATUS.lock().unwrap_or_else(|p| p.into_inner()) = None;
    progress::set_update(None);
}

/// The recorded status, if any check has succeeded in this process.
pub fn status_snapshot() -> Option<Status> {
    STATUS.lock().unwrap_or_else(|p| p.into_inner()).clone()
}

/// The status as JSON for the control plane.
pub fn status_json() -> Value {
    match status_snapshot() {
        Some(s) => json!({
            "checked": true,
            "latest": s.latest.to_string(),
            "current": s.current.as_ref().map(|v| v.to_string()),
            "available": s.available,
            "ahead": s.ahead,
            "checked_at": unix_secs(s.checked_at),
            "text": update_text(&s),
        }),
        None => json!({ "checked": false }),
    }
}

/// Ask the registry for the latest published `@deepseek-ai/dsh`.
///
/// One implementation for both callers (the startup pipeline and the timer).
/// The package-manager choice mirrors what dshl would use for the install
/// itself; `npm` answers for npm/bun (it ships with node and reads the same
/// registry config), while pnpm/nub expose their own `view`.
pub(crate) async fn query_latest(
    pm: Pm,
    prefix: &[PathBuf],
    path: &OsStr,
    env: &[(String, String)],
) -> Option<FullVersion> {
    let tool = match pm {
        Pm::Npm | Pm::Bun => "npm",
        Pm::Pnpm => "pnpm",
        Pm::Nub => "nub",
    };
    let mut cmd = std::process::Command::new(platform::tool_in(tool, prefix));
    cmd.args(["view", "@deepseek-ai/dsh", "version"]);
    cmd.env("PATH", path);
    process::with_env(&mut cmd, env);
    // npm view normally answers in ~1s; the cap keeps a slow or blocked
    // registry from holding the caller (startup page, timer tick) on a stall.
    let res = process::run_bounded(&mut cmd, QUERY_TIMEOUT).await.ok()?;
    if !res.success() {
        return None;
    }
    FullVersion::parse(res.stdout.trim())
}

/// Store the status and publish it to the UI state.
fn store(status: &Status) {
    *STATUS.lock().unwrap_or_else(|p| p.into_inner()) = Some(status.clone());
    progress::set_update(Some(progress::UpdateInfo {
        latest: status.latest.to_string(),
        current: status.current.as_ref().map(|v| v.to_string()),
        available: status.available,
        ahead: status.ahead,
        checked_at: unix_secs(status.checked_at),
        text: update_text(status),
    }));
}

/// Localized one-liner describing the status (log line and UI row).
fn update_text(status: &Status) -> String {
    let latest = status.latest.to_string();
    let current = status
        .current
        .as_ref()
        .map(|v| v.to_string())
        .unwrap_or_else(|| t!("update.unknown_current").to_string());
    if status.available {
        t!("update.available", latest = latest, current = current).to_string()
    } else if status.ahead {
        t!("update.ahead", latest = latest, current = current).to_string()
    } else if status.current.is_none() {
        // Nothing installed anywhere (global mode on a bare machine, or an
        // empty cache): "up to date" would be a lie.
        t!("update.not_installed").to_string()
    } else {
        t!("update.up_to_date", latest = latest).to_string()
    }
}

fn lock_ctx() -> std::sync::MutexGuard<'static, Option<Ctx>> {
    CTX.lock().unwrap_or_else(|p| p.into_inner())
}

fn unix_secs(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> FullVersion {
        FullVersion::parse(s).expect("test version parses")
    }

    #[test]
    fn cadence_is_two_hours() {
        assert_eq!(CHECK_INTERVAL, Duration::from_secs(7200));
        // The startup fallback must outlive at least one full interval, or a
        // healthy session could lose its answer between two ticks.
        assert!(RECENT_TTL > CHECK_INTERVAL);
        assert!(FIRST_DELAY < CHECK_INTERVAL);
    }

    #[test]
    fn classify_reports_behind_and_ahead() {
        // Behind: an update is available.
        assert_eq!(classify(&v("0.1.5-rc.2"), Some(&v("0.1.0"))), (true, false));
        // Equal: up to date, neither flag.
        assert_eq!(
            classify(&v("0.1.5-rc.2"), Some(&v("0.1.5-rc.2"))),
            (false, false)
        );
        // Ahead: the registry's `latest` moved BACKWARDS (e.g. a dist-tag was
        // re-pointed from 0.1.6-alpha.2 to 0.1.5-rc.2). Never an "update".
        assert_eq!(
            classify(&v("0.1.5-rc.2"), Some(&v("0.1.6-alpha.2"))),
            (false, true)
        );
        // Nothing installed yet: the pipeline is about to install it.
        assert_eq!(classify(&v("0.1.5-rc.2"), None), (false, false));
    }

    /// A check with no configured context must be a no-op — and, crucially,
    /// must not touch the network (the control-plane tests rely on this).
    #[test]
    fn check_without_context_is_a_noop() {
        assert!(crate::runtime::block_on(check_once()).is_none());
        assert!(status_snapshot().is_none());
        assert!(recent_latest().is_none());
        assert_eq!(status_json()["checked"], serde_json::json!(false));
    }

    #[test]
    fn stale_status_is_not_reused() {
        let now = SystemTime::now();
        let ttl = Duration::from_secs(3600);
        // Fresh, boundary and stale — this is exactly the guard
        // `recent_latest` applies (the previous version of this test only
        // re-implemented the comparison, so the branch had no guard at all).
        assert!(is_recent(now - Duration::from_secs(60), now, ttl));
        assert!(is_recent(now - ttl, now, ttl));
        assert!(!is_recent(now - ttl - Duration::from_secs(1), now, ttl));
        // A timestamp in the future (clock skew) is not authoritative either.
        assert!(!is_recent(now + Duration::from_secs(60), now, ttl));
    }
}
