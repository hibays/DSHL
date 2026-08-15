//! Detection of the tools dshl cares about: existence, path and version.

use std::path::PathBuf;
use std::process::Command;

use crate::platform;
use crate::process;
use crate::version::Version;

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
    fn missing(name: &'static str) -> Self {
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

fn probe_cmd(name: &'static str, version_args: &[&str]) -> Tool {
    probe_cmd_in(name, version_args, &[])
}

/// Like `probe_cmd`, but searches `extra_dirs` first (plus the normal
/// locations), so tools that an earlier flow step installed (fnm's node, a
/// fresh pnpm, …) are found even when they are not on the ambient `PATH`.
fn probe_cmd_in(name: &'static str, version_args: &[&str], extra_dirs: &[PathBuf]) -> Tool {
    let Some(path) = platform::which_in(name, extra_dirs) else {
        return Tool::missing(name);
    };
    let mut cmd = Command::new(&path);
    cmd.args(version_args);
    match process::run(&mut cmd) {
        Ok(res) => {
            let raw = format!("{}{}", res.stdout.trim(), res.stderr.trim());
            Tool::found(name, path, raw)
        }
        Err(_) => Tool {
            name,
            found: true,
            path: Some(path),
            version: None,
            raw: String::new(),
        },
    }
}

pub fn node() -> Tool {
    probe_cmd("node", &["--version"])
}

pub fn bun() -> Tool {
    probe_cmd("bun", &["--version"])
}

pub fn pnpm() -> Tool {
    probe_cmd("pnpm", &["--version"])
}

pub fn fnm() -> Tool {
    probe_cmd("fnm", &["--version"])
}

pub fn cargo() -> Tool {
    probe_cmd("cargo", &["--version"])
}

pub fn dsh() -> Tool {
    probe_cmd("dsh", &["--version"])
}

/// Probe `dsh` searching `extra_dirs` first (the runtime prefix: fnm's node
/// bin, pnpm's global bin, …), so a just-installed dsh is found even when
/// its directory is not on the ambient `PATH`.
pub fn dsh_in(extra_dirs: &[PathBuf]) -> Tool {
    probe_cmd_in("dsh", &["--version"], extra_dirs)
}

/// nvm needs special handling: it is a shell function on Unix and a binary
/// (nvm-windows) on Windows.
pub fn nvm() -> Tool {
    if cfg!(target_os = "windows") {
        let Some(path) = platform::which("nvm") else {
            return Tool::missing("nvm");
        };
        let mut cmd = Command::new(&path);
        cmd.arg("version");
        return match process::run(&mut cmd) {
            Ok(res) => {
                let raw = res.stdout.trim().to_string();
                Tool::found("nvm", path, raw)
            }
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
    probe_cmd("nvm", &["--version"])
}
