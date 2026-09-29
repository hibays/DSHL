//! Bun: [`ensure_bun`] and the install fallback chain.
//!
//! Bun is installed only when the config's `pm` asks for it. Chain:
//! registry-direct tarball (`@oven/bun-<platform>`, same channel as nub —
//! respects the npm mirror, resumable, no npm process) → official install
//! script → npm install into dshl's cache (respects the npm registry
//! mirror). Never installed globally.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::config::Config;
use crate::error::{Error, Result};
use crate::mirror::MirrorConfig;
use crate::platform;
use crate::probe;
use crate::process;
use crate::progress;

use super::BUN_MIN;
use super::bin_in_dir;
use super::download;
use super::runtime::Runtime;
use super::stream::run_streaming;

/// Session-level negative cache (same rationale as nub's): once a full
/// install attempt fails, do NOT retry it on every startup — retrying a
/// doomed install added seconds of perceived launch delay to every boot.
static INSTALL_FAILED: AtomicBool = AtomicBool::new(false);
/// Ensure bun is installed when the config requires it.
///
/// `node_dir` is the node the caller resolved (it may be one dshl installed
/// into its own cache, i.e. NOT on the ambient PATH): the npm fallback below
/// both resolves `npm` through it and prepends it to the child's PATH.
pub async fn ensure_bun(
    config: &Config,
    mirror: &MirrorConfig,
    node_dir: &Path,
) -> Result<Option<PathBuf>> {
    if !config.dsh.needs_bun() {
        return Ok(None);
    }

    let bun = probe::bun().await;
    if bun.found {
        if let Some(v) = bun.version {
            if v >= BUN_MIN {
                let dir = bun
                    .path
                    .as_ref()
                    .and_then(|p| p.parent())
                    .map(|p| p.to_path_buf());
                progress::log(t!("install.bun.satisfies", v = v, min = BUN_MIN));
                return Ok(dir);
            }
            progress::log(t!("install.bun.too_old", v = v, min = BUN_MIN));
        }
    } else {
        progress::log(t!("install.bun.not_found"));
    }

    // Reuse a bun we installed into the cache on an earlier run, even though
    // it is not on the ambient PATH.
    let cache = crate::platform::cache_dir();
    for dir in [
        cache.join("bun").join("bin"),
        cache
            .join("dshl")
            .join("bun-npm")
            .join("node_modules")
            .join(".bin"),
    ] {
        if bin_in_dir(&dir, "bun") {
            progress::log(t!("install.bun.cached", dir = dir.display()));
            return Ok(Some(dir));
        }
    }

    // Arch Linux manages bun with pacman; the user installs it themselves
    // (CLI autonomy) instead of dshl downloading a binary. Checked BEFORE
    // the negative cache so its specific guidance is never swallowed by a
    // generic "install failed" left over from an earlier attempt.
    if platform::distro() == platform::Distro::Arch {
        progress::log(t!("install.bun.arch_pacman"));
        return Err(Error(t!("install.bun.arch_pacman_fatal").to_string()));
    }

    if INSTALL_FAILED.load(Ordering::Relaxed) {
        return Err(Error(t!("install.bun.failed").to_string()));
    }
    match install_bun(mirror, node_dir).await {
        Ok(dir) => Ok(Some(dir)),
        Err(e) => {
            INSTALL_FAILED.store(true, Ordering::Relaxed);
            Err(e)
        }
    }
}

async fn install_bun(mirror: &MirrorConfig, node_dir: &Path) -> Result<PathBuf> {
    let install_dir = platform::cache_dir().join("bun");
    let bin = install_dir.join("bin");
    std::fs::create_dir_all(&install_dir).map_err(|e| Error(e.to_string()))?;

    // 1. Registry-direct tarball (`@oven/bun-<platform>`): honours
    //    `mirrors.npm`, resumable, never spawns npm. Replaces the old
    //    GitHub-release zip download (and its dedicated `bun-download`
    //    mirror route) entirely.
    if install_bun_from_registry(mirror, &install_dir, &bin)
        .await
        .is_ok()
    {
        return Ok(bin);
    }
    progress::log(t!("install.bun.direct_failed"));

    // 2. Official install script.
    let script = if platform::os() == platform::Os::Windows {
        format!(
            "$env:BUN_INSTALL = '{}'; irm bun.sh/install.ps1 | iex",
            install_dir.display()
        )
    } else {
        format!(
            "export BUN_INSTALL='{}'; curl -fsSL https://bun.sh/install | bash",
            install_dir.display()
        )
    };
    let mut cmd = platform::shell_command();
    cmd.arg(script);
    process::with_env(&mut cmd, &mirror.npm_env());
    let _ = run_streaming(cmd, "bun install").await;

    if bin.join(platform::with_ext("bun")).is_file() {
        return Ok(bin);
    }

    // 3. npm fallback into dshl's cache (never `-g`), respects the npm mirror.
    progress::log(t!("install.bun.script_failed"));
    let npm_prefix = crate::platform::cache_dir().join("dshl").join("bun-npm");
    std::fs::create_dir_all(&npm_prefix).ok();
    // Resolve npm through `node_dir` and hand it to the child as PATH: when
    // node came from dshl's cache (fnm install) neither the lookup nor the
    // install would find npm otherwise — this fallback exists precisely for
    // machines where the other tiers failed.
    let node_dirs = [node_dir.to_path_buf()];
    let rt = Runtime {
        node_dir: Some(node_dir.to_path_buf()),
        bun_dir: None,
        extra_path: Vec::new(),
    };
    let mut npm = Command::new(platform::tool_in("npm", &node_dirs));
    npm.args(["install", "--prefix"]);
    npm.arg(&npm_prefix);
    npm.args(["--no-save", "bun"]);
    npm.env("PATH", rt.augmented_path());
    process::with_env(&mut npm, &mirror.npm_env());
    run_streaming(npm, "npm install bun").await?;
    let bin = npm_prefix.join("node_modules").join(".bin");
    // npm writes a `.cmd` shim on Windows, not `bun.exe` — checking only the
    // `.exe` spelling made a successful install look like a failure.
    if bin_in_dir(&bin, "bun") {
        return Ok(bin);
    }

    Err(Error(t!("install.bun.failed").to_string()))
}

/// Install bun by fetching its `@oven/bun-<platform>` binary package
/// straight from the configured registry (npmjs.org or a mirror), exactly
/// like [`super::nub`] does for `@nubjs/nub`. Verified upstream layout:
/// the tarball carries `package/bin/bun[.exe]`.
async fn install_bun_from_registry(
    mirror: &MirrorConfig,
    stage_root: &Path,
    bin: &Path,
) -> Result<()> {
    let pkg = oven_package();
    let base = download::registry_base(mirror);
    let latest =
        download::http_get_text(&format!("{base}/{}/latest", pkg.replace('/', "%2F"))).await?;
    let version = download::extract_json_string(&latest, "version")
        .ok_or_else(|| Error("registry latest response has no version".into()))?;

    let stage = stage_root.join(".stage");
    let _ = std::fs::remove_dir_all(&stage);
    progress::log(t!(
        "install.bun.downloading",
        url = download::package_tgz_url(mirror, pkg, &version)
    ));
    let pkg_dir = download::fetch_package_extracted(mirror, pkg, &version, &stage).await?;
    let found = download::locate_file(&pkg_dir, "bun")
        .ok_or_else(|| Error("registry tarball contains no bun binary".into()))?;
    std::fs::create_dir_all(bin).map_err(|e| Error(e.to_string()))?;
    let dest = bin.join(platform::with_ext("bun"));
    if found != dest {
        std::fs::copy(&found, &dest).map_err(|e| Error(e.to_string()))?;
    }
    download::make_executable(&dest);
    let _ = std::fs::remove_dir_all(&stage);
    if !dest.is_file() {
        return Err(Error("failed to place the bun binary".into()));
    }
    Ok(())
}

/// The @oven platform binary package for this OS/arch — bun's own npm
/// optionalDependencies naming (the `bun` wrapper package pulls these in).
fn oven_package() -> &'static str {
    oven_package_for(platform::os(), platform::arch())
}

/// Pure mapping so the full platform matrix (including Windows ARM64) is
/// unit-testable on any host. `Arch::Other` falls through to x64, matching
/// the pre-registry zip behaviour.
fn oven_package_for(os: platform::Os, arch: platform::Arch) -> &'static str {
    match (os, arch) {
        (platform::Os::Windows, platform::Arch::Aarch64) => "@oven/bun-windows-aarch64",
        (platform::Os::Windows, _) => "@oven/bun-windows-x64",
        (platform::Os::Macos, platform::Arch::Aarch64) => "@oven/bun-darwin-aarch64",
        (platform::Os::Macos, _) => "@oven/bun-darwin-x64",
        (platform::Os::Linux, platform::Arch::Aarch64) => "@oven/bun-linux-aarch64",
        (platform::Os::Linux, _) => "@oven/bun-linux-x64",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oven_package_covers_full_platform_matrix() {
        use platform::{Arch, Os};
        let cases = [
            ((Os::Windows, Arch::X86_64), "@oven/bun-windows-x64"),
            ((Os::Windows, Arch::Aarch64), "@oven/bun-windows-aarch64"),
            ((Os::Windows, Arch::Other), "@oven/bun-windows-x64"),
            ((Os::Macos, Arch::X86_64), "@oven/bun-darwin-x64"),
            ((Os::Macos, Arch::Aarch64), "@oven/bun-darwin-aarch64"),
            ((Os::Linux, Arch::X86_64), "@oven/bun-linux-x64"),
            ((Os::Linux, Arch::Aarch64), "@oven/bun-linux-aarch64"),
        ];
        for ((os, arch), expected) in cases {
            assert_eq!(oven_package_for(os, arch), expected, "{os:?}/{arch:?}");
        }
    }
}
