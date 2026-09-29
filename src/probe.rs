//! Detection of the tools dshl cares about: existence, path and version.

use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use crate::platform;
use crate::process;
use crate::version::Version;

/// Hard ceiling for a `--version` probe.
///
/// Probe/verification subprocesses are bounded by policy (installs and
/// downloads deliberately are not). Without it a tool that never answers —
/// a wedged shim, a network-mounted binary — stalled the whole startup
/// pipeline, and the 2h update check would accumulate stuck children; the
/// child is killed when the budget expires (see [`process::run_bounded`]).
const PROBE_TIMEOUT: Duration = Duration::from_secs(30);

/// A detected tool.
#[derive(Debug, Clone)]
pub struct Tool {
    pub name: &'static str,
    pub found: bool,
    pub path: Option<PathBuf>,
    pub version: Option<Version>,
    /// Raw version string as printed by the tool.
    pub raw: String,
}

impl Tool {
    pub(crate) fn missing(name: &'static str) -> Self {
        Self {
            name,
            found: false,
            path: None,
            version: None,
            raw: String::new(),
        }
    }

    fn found(name: &'static str, path: PathBuf, raw: String) -> Self {
        let version = Version::parse(&raw);
        Self {
            name,
            found: true,
            path: Some(path),
            version,
            raw,
        }
    }
}

async fn probe_cmd(name: &'static str, version_args: &[&str]) -> Tool {
    probe_cmd_in(name, version_args, &[]).await
}

/// Like `probe_cmd`, but searches `extra_dirs` first (plus the normal
/// locations), so tools that an earlier flow step installed (fnm's node, a
/// fresh pnpm, …) are found even when they are not on the ambient `PATH`.
async fn probe_cmd_in(name: &'static str, version_args: &[&str], extra_dirs: &[PathBuf]) -> Tool {
    let Some(path) = platform::which_in(name, extra_dirs) else {
        return Tool::missing(name);
    };
    probe_path(name, &path, version_args).await
}

/// Probe the program at `path` (already resolved by the caller).
///
/// Exists for callers that choose among several candidates themselves — the
/// global-`dsh` check walks past dshl's own cache copies — so the path that
/// answered `--version` is exactly the path they go on to spawn.
async fn probe_path(name: &'static str, path: &std::path::Path, version_args: &[&str]) -> Tool {
    let mut cmd = Command::new(path);
    cmd.args(version_args);
    match process::run_bounded(&mut cmd, PROBE_TIMEOUT).await {
        Ok(res) => tool_from_result(
            name,
            path.to_path_buf(),
            res.success(),
            res.stdout.trim().to_string(),
            res.stderr.trim().to_string(),
        ),
        // Spawn failure or timeout: the binary exists but is not usable.
        Err(_) => Tool {
            name,
            found: true,
            path: Some(path.to_path_buf()),
            version: None,
            raw: String::new(),
        },
    }
}

/// Classify a `--version` probe result.
///
/// Exit status is authoritative: a NON-ZERO exit means the binary exists but
/// is broken, and its output must NOT be mined for a version - a crashing
/// node shim prints `Node.js v26.7.0` in its stack trace, which used to be
/// extracted as the tool's own version and made a stale global dsh look
/// up-to-date. On success, combined stdout+stderr stays lenient for tools
/// that print their version on stderr.
fn tool_from_result(
    name: &'static str,
    path: PathBuf,
    success: bool,
    stdout: String,
    stderr: String,
) -> Tool {
    if success {
        let raw = format!("{}{}", stdout.trim(), stderr.trim());
        Tool::found(name, path, raw)
    } else {
        Tool {
            name,
            found: true,
            path: Some(path),
            version: None,
            raw: String::new(),
        }
    }
}

pub async fn node() -> Tool {
    probe_cmd("node", &["--version"]).await
}

pub async fn bun() -> Tool {
    probe_cmd("bun", &["--version"]).await
}

pub async fn pnpm() -> Tool {
    probe_cmd("pnpm", &["--version"]).await
}

pub async fn nub() -> Tool {
    probe_cmd("nub", &["--version"]).await
}

pub async fn fnm() -> Tool {
    probe_cmd("fnm", &["--version"]).await
}

pub async fn cargo() -> Tool {
    probe_cmd("cargo", &["--version"]).await
}

/// Probe `dsh` on the ambient PATH.
///
/// Answers "what would a spawn resolve to", which INCLUDES the copy dshl
/// installed into its own cache. Deciding whether the user has a *global* dsh
/// must go through `flow::prepare::probe_user_global_dsh`, which walks past
/// those copies (see `dsh_at` for the probe it uses on the one it picked).
pub async fn dsh() -> Tool {
    probe_cmd("dsh", &["--version"]).await
}

/// Probe `dsh` searching `extra_dirs` first (the runtime prefix: fnm's node
/// bin, pnpm's global bin, …), so a just-installed dsh is found even when
/// its directory is not on the ambient `PATH`.
///
/// Same caveat as [`dsh`]: a cache copy is a valid answer here, so it is not
/// the probe a global/private decision may be based on.
pub async fn dsh_in(extra_dirs: &[PathBuf]) -> Tool {
    probe_cmd_in("dsh", &["--version"], extra_dirs).await
}

/// Probe a `dsh` program the caller already resolved to a path.
///
/// Used by the global check, which resolves the candidates itself so it can
/// skip the copies dshl installed into its own cache (see
/// `flow::prepare::probe_user_global_dsh`) and still probe the one it picked.
pub async fn dsh_at(path: &std::path::Path) -> Tool {
    probe_path("dsh", path, &["--version"]).await
}

/// nvm needs special handling: it is a shell function on Unix and a binary
/// (nvm-windows) on Windows.
pub async fn nvm() -> Tool {
    if cfg!(target_os = "windows") {
        let Some(path) = platform::which("nvm") else {
            return Tool::missing("nvm");
        };
        let mut cmd = Command::new(&path);
        cmd.arg("version");
        // Same exit-status gate as every other probe: nvm-windows answers
        // `nvm version` on stdout, but a NON-ZERO exit means the shell is
        // broken and its output must not be mined for a version (this branch
        // used to bypass `tool_from_result` and did exactly that).
        return match process::run_bounded(&mut cmd, PROBE_TIMEOUT).await {
            Ok(res) => tool_from_result(
                "nvm",
                path,
                res.success(),
                res.stdout.trim().to_string(),
                String::new(),
            ),
            Err(_) => Tool {
                name: "nvm",
                found: true,
                path: Some(path),
                version: None,
                raw: String::new(),
            },
        };
    }

    // Unix: nvm is a shell function installed as a script.
    if let Some(home) = platform::home_dir() {
        let mut candidates = vec![home.join(".nvm").join("nvm.sh")];
        if let Ok(dir) = std::env::var("NVM_DIR") {
            candidates.push(PathBuf::from(dir).join("nvm.sh"));
        }
        for c in candidates {
            if c.is_file() {
                return Tool {
                    name: "nvm",
                    found: true,
                    path: Some(c),
                    version: None,
                    raw: "shell function".to_string(),
                };
            }
        }
    }
    // Rare: a real `nvm` binary on PATH.
    probe_cmd("nvm", &["--version"]).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crashed_probe_never_mines_a_version() {
        // 真实事故：坏壳的 --version 崩溃堆栈末尾带 `Node.js v26.7.0`,
        // 全文扫版本号把 node 的版本当成了工具自己的版本。
        let t = tool_from_result(
            "dsh",
            PathBuf::from("C:/nowhere/dsh.exe"),
            false,
            String::new(),
            "Error: Cannot find module 'x'\nNode.js v26.7.0".to_string(),
        );
        assert!(t.found, "binary exists");
        assert_eq!(t.version, None, "failed probe must not yield a version");
        assert_eq!(t.raw, "", "crash output must not become raw");
    }

    #[test]
    fn successful_probe_keeps_combined_output() {
        let t = tool_from_result(
            "nub",
            PathBuf::from("C:/nowhere/nub.exe"),
            true,
            "0.7.5".to_string(),
            String::new(),
        );
        assert_eq!(t.version.map(|v| v.to_string()), Some("0.7.5".into()));
    }
}
