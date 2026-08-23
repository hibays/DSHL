//! nub: direct-from-registry installer for the all-in-one Node toolkit.
//!
//! Downloads @nubjs/nub and its platform binary package STRAIGHT from the
//! configured registry (npmjs.org or mirrors.npm) as tarballs - no `npm`
//! process is ever spawned. The native `nub.exe` in the platform package is
//! self-contained (it even provisions Node itself via `nub node install`),
//! so only its `bin/` directory is needed on PATH.
//!
//! Resume support: every download goes through the resumable helper in
//! [`super::download`] (`curl -C -` + retries), which IS the offline story:
//! a dropped connection continues where it stopped instead of restarting.
//!
//! Node provisioning tier: `provision_node` wraps `nub node install|which`
//! so the runtime-env assembly chain can offer a nub-provided toolchain when
//! mirrors are enabled (see flow/runtime_env).

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use crate::config::Config;
use crate::error::{Error, Result};
use crate::mirror::MirrorConfig;
use crate::platform;
use crate::probe;
use crate::process;
use crate::progress;

use super::download::{fetch_package_extracted, http_get_text, registry_base};

const NUB_VERSION_MARKER: &str = "nub.version";
const NUB_PKG: &str = "@nubjs/nub";

/// Session-level negative cache: once an install attempt fails (no npm,
/// unreachable registry, ...) do NOT retry it on every startup - retrying a
/// doomed install added seconds of perceived launch delay to every boot.
/// Cleared implicitly when a later attempt succeeds (writes the marker).
static INSTALL_FAILED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// Platform subpackage for @nubjs/nub (matches its optionalDependencies).
fn platform_package() -> &'static str {
    match platform::os() {
        platform::Os::Windows => {
            if std::env::consts::ARCH == "x86_64" {
                "@nubjs/nub-win32-x64"
            } else {
                "@nubjs/nub-win32-arm64"
            }
        }
        platform::Os::Macos => {
            if std::env::consts::ARCH == "aarch64" {
                "@nubjs/nub-darwin-arm64"
            } else {
                "@nubjs/nub-darwin-x64"
            }
        }
        platform::Os::Linux => {
            if std::env::consts::ARCH == "aarch64" {
                "@nubjs/nub-linux-arm64"
            } else {
                "@nubjs/nub-linux-x64"
            }
        }
    }
}

/// Ensure nub is available when the config requires it (`pm = "nub"`),
/// downloading straight from the configured registry when missing.
///
/// Layout under `<cache>/dshl/nub/`:
/// - `bin/` - platform binaries (nub.exe / nub / nubx) prepended to PATH
/// - `nub.version` - installed-version marker (offline fast path)
///
/// The user's own global nub still wins over our cache copy.
pub async fn ensure_nub(config: &Config, mirror: &MirrorConfig) -> Result<Vec<PathBuf>> {
    if !config.dsh.needs_nub() {
        return Ok(Vec::new());
    }
    // Fast paths first (zero process spawns):
    //   1. previous session-level failure -> skip silently;
    //   2. cached install with marker     -> use as-is.
    let root = crate::platform::cache_dir().join("dshl").join("nub");
    let bin_dir = root.join("bin");
    let exe = bin_dir.join(platform::with_ext("nub"));
    if std::sync::atomic::AtomicBool::load(&INSTALL_FAILED, std::sync::atomic::Ordering::Relaxed) {
        return Ok(Vec::new());
    }
    if exe.is_file() && marker_version(&root).is_some() {
        progress::log(t!("install.nub.cached", dir = root.display().to_string()));
        return Ok(vec![bin_dir]);
    }

    // User's own global nub wins over our cache copy.
    if probe::nub().await.found {
        return Ok(Vec::new());
    }

    if exe.is_file() && marker_version(&root).is_some() {
        progress::log(t!("install.nub.cached", dir = root.display().to_string()));
        return Ok(vec![bin_dir]);
    }

    progress::log(t!("install.nub.not_found"));

    let install_result = async {
        let base = registry_base(mirror);
        let latest_json = http_get_text(&format!("{base}/{}%2Fnub/latest", "nubjs")).await?;
        let version = super::download::extract_json_string(&latest_json, "version")
            .ok_or_else(|| Error("registry latest response has no version".into()))?;

        let stage = root.join(".stage");
        let _ = std::fs::remove_dir_all(&stage);

        // Main package: JS launchers + metadata (kept for future flexibility).
        let main_pkg = fetch_package_extracted(mirror, NUB_PKG, &version, &stage).await?;

        // Platform package: self-contained binaries.
        let plat_pkg = platform_package();
        let plat_dir = fetch_package_extracted(mirror, plat_pkg, &version, &stage).await?;

        // Assemble bin/: platform binaries first (authoritative), then the main
        // package's launcher scripts for anything the exe does not cover.
        let src_bin = locate_dir(&plat_dir, "bin")
            .ok_or_else(|| Error("platform package has no bin dir".into()))?;
        std::fs::create_dir_all(&bin_dir).map_err(|e| Error(e.to_string()))?;
        copy_dir_contents(&src_bin, &bin_dir)?;
        if let Some(js_bin) = locate_dir(&main_pkg, "bin") {
            copy_dir_contents(&js_bin, &bin_dir)?;
        }

        std::fs::write(root.join(NUB_VERSION_MARKER), &version)
            .map_err(|e| Error(e.to_string()))?;
        let _ = std::fs::remove_dir_all(&stage);

        Ok(bin_dir)
    }
    .await;

    // Session-level negative cache: a failed install (no npm, unreachable
    // registry) must not be retried on every startup - that retry loop was
    // adding seconds of perceived launch delay to each boot.
    match install_result {
        Ok(bin_dir) => {
            progress::log(t!(
                "install.nub.cached",
                dir = bin_dir.display().to_string()
            ));
            Ok(vec![bin_dir])
        }
        Err(e) => {
            INSTALL_FAILED.store(true, std::sync::atomic::Ordering::Relaxed);
            Err(e)
        }
    }
}

fn marker_version(root: &Path) -> Option<String> {
    let text = std::fs::read_to_string(root.join(NUB_VERSION_MARKER)).ok()?;
    let trimmed = text.trim().to_string();
    (!trimmed.is_empty()).then_some(trimmed)
}

fn locate_dir(dir: &Path, name: &str) -> Option<PathBuf> {
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        if let Ok(entries) = std::fs::read_dir(&current) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    if path.file_name().and_then(|n| n.to_str()) == Some(name) {
                        return Some(path);
                    }
                    stack.push(path);
                }
            }
        }
    }
    None
}

fn copy_dir_contents(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst).map_err(|e| Error(e.to_string()))?;
    for entry in std::fs::read_dir(src).map_err(|e| Error(e.to_string()))? {
        let entry = entry.map_err(|e| Error(e.to_string()))?;
        if entry
            .file_type()
            .map_err(|e| Error(e.to_string()))?
            .is_file()
        {
            std::fs::copy(entry.path(), dst.join(entry.file_name()))
                .map_err(|e| Error(e.to_string()))?;
        }
    }
    Ok(())
}

/// Provision a Node toolchain through nub and return its bin directory.
///
/// Requires the nub executable path (caller gets it from [`super::ensure_nub`]).
/// Downloads honor `NODEJS_ORG_MIRROR` (wired from `mirrors.nodejs_release`)
/// plus the npm registry env. Returns `None` when provisioning fails so the
/// caller can fall back to the fnm/cargo/nvm chain.
pub async fn provision_node(
    nub_exe: &Path,
    mirror: &MirrorConfig,
    version: &str,
) -> Option<PathBuf> {
    let run = |args: &[&str]| {
        let mut c = Command::new(nub_exe);
        c.args(args);
        for (k, v) in mirror.nub_env() {
            c.env(k, v);
        }
        c
    };

    // 1) Provision into nub's cache (idempotent for an existing pin).
    let mut install_cmd = run(&["node", "install"]);
    install_cmd.arg(version);
    let _ = tokio::time::timeout(
        Duration::from_secs(600),
        process::run_async(&mut install_cmd),
    )
    .await;

    // 2) Ask nub for the resolved binary.
    let mut which = run(&["node", "which"]);
    let Ok(Ok(res)) =
        tokio::time::timeout(Duration::from_secs(120), process::run_async(&mut which)).await
    else {
        return None;
    };
    if !res.success() {
        return None;
    }
    // Skip any banner line ("» node ... (from PATH)") and take the first
    // line pointing at an existing file.
    for line in res.stdout.lines() {
        let p = PathBuf::from(line.trim());
        if p.is_file() {
            return p.parent().map(|d| d.to_path_buf());
        }
    }
    None
}

/// Ensure nub AND ask it to provision `version`, returning the node bin dir.
///
/// `Ok(None)` means provisioning failed - callers fall back to the legacy
/// fnm/cargo/nvm chain. `Err` propagates real errors (network/IO).
pub async fn ensure_nub_with_node(
    config: &Config,
    mirror: &MirrorConfig,
    version: &str,
) -> Option<PathBuf> {
    let dirs = match ensure_nub(config, mirror).await {
        Ok(d) => d,
        Err(e) => {
            crate::progress::log(t!("flow.runtime.nub_failed", err = e.to_string()));
            return None;
        }
    };
    let exe_dir = dirs
        .iter()
        .find(|d| d.join(platform::with_ext("nub")).is_file())
        .cloned()?;
    let exe = exe_dir.join(platform::with_ext("nub"));
    provision_node(&exe, mirror, version).await
}
