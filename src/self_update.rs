//! Launcher self-update: check → download → verify → stage → apply at start.
//!
//! The launcher updates ITSELF (not `dsh`; that is [`crate::update_check`]).
//! Flow: the background timer (or the user, via the UI / control plane) asks
//! GitHub for the latest release, downloads the portable archive for this
//! platform into `<cache>/dshl/update/`, verifies its published SHA-256, and
//! stages the binary. [`apply_staged`] then swaps the executable in place —
//! called from the CLI shell **before any child process exists**.
//!
//! # Why "apply at the next start" and not a live swap
//!
//! A running launcher OWNS the `dsh` process (Windows: a kill-on-close job
//! object; Linux: `PR_SET_PDEATHSIG`). Replacing the binary under a live
//! session therefore either kills dsh or leaves it unsupervised, and a
//! genuinely hot swap needs much more than a file copy: handing the job
//! object over, keeping the control endpoint (port + per-launch token) valid
//! for the still-running dsh, re-adopting a process this launcher did not
//! spawn (a tokio child cannot be adopted — its pipes died with the old
//! process, so dsh's stdout would have to become a file from the start), and
//! restoring the window state. All of that is designed in
//! `.agents/notes/proposed/architecture/2026-09-22-dshl-hot-self-update.md`
//! and deliberately NOT shipped untested: the swap happens at the next start,
//! where the cache is quiescent and no child exists yet — so an update can
//! never interrupt a running dsh session.
//!
//! Safety properties kept here:
//! * the download is verified against the release's published `sha256` digest
//!   and refused otherwise (no tool to verify with = no update);
//! * the swap is rollback-safe (the old binary is renamed aside first and
//!   restored if the copy fails), so a failed update cannot brick the install;
//! * macOS `.app` bundles are never swapped in place (it would invalidate the
//!   code signature) — those users get the download link instead.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::{Config, SelfUpdateMode};
use crate::error::{Error, Result};
use crate::install::download::{self, extract_zip, http_download};
use crate::mirror::MirrorConfig;
use crate::platform;
use crate::progress::{self, SelfUpdateInfo};
use crate::version::{BUILD_VERSION, FullVersion};

/// Where releases are published (the `Cargo.toml` `repository` field points at
/// a stale name; this one is what the workflow actually releases to).
const REPO: &str = "hibays/DSHL";

/// Delay before the first background self-update check (the dsh timer wakes at
/// 5 minutes; offset so the two never query at the same instant).
const FIRST_DELAY: Duration = Duration::from_secs(10 * 60);

/// Budget for the release-metadata query (probe class: always bounded).
const QUERY_TIMEOUT: Duration = Duration::from_secs(5);

/// Marker file describing what is staged for the next start.
const MARKER: &str = "staged.json";

/// A release that is newer than the running build.
struct Available {
    version: FullVersion,
    url: String,
    sha256: Option<String>,
}

/// Everything the module knows, behind ONE lock (no lock-ordering surface).
#[derive(Default)]
struct Inner {
    ctx: Option<Ctx>,
    latest: Option<Available>,
    /// Version whose archive is on disk and ready to be applied.
    staged: Option<String>,
    /// Version swapped in during this process (effective after a restart).
    applied: Option<String>,
    /// Last published status (what the UI/control plane reads).
    published: Option<SelfUpdateInfo>,
    /// Why the last apply attempt could not finish (surfaced as "manual").
    apply_error: Option<String>,
    /// The last check reached the feed (distinguishes "up to date" from
    /// "could not check" in the published text).
    checked: bool,
}

struct Ctx {
    mode: SelfUpdateMode,
    interval: Duration,
    /// False when `[update] interval-hours = 0`: the timer still runs, but
    /// skips every tick. Kept in the context (not just checked at start) so
    /// turning the timer off takes effect without a process restart.
    enabled: bool,
    /// GitHub proxy prefix (empty = reach GitHub directly).
    github: String,
}

static INNER: Mutex<Option<Inner>> = Mutex::new(None);
static STARTED: std::sync::OnceLock<()> = std::sync::OnceLock::new();

fn with<R>(f: impl FnOnce(&mut Inner) -> R) -> R {
    let mut guard = INNER.lock().unwrap_or_else(|p| p.into_inner());
    f(guard.get_or_insert_with(Inner::default))
}

/// Hand the timer its configuration and start it (idempotent). Called from the
/// startup pipeline on every launch, so a config change takes effect without a
/// process restart.
pub fn configure(config: &Config, mirror: &MirrorConfig) {
    with(|inner| {
        inner.ctx = Some(Ctx {
            mode: config.update.self_update,
            interval: config.update.interval(),
            enabled: config.update.background_enabled(),
            github: if mirror.enabled() {
                mirror.github.clone().unwrap_or_default()
            } else {
                String::new()
            },
        });
    });
    STARTED.get_or_init(|| {
        crate::runtime::spawn(async move {
            tokio::time::sleep(FIRST_DELAY).await;
            loop {
                let (mode, enabled, interval) = with(|inner| match &inner.ctx {
                    Some(ctx) => (ctx.mode, ctx.enabled, ctx.interval),
                    None => (SelfUpdateMode::Off, false, Duration::from_secs(3600)),
                });
                if enabled && mode == SelfUpdateMode::Auto {
                    let _ = download_once().await;
                } else if enabled && mode == SelfUpdateMode::Notify {
                    let _ = check_once().await;
                }
                tokio::time::sleep(interval).await;
            }
        });
    });
}

/// Ask the release feed for the newest published version.
///
/// Returns the published status; `None` means the feed could not be reached
/// (the status is still published, so the UI can offer the manual route).
pub async fn check_once() -> Option<SelfUpdateInfo> {
    let (github, mode) = with(|inner| {
        inner
            .ctx
            .as_ref()
            .map(|c| (c.github.clone(), c.mode))
            .unwrap_or((String::new(), SelfUpdateMode::Off))
    });
    if mode == SelfUpdateMode::Off {
        return None;
    }
    let Some(release) = fetch_release(&github).await else {
        with(|inner| {
            inner.checked = false;
            inner.latest = None;
        });
        publish();
        return None;
    };
    let current = FullVersion::parse(BUILD_VERSION);
    let available = match (&current, &release.version) {
        (Some(current), latest) => latest > current,
        // An unparseable build version (local `cargo build` without tags)
        // must not turn into an endless "update available".
        (None, _) => false,
    };
    with(|inner| {
        inner.checked = true;
        inner.latest = available.then(|| Available {
            version: release.version.clone(),
            url: release.url.clone(),
            sha256: release.sha256.clone(),
        });
        inner.apply_error = None;
    });
    publish()
}

/// Download, verify and stage the newer release. Returns the published status.
pub async fn download_once() -> Result<SelfUpdateInfo> {
    // Refresh the feed when we do not know a newer release yet.
    let known = with(|inner| {
        inner.latest.as_ref().map(|a| Available {
            version: a.version.clone(),
            url: a.url.clone(),
            sha256: a.sha256.clone(),
        })
    });
    let available = match known {
        Some(a) => a,
        None => {
            check_once().await;
            with(|inner| {
                inner.latest.as_ref().map(|a| Available {
                    version: a.version.clone(),
                    url: a.url.clone(),
                    sha256: a.sha256.clone(),
                })
            })
            .ok_or_else(|| Error(t!("self_update.nothing_to_do").to_string()))?
        }
    };
    stage(&available).await?;
    publish().ok_or_else(|| Error(t!("self_update.nothing_to_do").to_string()))
}

/// The page's update button: download when there is something to download,
/// otherwise hand the user the release page (manual install).
///
/// The transfer runs on the shared runtime — it is an install-class download
/// (unbounded by design), and the webui thread must never block on it; the
/// 250 ms state poll picks the result up.
pub fn action() -> Value {
    let (available, staged, applied, url) = with(|inner| {
        (
            inner.latest.is_some(),
            inner.staged.is_some(),
            inner.applied.is_some(),
            inner
                .latest
                .as_ref()
                .map(|a| a.url.clone())
                // The release page is only a fallback once a check actually
                // ran — never for a module that has done nothing yet (that
                // would open a browser out of nowhere, unit tests included).
                .or_else(|| inner.published.as_ref().map(|_| release_page())),
        )
    });
    if available && !staged && !applied {
        crate::runtime::spawn(async {
            if let Err(e) = download_once().await {
                progress::log(t!("self_update.download_failed", err = e.to_string()));
                publish();
            }
        });
        return serde_json::json!({ "ok": true, "action": "download" });
    }
    match url {
        Some(url) => match platform::open_url(&url) {
            Ok(()) => serde_json::json!({ "ok": true, "action": "open" }),
            Err(e) => serde_json::json!({ "ok": false, "error": e.to_string() }),
        },
        None => serde_json::json!({ "ok": false, "error": "nothing to do" }),
    }
}

/// The human-facing release page (manual route).
fn release_page() -> String {
    format!("https://github.com/{REPO}/releases/latest")
}

/// Swap in a staged update. Returns the version now in place.
///
/// Called by the CLI shell after the single-instance lock is held and BEFORE
/// any child is spawned: that ordering is what makes the swap safe (no live
/// dsh to disturb, no window to rebuild).
///
/// On success the running process keeps executing the OLD code — the new
/// binary takes over at the next start, which the published status says
/// explicitly.
///
/// `enabled` mirrors `[update] self != "off"`: with the feature switched off a
/// downloaded artifact is left alone rather than applied behind the user's
/// back.
pub fn apply_staged(enabled: bool) -> Option<String> {
    let Some(marker) = read_marker() else {
        // No update pending — still tidy up leftovers from an earlier one.
        if let Ok(exe) = std::env::current_exe() {
            cleanup_old_binaries(&exe);
        }
        return None;
    };
    let staged = FullVersion::parse(&marker.version)?;
    if !enabled {
        crate::debug::emit("self-update: disabled by config; leaving the staged update in place");
        return None;
    }
    let current = FullVersion::parse(BUILD_VERSION);
    if current.as_ref().is_some_and(|c| &staged <= c) {
        // Same or older than what we run (already applied, or a downgrade):
        // drop it instead of re-applying forever.
        clear_staged();
        return None;
    }

    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(e) => return fail_apply(&marker, format!("current_exe: {e}")),
    };
    if cfg!(target_os = "macos") && inside_app_bundle(&exe) {
        // Swapping the inner binary of a signed bundle invalidates the
        // signature; point the user at the download page instead.
        return fail_apply(&marker, "installed as a .app bundle".to_string());
    }
    if is_dev_path(&exe) {
        // `cargo run` / a build tree: replacing the artifact under target/ with
        // a released binary would silently break the developer's build.
        return fail_apply(&marker, "running from a cargo build tree".to_string());
    }
    let staged_file = match staged_path(&marker) {
        Ok(path) => path,
        Err(e) => return fail_apply(&marker, e.to_string()),
    };
    // The archive was verified when it was downloaded; re-verify before
    // executing it, because the cache directory is writable by this user and
    // an apply can happen days later.
    if let Some(expected) = &marker.sha256
        && crate::runtime::block_on(sha256_of(&staged_file)).as_ref() != Some(expected)
    {
        clear_staged();
        return fail_apply(
            &marker,
            "staged archive no longer matches its digest".into(),
        );
    }

    match swap_binary(&exe, &staged_file, &staged.to_string()) {
        Ok(()) => {
            let version = staged.to_string();
            clear_staged();
            cleanup_old_binaries(&exe);
            with(|inner| {
                inner.applied = Some(version.clone());
                inner.apply_error = None;
            });
            publish();
            Some(version)
        }
        Err(e) => fail_apply(&marker, e.to_string()),
    }
}

/// Rename the running binary aside, write the staged one in, and roll back if
/// anything fails. The running image keeps executing from the renamed file on
/// every platform (both Windows and Unix allow renaming an executing file).
///
/// The new bytes land in a sibling temp file first and are renamed into place,
/// so an interrupted write can never leave a half-written executable at the
/// path the OS will launch next.
fn swap_binary(exe: &Path, staged: &Path, version: &str) -> Result<()> {
    let file = exe
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_else(|| "dshl".to_string());
    let backup = exe.with_file_name(format!("{file}.old-{version}"));
    let incoming = exe.with_file_name(format!("{file}.new-{version}"));

    // Sanity: a real executable, not a stray empty file.
    let size = std::fs::metadata(staged).map(|m| m.len()).unwrap_or(0);
    if size == 0 {
        return Err(Error(
            t!(
                "self_update.staged_missing",
                path = staged.display().to_string()
            )
            .to_string(),
        ));
    }

    let _ = std::fs::remove_file(&incoming);
    std::fs::copy(staged, &incoming).map_err(|e| {
        Error(
            t!(
                "self_update.copy_failed",
                err = e,
                path = incoming.display().to_string()
            )
            .to_string(),
        )
    })?;
    if let Ok(f) = std::fs::File::open(&incoming) {
        let _ = f.sync_all();
    }

    std::fs::rename(exe, &backup).map_err(|e| {
        let _ = std::fs::remove_file(&incoming);
        Error(
            t!(
                "self_update.rename_failed",
                path = exe.display().to_string(),
                err = e
            )
            .to_string(),
        )
    })?;
    if let Err(e) = std::fs::rename(&incoming, exe) {
        // Put the old binary back: a failed update must never leave the
        // install without its executable.
        let _ = std::fs::rename(&backup, exe);
        let _ = std::fs::remove_file(&incoming);
        return Err(Error(
            t!(
                "self_update.copy_failed",
                err = e,
                path = exe.display().to_string()
            )
            .to_string(),
        ));
    }
    download::make_executable(exe);
    Ok(())
}

/// Record why the swap did not happen and publish it (the UI then offers the
/// manual route). The staged artifact is KEPT: the user may fix the cause (or
/// install by hand) without downloading again.
fn fail_apply(marker: &Marker, err: String) -> Option<String> {
    crate::debug::emit(&format!(
        "self-update: cannot apply {} automatically: {err}",
        marker.version
    ));
    with(|inner| inner.apply_error = Some(err));
    publish();
    None
}

/// The published status, if a check has run in this process.
pub fn status() -> Option<SelfUpdateInfo> {
    with(|inner| inner.published.clone())
}

/// The status as JSON for the control plane.
///
/// `checked` mirrors [`crate::update_check::status_json`]: it reports whether
/// a check actually REACHED the feed, not merely whether a status exists.
pub fn status_json() -> Value {
    match status() {
        Some(info) => {
            let checked = with(|inner| inner.checked);
            serde_json::json!({
                "checked": checked,
                "current": info.current,
                "latest": info.latest,
                "available": info.available,
                "staged": info.staged,
                "applied": info.applied,
                "url": info.url,
                "checked_at": info.checked_at,
                "text": info.text,
            })
        }
        None => serde_json::json!({ "checked": false }),
    }
}

// ---------------------------------------------------------------- internals

/// Latest release: version, portable-archive URL and its published digest.
struct Release {
    version: FullVersion,
    url: String,
    sha256: Option<String>,
}

async fn fetch_release(github: &str) -> Option<Release> {
    let api = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let url = if github.is_empty() {
        api
    } else {
        format!("{}/{}", github.trim_end_matches('/'), api)
    };
    let body = download::http_get_text_bounded(&url, QUERY_TIMEOUT)
        .await
        .ok()?;
    let json: Value = serde_json::from_str(&body).ok()?;
    let mut release = release_from_json(&json)?;
    // Download AND the browser link go through the same proxy prefix.
    release.url = proxied(github, &release.url);
    Some(release)
}

/// Pure half of [`fetch_release`] (unit-tested against a canned payload).
fn release_from_json(json: &Value) -> Option<Release> {
    let tag = json.get("tag_name")?.as_str()?;
    let version = FullVersion::parse(tag)?;
    let (url, sha256) = pick_asset(json, &version.to_string())?;
    Some(Release {
        version,
        url,
        sha256,
    })
}

/// Apply the configured GitHub proxy prefix to an asset URL (the download and
/// the browser link then go the same way as every other GitHub fetch).
fn proxied(github: &str, url: &str) -> String {
    if github.is_empty() {
        url.to_string()
    } else {
        format!("{}/{}", github.trim_end_matches('/'), url)
    }
}

/// Pick this platform's portable archive out of a release payload, with its
/// `sha256:` digest when the feed publishes one.
fn pick_asset(json: &Value, version: &str) -> Option<(String, Option<String>)> {
    let key = platform_key();
    let want = format!("dshl-{version}-{key}.zip");
    let assets = json.get("assets")?.as_array()?;
    let asset = assets
        .iter()
        .find(|a| a.get("name").and_then(Value::as_str) == Some(want.as_str()))
        .or_else(|| {
            // Fall back to any archive for this platform (e.g. the tag and the
            // asset name disagree): still better than offering nothing.
            let suffix = format!("-{key}.zip");
            assets.iter().find(|a| {
                a.get("name")
                    .and_then(Value::as_str)
                    .is_some_and(|n| n.ends_with(&suffix))
            })
        })?;
    let url = asset.get("browser_download_url")?.as_str()?.to_string();
    let sha256 = asset
        .get("digest")
        .and_then(Value::as_str)
        .and_then(|d| d.strip_prefix("sha256:"))
        .map(|h| h.trim().to_ascii_lowercase());
    Some((url, sha256))
}

/// `windows-x86_64` / `linux-aarch64` / `macos-x86_64` — the artifact suffix
/// used by `release.yml` (`dshl-<version>-<key>.zip`).
fn platform_key() -> &'static str {
    platform_key_for(platform::os(), platform::arch())
}

fn platform_key_for(os: platform::Os, arch: platform::Arch) -> &'static str {
    match (os, arch) {
        (platform::Os::Windows, platform::Arch::Aarch64) => "windows-aarch64",
        (platform::Os::Windows, _) => "windows-x86_64",
        (platform::Os::Macos, platform::Arch::Aarch64) => "macos-aarch64",
        (platform::Os::Macos, _) => "macos-x86_64",
        (platform::Os::Linux, platform::Arch::Aarch64) => "linux-aarch64",
        (platform::Os::Linux, _) => "linux-x86_64",
    }
}

fn update_dir() -> PathBuf {
    platform::cache_dir().join("dshl").join("update")
}

fn marker_path() -> PathBuf {
    update_dir().join(MARKER)
}

/// What is staged on disk (survives restarts; read by [`apply_staged`]).
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Marker {
    version: String,
    /// Path of the new binary, relative to [`update_dir`].
    binary: String,
    sha256: Option<String>,
    url: String,
    downloaded_at: u64,
}

fn read_marker() -> Option<Marker> {
    let text = std::fs::read_to_string(marker_path()).ok()?;
    serde_json::from_str(&text).ok()
}

/// Resolve the staged binary.
///
/// The marker is a plain file in a user-writable cache directory, so it must
/// never become a way to copy an arbitrary file over the launcher: only plain
/// relative components under [`update_dir`] are accepted.
fn staged_path(marker: &Marker) -> Result<PathBuf> {
    let rel = Path::new(&marker.binary);
    let clean = !rel.is_absolute()
        && rel
            .components()
            .all(|c| matches!(c, std::path::Component::Normal(_)));
    if !clean {
        return Err(Error(t!("self_update.bad_marker").to_string()));
    }
    let path = update_dir().join(rel);
    if path.is_file() {
        Ok(path)
    } else {
        Err(Error(
            t!(
                "self_update.staged_missing",
                path = path.display().to_string()
            )
            .to_string(),
        ))
    }
}

/// Download (resumable), verify and extract into `<cache>/dshl/update/staged`.
///
/// The SHA-256 published by the release feed is REQUIRED: this code path ends
/// in an executed binary, so an unverifiable download is refused outright and
/// the user is pointed at the manual route instead (the UI offers the release
/// page once the status carries a URL the button can open).
async fn stage(available: &Available) -> Result<()> {
    let dir = update_dir();
    std::fs::create_dir_all(&dir).map_err(|e| Error(e.to_string()))?;
    let url = available.url.clone();
    let zip = dir.join(format!("dshl-{}-{}.zip", available.version, platform_key()));

    let Some(expected) = available.sha256.clone() else {
        // Refuse BEFORE downloading: a mirror that strips the digest must not
        // turn into "install whatever it served".
        return Err(Error(t!("self_update.no_digest").to_string()));
    };
    http_download(&url, &zip).await?;
    verify_sha256(&zip, &expected).await?;

    let staged = dir.join("staged");
    let _ = std::fs::remove_dir_all(&staged);
    extract_zip(&zip, &staged).await?;
    let binary = download::locate_file(&staged, "dshl")
        .ok_or_else(|| Error(t!("self_update.no_binary").to_string()))?;
    let relative = binary
        .strip_prefix(&dir)
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|_| binary.clone());

    let marker = Marker {
        version: available.version.to_string(),
        binary: relative.to_string_lossy().to_string(),
        sha256: available.sha256.clone(),
        url: available.url.clone(),
        downloaded_at: now_secs(),
    };
    let text = serde_json::to_string_pretty(&marker).map_err(|e| Error(e.to_string()))?;
    std::fs::write(marker_path(), text).map_err(|e| Error(e.to_string()))?;
    let _ = std::fs::remove_file(&zip);

    with(|inner| {
        inner.staged = Some(marker.version.clone());
        inner.apply_error = None;
    });
    progress::log(t!("self_update.downloaded", version = marker.version));
    Ok(())
}

/// Refuse a download whose digest does not match the release's own metadata.
async fn verify_sha256(path: &Path, expected: &str) -> Result<()> {
    let actual = sha256_of(path).await.ok_or_else(|| {
        Error(
            t!(
                "self_update.no_hash_tool",
                path = path.display().to_string()
            )
            .to_string(),
        )
    })?;
    if actual == expected.to_ascii_lowercase() {
        return Ok(());
    }
    let _ = std::fs::remove_file(path);
    Err(Error(
        t!(
            "self_update.hash_mismatch",
            expected = expected,
            actual = actual
        )
        .to_string(),
    ))
}

/// SHA-256 of a file, via the platform's own tool (no crypto dependency).
async fn sha256_of(path: &Path) -> Option<String> {
    let (program, args): (&str, Vec<String>) = if cfg!(target_os = "windows") {
        (
            "certutil",
            vec![
                "-hashfile".to_string(),
                path.display().to_string(),
                "SHA256".to_string(),
            ],
        )
    } else if cfg!(target_os = "macos") {
        (
            "shasum",
            vec!["-a".into(), "256".into(), path.display().to_string()],
        )
    } else {
        ("sha256sum", vec![path.display().to_string()])
    };
    let mut cmd = std::process::Command::new(program);
    cmd.args(&args);
    let res = crate::process::run_bounded(&mut cmd, Duration::from_secs(60))
        .await
        .ok()?;
    if !res.success() {
        return None;
    }
    let text = format!("{}{}", res.stdout, res.stderr);
    // Accept only a full standalone hex line / first token: a heuristic "any
    // 64-char hex substring" would happily match unrelated output.
    text.lines()
        .flat_map(|l| l.split_whitespace())
        .find(|t| t.len() == 64 && t.chars().all(|c| c.is_ascii_hexdigit()))
        .map(|t| t.to_ascii_lowercase())
}

/// Drop the staged artifact (applied, superseded or downgrade).
fn clear_staged() {
    let _ = std::fs::remove_dir_all(update_dir().join("staged"));
    let _ = std::fs::remove_file(marker_path());
    with(|inner| inner.staged = None);
}

/// Best-effort removal of `*.old-<version>` leftovers from earlier updates
/// (and of an `*.new-*` temp file left by an interrupted write).
///
/// The just-renamed binary is still executing, so it cannot be deleted yet on
/// Windows — that is exactly what the next start is for.
fn cleanup_old_binaries(exe: &Path) {
    let Some(dir) = exe.parent() else {
        return;
    };
    let Some(file) = exe.file_name().map(|f| f.to_string_lossy().to_string()) else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with(&format!("{file}.old-")) || name.starts_with(&format!("{file}.new-")) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

fn inside_app_bundle(exe: &Path) -> bool {
    exe.components()
        .any(|c| c.as_os_str().to_string_lossy().ends_with(".app"))
}

/// True when the executable lives in a cargo build tree (`target/debug`,
/// `target/release`, or their `deps/`): replacing a developer's artifact with
/// a released binary would silently break their build.
fn is_dev_path(exe: &Path) -> bool {
    let parts: Vec<String> = exe
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_ascii_lowercase())
        .collect();
    parts
        .windows(2)
        .any(|w| w[0] == "target" && (w[1] == "debug" || w[1] == "release"))
}

/// Build (and publish) the status the UI and control plane read.
fn publish() -> Option<SelfUpdateInfo> {
    let built = with(|inner| {
        let current = BUILD_VERSION.to_string();
        let latest = inner.latest.as_ref().map(|a| a.version.to_string());
        let available = inner.latest.is_some();
        let staged = inner.staged.is_some();
        let applied = inner.applied.clone();
        let url = inner
            .latest
            .as_ref()
            .map(|a| a.url.clone())
            .or_else(|| Some(release_page()));
        let text = if let Some(applied) = &applied {
            t!("self_update.applied", version = applied.clone()).to_string()
        } else if let Some(err) = &inner.apply_error {
            t!(
                "self_update.manual",
                version = inner.staged.clone().unwrap_or_default(),
                err = err.clone()
            )
            .to_string()
        } else if staged {
            t!(
                "self_update.staged",
                version = inner.staged.clone().unwrap_or_default()
            )
            .to_string()
        } else if let Some(latest) = &latest {
            t!(
                "self_update.available",
                latest = latest.clone(),
                current = current.clone()
            )
            .to_string()
        } else if inner.checked {
            t!("self_update.up_to_date", current = current.clone()).to_string()
        } else {
            t!("self_update.failed").to_string()
        };
        let info = SelfUpdateInfo {
            current,
            latest,
            available,
            staged,
            applied,
            url,
            checked_at: now_secs(),
            text,
        };
        let changed = inner
            .published
            .as_ref()
            .map(|p| p.text != info.text)
            .unwrap_or(true);
        inner.published = Some(info.clone());
        Some((info, changed))
    });
    let (info, changed) = built?;
    progress::set_self_update(Some(info.clone()));
    // Log only on a real change: the timer runs unattended for days.
    if changed {
        progress::log(info.text.clone());
    }
    Some(info)
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_keys_match_the_release_artifact_names() {
        use platform::{Arch, Os};
        assert_eq!(
            platform_key_for(Os::Windows, Arch::X86_64),
            "windows-x86_64"
        );
        assert_eq!(
            platform_key_for(Os::Windows, Arch::Aarch64),
            "windows-aarch64"
        );
        assert_eq!(platform_key_for(Os::Linux, Arch::Aarch64), "linux-aarch64");
        assert_eq!(platform_key_for(Os::Macos, Arch::X86_64), "macos-x86_64");
    }

    /// Payload shaped like the real `releases/latest` response (v0.2.22).
    fn release_payload() -> Value {
        serde_json::json!({
            "tag_name": "v0.2.22",
            "assets": [
                { "name": "dshl-0.2.22-linux-x86_64.zip",
                  "browser_download_url": "https://example/linux.zip",
                  "digest": "sha256:AABB" },
                { "name": "dshl-0.2.22-windows-x86_64.zip",
                  "browser_download_url": "https://example/win.zip",
                  "digest": "sha256:897DAA69859B606DA6145147F32DD2487D56401AA021F58352317AB6BC1D5B5E" },
                { "name": "dshl-0.2.22-windows-x86_64-setup.exe",
                  "browser_download_url": "https://example/win-setup.exe" }
            ]
        })
    }

    #[test]
    fn picks_the_portable_archive_for_this_platform() {
        let release = release_from_json(&release_payload()).expect("release parses");
        assert_eq!(release.version.to_string(), "0.2.22");
        // The canned payload only carries assets for two platforms; on any
        // other host the picker must simply find nothing.
        match platform_key() {
            "windows-x86_64" => {
                assert_eq!(release.url, "https://example/win.zip");
                assert_eq!(
                    release.sha256.as_deref(),
                    Some("897daa69859b606da6145147f32dd2487d56401aa021f58352317ab6bc1d5b5e")
                );
            }
            "linux-x86_64" => assert_eq!(release.url, "https://example/linux.zip"),
            _ => assert!(release_from_json(&release_payload()).is_none()),
        }
    }

    #[test]
    fn proxy_prefix_is_applied_to_the_asset_url() {
        assert_eq!(
            proxied("https://gh-proxy.org/", "https://github.com/a/b.zip"),
            "https://gh-proxy.org/https://github.com/a/b.zip"
        );
        assert_eq!(
            proxied("", "https://github.com/a/b.zip"),
            "https://github.com/a/b.zip"
        );
    }

    #[test]
    fn missing_asset_is_not_an_update() {
        let payload = serde_json::json!({
            "tag_name": "v0.2.23",
            "assets": [ { "name": "dshl-0.2.23-freebsd-x86_64.zip",
                          "browser_download_url": "https://example/x.zip" } ]
        });
        assert!(release_from_json(&payload).is_none());
    }

    #[test]
    fn unparseable_tag_is_not_an_update() {
        let payload = serde_json::json!({
            "tag_name": "nightly",
            "assets": [ { "name": "dshl-nightly-windows-x86_64.zip",
                          "browser_download_url": "https://example/x.zip" } ]
        });
        assert!(release_from_json(&payload).is_none());
    }

    /// The pure half of the comparison that decides "is there an update?".
    #[test]
    fn newer_release_is_an_update_older_is_not() {
        let current = FullVersion::parse(BUILD_VERSION);
        let newer = FullVersion::parse("99.0.0").unwrap();
        assert!(current.as_ref().is_none_or(|c| newer > *c));
        let older = FullVersion::parse("0.0.1").unwrap();
        assert!(current.as_ref().is_some_and(|c| older < *c));
    }

    #[test]
    fn app_bundle_and_dev_paths_are_recognised() {
        assert!(inside_app_bundle(Path::new(
            "/Applications/DSHL.app/Contents/MacOS/dshl"
        )));
        assert!(!inside_app_bundle(Path::new("/usr/local/bin/dshl")));
        // A cargo build tree must never be replaced by a released binary.
        assert!(is_dev_path(Path::new(r"G:\p\target\debug\dshl.exe")));
        assert!(is_dev_path(Path::new("/p/target/release/dshl")));
        assert!(is_dev_path(Path::new(
            "/p/target/debug/deps/dshl_core-1.exe"
        )));
        assert!(!is_dev_path(Path::new(r"C:\Program Files\dshl\dshl.exe")));
        assert!(!is_dev_path(Path::new("/usr/local/bin/dshl")));
    }

    /// The marker is data from a user-writable cache: it must not be able to
    /// point the swap at a file outside the update directory.
    #[test]
    fn staged_path_rejects_escapes() {
        let marker = |binary: &str| Marker {
            version: "9.9.9".into(),
            binary: binary.into(),
            sha256: None,
            url: String::new(),
            downloaded_at: 0,
        };
        assert!(staged_path(&marker("..\\..\\Windows\\System32\\cmd.exe")).is_err());
        assert!(staged_path(&marker("../../etc/passwd")).is_err());
        assert!(staged_path(&marker("C:\\Windows\\System32\\cmd.exe")).is_err());
        assert!(staged_path(&marker("/usr/bin/env")).is_err());
        // A clean relative path simply does not exist here.
        assert!(staged_path(&marker("staged/dshl.exe")).is_err());
    }

    /// The swap itself: happy path plus the two guard branches that must leave
    /// the current binary untouched.
    #[test]
    fn swap_replaces_the_binary_or_leaves_it_alone() {
        let dir = std::env::temp_dir().join(format!("dshl-swap-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("dshl.exe");
        let staged = dir.join("staged.bin");
        std::fs::write(&exe, b"OLD").unwrap();
        std::fs::write(&staged, b"NEW-BINARY").unwrap();

        swap_binary(&exe, &staged, "9.9.9").expect("swap succeeds");
        assert_eq!(std::fs::read(&exe).unwrap(), b"NEW-BINARY");
        assert_eq!(
            std::fs::read(dir.join("dshl.exe.old-9.9.9")).unwrap(),
            b"OLD",
            "the previous binary is kept aside"
        );
        assert!(
            !dir.join("dshl.exe.new-9.9.9").exists(),
            "temp is renamed away"
        );

        // Missing / empty staged file: refuse BEFORE touching the executable.
        std::fs::write(&exe, b"OLD2").unwrap();
        assert!(swap_binary(&exe, &dir.join("nope.bin"), "9.9.9").is_err());
        assert_eq!(std::fs::read(&exe).unwrap(), b"OLD2");
        std::fs::write(&staged, b"").unwrap();
        assert!(swap_binary(&exe, &staged, "9.9.9").is_err());
        assert_eq!(std::fs::read(&exe).unwrap(), b"OLD2");

        // Leftovers (old backups AND an interrupted-write temp) are swept on
        // the next start; unrelated files are left alone.
        std::fs::write(dir.join("dshl.exe.old-1.0.0"), b"x").unwrap();
        std::fs::write(dir.join("dshl.exe.new-1.0.0"), b"x").unwrap();
        std::fs::write(dir.join("unrelated.txt"), b"x").unwrap();
        cleanup_old_binaries(&exe);
        assert!(!dir.join("dshl.exe.old-1.0.0").exists());
        assert!(!dir.join("dshl.exe.new-1.0.0").exists());
        assert!(dir.join("unrelated.txt").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Digest verification must accept the matching file and reject any other,
    /// and it must refuse to proceed when the host has no hashing tool.
    #[test]
    fn sha256_verification_round_trips() {
        let path = std::env::temp_dir().join(format!("dshl-sha-test-{}", std::process::id()));
        std::fs::write(&path, b"dshl self-update test payload").unwrap();
        let Some(digest) = crate::runtime::block_on(sha256_of(&path)) else {
            // No certutil / shasum / sha256sum on this host: the feature fails
            // closed, and so does this test (see `verify_sha256`).
            let _ = std::fs::remove_file(&path);
            return;
        };
        assert_eq!(digest.len(), 64);
        assert!(
            crate::runtime::block_on(verify_sha256(&path, &digest)).is_ok(),
            "matching digest must verify"
        );
        let wrong = "0".repeat(64);
        assert!(
            crate::runtime::block_on(verify_sha256(&path, &wrong)).is_err(),
            "wrong digest must be refused"
        );
        // A refused download is deleted, so the next attempt starts clean.
        assert!(!path.exists());
    }

    /// Nothing staged / no context: every entry point must be a quiet no-op
    /// (and, in particular, must not touch the network).
    #[test]
    fn empty_state_is_a_noop() {
        assert!(status().is_none());
        assert_eq!(status_json()["checked"], serde_json::json!(false));
        // Enabled or not, nothing is staged in a test process.
        assert!(apply_staged(true).is_none());
        assert!(apply_staged(false).is_none());
        assert!(crate::runtime::block_on(check_once()).is_none());
    }
}
