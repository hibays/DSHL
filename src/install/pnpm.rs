//! pnpm: [`ensure_pnpm`] and global-bin-dir resolution.
//!
//! pnpm is installed through `npm install --prefix <cache>/dshl/pnpm` when
//! missing — node (and with it npm) is always present at that point. The bin
//! directory of the cache install is returned so the caller can prepend it to
//! the runtime PATH; a freshly installed pnpm is not on the ambient PATH.

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

/// Ensure pnpm is installed when the config requires it (`pm = "pnpm"`).
///
/// pnpm is never `-g` installed: when missing it is installed into dshl's own
/// cache (`<cache>/dshl/pnpm`) via `npm install --prefix`, and the resulting
/// bin dir is returned so the caller can prepend it to the runtime PATH.
pub async fn ensure_pnpm(
    config: &Config,
    mirror: &MirrorConfig,
    node_dir: &Path,
) -> Result<Vec<PathBuf>> {
    if !config.dsh.needs_pnpm() {
        return Ok(Vec::new());
    }

    let prefix = crate::platform::cache_dir().join("dshl").join("pnpm");
    let cached_bin = prefix.join("node_modules").join(".bin");

    // Prefer the user's own global pnpm, then a previous cache install.
    let pnpm = probe::pnpm().await;
    if pnpm.found {
        match pnpm.version {
            Some(v) => progress::log(t!("install.pnpm.satisfies", v = v)),
            None => {
                let path = pnpm
                    .path
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default();
                progress::log(t!("install.pnpm.found", path = path));
            }
        }
        return Ok(pnpm_bin_dirs(node_dir).await);
    }
    if cached_bin.join(platform::with_ext("pnpm")).is_file() {
        progress::log(t!("install.pnpm.cached", dir = cached_bin.display()));
        return Ok(vec![cached_bin]);
    }

    progress::log(t!("install.pnpm.not_found"));
    // Install pnpm into dshl's cache (never `-g`). The npm of a freshly
    // installed fnm node lives in `node_dir`, which is not on the ambient
    // PATH — augment it so the install works.
    std::fs::create_dir_all(&prefix).ok();
    let mut cmd = Command::new(platform::tool("npm"));
    cmd.args(["install", "--prefix"]);
    cmd.arg(&prefix);
    cmd.args(["--no-save", "pnpm"]);
    let rt = Runtime {
        node_dir: Some(node_dir.to_path_buf()),
        bun_dir: None,
        extra_path: Vec::new(),
    };
    cmd.env("PATH", rt.augmented_path());
    process::with_env(&mut cmd, &mirror.npm_env());
    run_streaming(cmd, "install pnpm").await?;

    if cached_bin.join(platform::with_ext("pnpm")).is_file() {
        return Ok(vec![cached_bin]);
    }
    // Fall back to global-bin resolution so a pnpm that npm placed elsewhere
    // is still found.
    Ok(pnpm_bin_dirs(node_dir).await)
}

/// Resolve pnpm's global bin directory (where `pnpm add -g` links bins).
///
/// `pnpm bin -g` is authoritative, but on machines where that directory is
/// not on PATH it exits non-zero and prints an error that still quotes the
/// configured path. Fallback chain: printed line → path quoted in the error
/// → platform default. Directories that do not exist yet are created so a
/// freshly installed pnpm is findable right after the first `add -g`.
async fn pnpm_bin_dirs(node_dir: &Path) -> Vec<PathBuf> {
    let rt = Runtime {
        node_dir: Some(node_dir.to_path_buf()),
        bun_dir: None,
        extra_path: Vec::new(),
    };
    let mut cmd = Command::new(platform::tool("pnpm"));
    cmd.args(["bin", "-g"]);
    cmd.env("PATH", rt.augmented_path());
    let text = process::run_async(&mut cmd)
        .await
        .map(|res| format!("{}{}", res.stdout, res.stderr))
        .unwrap_or_default();

    let mut dirs: Vec<PathBuf> = Vec::new();
    let push_dir = |dirs: &mut Vec<PathBuf>, dir: PathBuf| {
        if dirs.iter().all(|d| d != &dir) {
            let _ = std::fs::create_dir_all(&dir);
            dirs.push(dir);
        }
    };

    // 1. Success: the whole first line is the path.
    let first = text
        .lines()
        .next()
        .unwrap_or("")
        .trim()
        .trim_matches('"')
        .trim();
    if looks_like_dir(first) {
        push_dir(&mut dirs, PathBuf::from(first));
    }
    // 2. Error output still quotes the configured path, e.g.
    //    [ERROR] The configured global bin directory "C:..." is not in PATH
    if dirs.is_empty()
        && let Some(start) = text.find('"')
        && let Some(rel) = text[start + 1..].find('"')
    {
        let quoted = text[start + 1..start + 1 + rel].trim();
        if looks_like_dir(quoted) {
            push_dir(&mut dirs, PathBuf::from(quoted));
        }
    }
    // 3. Platform defaults (both the pnpm 10 and pnpm 11 Windows layouts).
    if cfg!(target_os = "windows")
        && let Some(base) = std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
    {
        push_dir(&mut dirs, base.join("pnpm").join("bin"));
        push_dir(&mut dirs, base.join("pnpm"));
    } else {
        let home = platform::home_dir().unwrap_or_default();
        push_dir(&mut dirs, home.join(".local").join("share").join("pnpm"));
    }
    dirs
}

/// A crude "this looks like an absolute path" check used to pick the pnpm
/// bin dir out of command output.
fn looks_like_dir(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    if cfg!(target_os = "windows") {
        let b = s.as_bytes();
        (b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':')
            || s.starts_with("\\")
            || s.starts_with('/')
    } else {
        s.starts_with('/') || s.starts_with("~")
    }
}
