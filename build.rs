//! Build script: pin the launcher's own version at compile time.
//!
//! `CARGO_PKG_VERSION` is hand-edited (`Cargo.toml`) while releases are cut
//! from git tags, so the two drifted apart (the repo shipped `v0.2.22`
//! artifacts whose `dshl --version` answered `0.2.0`). That is merely
//! cosmetic until the launcher has to compare itself against a published
//! release — a self-update check would then offer the SAME release it is
//! already running, forever.
//!
//! Resolution order:
//! 1. `DSHL_VERSION` (set by `release.yml` from the tag) — authoritative;
//! 2. `git describe --tags --abbrev=0` — the nearest tag, for local builds;
//! 3. `CARGO_PKG_VERSION` — packaged builds without `.git`.
//!
//! The value is exposed as [`dshl_core::version::BUILD_VERSION`].

use std::path::Path;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=DSHL_VERSION");
    // A new tag must invalidate the cached value, so watch the refs.
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/refs/tags");

    let version = std::env::var("DSHL_VERSION")
        .ok()
        .map(|v| v.trim().trim_start_matches('v').to_string())
        .filter(|v| looks_like_version(v))
        .or_else(from_git_tag)
        .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string());

    println!("cargo:rustc-env=DSHL_BUILD_VERSION={version}");
}

/// `X.Y.Z` (optionally with a pre-release suffix) — the shape the rest of the
/// code parses with `FullVersion::parse`.
fn looks_like_version(v: &str) -> bool {
    let core = v.split(['-', '+']).next().unwrap_or("");
    let parts: Vec<&str> = core.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
}

/// Nearest git tag, e.g. `v0.2.22` → `0.2.22`. `None` outside a git checkout
/// (packaged sources) or when the repo has no tags yet.
fn from_git_tag() -> Option<String> {
    let dir = std::env::var("CARGO_MANIFEST_DIR").ok()?;
    if !Path::new(&dir).join(".git").exists() {
        return None;
    }
    let out = Command::new("git")
        .args(["describe", "--tags", "--abbrev=0"])
        .current_dir(&dir)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let tag = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let version = tag.trim_start_matches('v').to_string();
    looks_like_version(&version).then_some(version)
}
