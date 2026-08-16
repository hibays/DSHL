//! Node.js: [`ensure_node`] and the install fallback chain.
//!
//! Node is always required — dsh runs on it regardless of the chosen `pm` or
//! `exector` — so [`ensure_node`] does not depend on the config. The install
//! chain is: existing fnm → `cargo install fnm` → nvm → best-effort fnm
//! auto-install into `~/.cache/bin` → tell the UI to install fnm manually.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::error::{Error, Result};
use crate::mirror::MirrorConfig;
use crate::platform;
use crate::probe;
use crate::process;
use crate::progress;

use super::download;
use super::stream::run_streaming;
use super::{FNM_GUIDE_URL, NODE_INSTALL_VERSION, NODE_MIN};

/// Ensure Node.js is present and recent enough. Returns the directory that
/// contains the `node` executable.
pub async fn ensure_node(mirror: &MirrorConfig) -> Result<PathBuf> {
    let node = probe::node();
    if node.found {
        if let Some(v) = node.version {
            if v >= NODE_MIN {
                let dir = node
                    .path
                    .as_ref()
                    .and_then(|p| p.parent())
                    .map(|p| p.to_path_buf());
                if let Some(d) = dir {
                    progress::log(t!(
                        "install.node.satisfies",
                        v = v,
                        min = NODE_MIN.to_string()
                    ));
                    return Ok(d);
                }
            } else {
                progress::log(t!(
                    "install.node.too_old",
                    v = v,
                    min = NODE_MIN.to_string(),
                    install = NODE_INSTALL_VERSION
                ));
            }
        } else {
            progress::log(t!("install.node.unparsable"));
        }
    } else {
        progress::log(t!("install.node.not_found"));
    }
    install_node(mirror).await
}

/// Install node `NODE_INSTALL_VERSION` through the fallback chain.
async fn install_node(mirror: &MirrorConfig) -> Result<PathBuf> {
    progress::log(t!("install.node.starting", version = NODE_INSTALL_VERSION));

    // 1. fnm already present
    if let Some(fnm) = platform::which("fnm") {
        progress::log(t!(
            "install.node.using_fnm",
            path = fnm.display().to_string()
        ));
        if let Ok(dir) = install_node_with_fnm(&fnm, mirror).await {
            return Ok(dir);
        }
        progress::log(t!("install.node.fnm_failed"));
    }

    // 2. cargo install fnm
    if probe::cargo().found {
        progress::log(t!("install.node.try_cargo"));
        match install_fnm_via_cargo(mirror).await {
            Ok(fnm) => {
                if let Ok(dir) = install_node_with_fnm(&fnm, mirror).await {
                    return Ok(dir);
                }
                progress::log(t!("install.node.cargo_fnm_node_failed"));
            }
            Err(e) => progress::log(t!("install.node.cargo_failed", err = e.to_string())),
        }
    }

    // 3. nvm
    if probe::nvm().found {
        progress::log(t!("install.node.try_nvm"));
        if let Ok(dir) = install_node_with_nvm(mirror).await {
            return Ok(dir);
        }
        progress::log(t!("install.node.nvm_failed"));
    }

    // 4. best-effort auto-install fnm into ~/.cache/bin
    match download::install_fnm_binary(mirror).await {
        Ok(fnm) => {
            progress::log(t!(
                "install.node.auto_fnm_installed",
                path = fnm.display().to_string()
            ));
            if let Ok(dir) = install_node_with_fnm(&fnm, mirror).await {
                return Ok(dir);
            }
        }
        Err(e) => progress::log(t!("install.node.auto_fnm_failed", err = e.to_string())),
    }

    Err(Error(
        t!(
            "install.node.fatal",
            version = NODE_INSTALL_VERSION,
            url = FNM_GUIDE_URL
        )
        .to_string(),
    ))
}

async fn install_node_with_fnm(fnm: &Path, mirror: &MirrorConfig) -> Result<PathBuf> {
    let mut cmd = Command::new(fnm);
    cmd.args(["install", NODE_INSTALL_VERSION]);
    process::with_env(&mut cmd, &mirror.fnm_env());
    run_streaming(cmd, "fnm install").await?;

    if let Some(dir) = find_node_bin() {
        progress::log(t!("install.node.fnm_done", dir = dir.display().to_string()));
        return Ok(dir);
    }
    Err(Error(t!("install.node.fnm_no_dir").to_string()))
}

async fn install_fnm_via_cargo(mirror: &MirrorConfig) -> Result<PathBuf> {
    let mut cmd = Command::new("cargo");
    cmd.args(["install", "fnm"]);
    process::with_env(&mut cmd, &mirror.cargo_env());
    run_streaming(cmd, "cargo install fnm").await?;

    // fnm lands in ~/.cargo/bin
    if let Some(home) = platform::home_dir() {
        let candidate = home
            .join(".cargo")
            .join("bin")
            .join(platform::with_ext("fnm"));
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    if let Some(p) = platform::which("fnm") {
        return Ok(p);
    }
    Err(Error(t!("install.node.cargo_no_fnm").to_string()))
}

async fn install_node_with_nvm(mirror: &MirrorConfig) -> Result<PathBuf> {
    if platform::os() == platform::Os::Windows {
        let mut cmd = Command::new("nvm");
        cmd.args(["install", NODE_INSTALL_VERSION]);
        process::with_env(&mut cmd, &mirror.nvm_env());
        run_streaming(cmd, "nvm install").await?;

        let mut use_cmd = Command::new("nvm");
        use_cmd.args(["use", NODE_INSTALL_VERSION]);
        run_streaming(use_cmd, "nvm use").await?;
    } else {
        // nvm is a shell function; source its script first.
        let nvm_sh = probe::nvm().path.unwrap_or_else(|| {
            platform::home_dir()
                .unwrap_or_default()
                .join(".nvm")
                .join("nvm.sh")
        });
        let script = format!(
            "source '{}' >/dev/null 2>&1 && nvm install {} && nvm use {}",
            nvm_sh.display(),
            NODE_INSTALL_VERSION,
            NODE_INSTALL_VERSION
        );
        let mut cmd = platform::shell_command();
        cmd.arg(script);
        process::with_env(&mut cmd, &mirror.nvm_env());
        run_streaming(cmd, "nvm install").await?;
    }

    if let Some(dir) = find_node_bin() {
        return Ok(dir);
    }
    Err(Error(t!("install.node.nvm_no_dir").to_string()))
}

/// Recursively search for a directory containing `node`/`node.exe` under the
/// fnm/nvm install roots.
fn find_node_bin() -> Option<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Some(home) = platform::home_dir() {
        roots.push(home.join(".fnm").join("node-versions"));
        roots.push(home.join(".nvm").join("versions").join("node"));
    }
    if let Ok(appdata) = std::env::var("APPDATA") {
        let appdata = PathBuf::from(appdata);
        roots.push(appdata.join("fnm").join("node-versions"));
        roots.push(appdata.join("nvm"));
    }
    for root in roots {
        if let Some(dir) = find_node_in(&root, 0) {
            return Some(dir);
        }
    }
    None
}

fn find_node_in(dir: &Path, depth: u32) -> Option<PathBuf> {
    if depth > 6 {
        return None;
    }
    let name = platform::with_ext("node");
    if dir.join(&name).is_file() {
        return Some(dir.to_path_buf());
    }
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir()
            && let Some(found) = find_node_in(&path, depth + 1)
        {
            return Some(found);
        }
    }
    None
}
