//! nub: [`ensure_nub`] — install the all-in-one Node toolkit into dshl's cache.
//!
//! nub (npm: `@nubjs/nub`, a Rust binary + platform N-API addons) replaces the
//! package manager, `npx` and Node provisioning in one binary. It is installed
//! through `npm install --prefix <cache>/dshl/nub` when missing — node (and
//! with it npm) is always present at that point, and the npm registry honors
//! `mirrors.npm`, so the download is mirrorable like every other fetch. The
//! bin directory of the cache install is returned so the caller can prepend it
//! to the runtime PATH; a freshly installed nub is not on the ambient PATH.
//!
//! nub can also provision Node itself (`nub node install|which`) — that tier
//! is intentionally NOT wired into [`super::node`]'s fallback chain yet; the
//! proven fnm → cargo → nvm chain stays authoritative until the nub-managed
//! layout is validated on real machines.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::config::Config;
use crate::error::Result;
use crate::mirror::MirrorConfig;
use crate::platform;
use crate::probe;
use crate::process;
use crate::progress;

use super::runtime::Runtime;
use super::stream::run_streaming;

/// Ensure nub is installed when the config requires it (`pm = "nub"`).
///
/// Never `-g` installed: when missing it is installed into dshl's own cache
/// (`<cache>/dshl/nub`) via `npm install --prefix`, and the resulting bin dir
/// is returned so the caller can prepend it to the runtime PATH. The npm
/// registry (and therefore this install) honors the configured mirror.
pub async fn ensure_nub(
    config: &Config,
    mirror: &MirrorConfig,
    node_dir: Option<&Path>,
) -> Result<Vec<PathBuf>> {
    if !config.dsh.needs_nub() {
        return Ok(Vec::new());
    }

    let prefix = crate::platform::cache_dir().join("dshl").join("nub");
    let cached_bin = prefix.join("node_modules").join(".bin");

    // Prefer the user's own global nub, then a previous cache install.
    let nub = probe::nub().await;
    if nub.found {
        match nub.version {
            Some(v) => progress::log(t!("install.nub.satisfies", v = v)),
            None => {
                let path = nub
                    .path
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default();
                progress::log(t!("install.nub.found", path = path));
            }
        }
        // Global nub is already on PATH — nothing to prepend.
        return Ok(Vec::new());
    }
    if cached_bin.join(platform::with_ext("nub")).is_file() {
        progress::log(t!("install.nub.cached", dir = cached_bin.display()));
        return Ok(vec![cached_bin]);
    }

    progress::log(t!("install.nub.not_found"));
    // Install @nubjs/nub into dshl's cache (never `-g`). The npm of a freshly
    // installed fnm node lives in `node_dir`, which is not on the ambient
    // PATH — augment it so the install works.
    std::fs::create_dir_all(&prefix).ok();
    let mut cmd = Command::new(platform::tool("npm"));
    cmd.args(["install", "--prefix"]);
    cmd.arg(&prefix);
    cmd.args(["--no-save", "@nubjs/nub"]);
    // nub 自身的下载/注册表操作吃 npm 与 Node 发行版镜像（见 mirror::nub_env）。
    for (k, v) in mirror.nub_env() {
        cmd.env(k, v);
    }
    let rt = Runtime {
        node_dir: node_dir.map(|p| p.to_path_buf()),
        bun_dir: None,
        extra_path: Vec::new(),
    };
    cmd.env("PATH", rt.augmented_path());
    process::with_env(&mut cmd, &mirror.npm_env());
    run_streaming(cmd, "install nub").await?;

    if cached_bin.join(platform::with_ext("nub")).is_file() {
        return Ok(vec![cached_bin]);
    }
    Ok(Vec::new())
}

/// Provision a Node toolchain through nub and return its bin directory.
///
/// Requires an already-runnable `nub` on PATH (probe first — without any Node
/// there is no way to bootstrap it). Downloads honor `NODEJS_ORG_MIRROR`
/// (wired from `mirrors.nodejs_release`) plus the npm registry env. Returns
/// `None` when nub is absent or provisioning fails so the caller can fall
/// back to the fnm/cargo/nvm chain.
pub async fn provision_node(mirror: &MirrorConfig, version: &str) -> Option<PathBuf> {
    if !probe::nub().await.found {
        return None;
    }
    let rt = Runtime {
        node_dir: None,
        bun_dir: None,
        extra_path: Vec::new(),
    };

    // 1) Provision into nub's cache (idempotent for an existing pin).
    let mut install_cmd = Command::new(platform::tool("nub"));
    install_cmd.args(["node", "install"]);
    install_cmd.arg(version);
    install_cmd.env("PATH", rt.augmented_path());
    for (k, v) in mirror.nub_env() {
        install_cmd.env(k, v);
    }
    // Best-effort: an already-provisioned version exits non-zero on some
    // releases; `nub node which` below is the authoritative check.
    let _ = process::run_async(&mut install_cmd).await;

    // 2) Ask nub for the resolved binary.
    let mut which = Command::new(platform::tool("nub"));
    which.args(["node", "which"]);
    which.env("PATH", rt.augmented_path());
    for (k, v) in mirror.nub_env() {
        which.env(k, v);
    }
    let Ok(Ok(res)) = tokio::time::timeout(
        std::time::Duration::from_secs(120),
        process::run_async(&mut which),
    )
    .await
    else {
        return None;
    };
    if !res.success() {
        return None;
    }
    let line = res.stdout.lines().next().unwrap_or("").trim();
    let bin = PathBuf::from(line);
    if !bin.is_file() {
        return None;
    }
    Some(bin.parent().map(|p| p.to_path_buf()).unwrap_or(bin))
}
