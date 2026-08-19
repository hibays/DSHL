//! Flow 4 — make sure `dsh` is available and build its launch command.
//!
//! * `install` mode: check the installed `dsh` (and its version), install
//!   `@deepseek-ai/dsh` with the configured package manager if needed, then
//!   run the `dsh` binary.
//! * `x` mode: run through `npx` / `bunx` / `pnpx` directly.

use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use crate::config::{Config, DshMode, Exector, Pm};
use crate::error::Result;
use crate::install::{Runtime, run_streaming};
use crate::mirror::MirrorConfig;
use crate::platform;
use crate::probe;
use crate::process;
use crate::progress::{self, StepStatus};
use crate::version::FullVersion;

/// Split a flag string the way a shell would (quotes and backslashes).
pub fn split_args(input: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut chars = input.chars().peekable();
    let mut in_single = false;
    let mut in_double = false;
    let mut has_token = false;

    while let Some(c) = chars.next() {
        if in_single {
            if c == '\'' {
                in_single = false;
            } else {
                current.push(c);
            }
        } else if in_double {
            if c == '"' {
                in_double = false;
            } else if c == '\\' {
                match chars.peek() {
                    Some(&n) if n == '"' || n == '\\' => {
                        current.push(n);
                        chars.next();
                    }
                    _ => current.push('\\'),
                }
            } else {
                current.push(c);
            }
        } else {
            match c {
                ' ' | '\t' | '\n' | '\r' => {
                    if has_token {
                        args.push(std::mem::take(&mut current));
                        has_token = false;
                    }
                }
                '\'' => {
                    in_single = true;
                    has_token = true;
                }
                '"' => {
                    in_double = true;
                    has_token = true;
                }
                '\\' => {
                    if let Some(&n) = chars.peek() {
                        current.push(n);
                        chars.next();
                    } else {
                        current.push('\\');
                    }
                    has_token = true;
                }
                _ => {
                    current.push(c);
                    has_token = true;
                }
            }
        }
    }
    if has_token || !current.is_empty() {
        args.push(current);
    }
    args
}

/// Prepend the resolved runtime dirs to `PATH` on a command.
fn apply_path(cmd: &mut Command, runtime: &Runtime) {
    cmd.env("PATH", runtime.augmented_path());
}

fn pm_name(pm: Pm) -> &'static str {
    match pm {
        Pm::Npm => "npm",
        Pm::Bun => "bun",
        Pm::Pnpm => "pnpm",
    }
}

fn exector_name(e: Exector) -> &'static str {
    match e {
        Exector::Npx => "npx",
        Exector::Bunx => "bunx",
        Exector::Pnpx => "pnpx",
    }
}

/// Does the installed dsh satisfy the configured version requirement?
///
/// Compared as full semantic versions (pre-release included), so
/// `0.1.0-rc.6` is distinct from `0.1.0-rc.5` and from `0.1.0`.
fn dsh_version_ok(tool: &probe::Tool, wanted: &str) -> bool {
    if wanted == "latest" || wanted.is_empty() {
        return true;
    }
    let Some(installed) = FullVersion::parse(&tool.raw) else {
        return false;
    };
    let Some(wanted_v) = FullVersion::parse(wanted) else {
        return true; // can't parse the request; don't block on it
    };
    installed == wanted_v
}

async fn install_dsh(config: &Config, mirror: &MirrorConfig, runtime: &Runtime) -> Result<()> {
    let spec = config.dsh.package_spec();
    let pm = pm_name(config.dsh.pm);
    progress::log(t!("flow.prepare.installing", spec = spec, pm = pm));

    let mut cmd = match config.dsh.pm {
        Pm::Npm => {
            let mut c = Command::new(platform::tool("npm"));
            c.args(["install", "-g", &spec]);
            c
        }
        Pm::Bun => {
            let mut c = Command::new(platform::tool("bun"));
            c.args(["add", "-g", "--ignore-scripts", &spec]);
            c
        }
        Pm::Pnpm => {
            let mut c = Command::new(platform::tool("pnpm"));
            c.args(["add", "-g", &spec]);
            c
        }
    };
    apply_path(&mut cmd, runtime);
    process::with_env(&mut cmd, &mirror.npm_env());
    run_streaming(cmd, "install dsh").await
}

/// Query the latest published `@deepseek-ai/dsh` version (best-effort).
///
/// Runs on the tokio runtime with a 5-second cap so a slow/offline registry
/// never stalls the startup pipeline. The query uses the configured package
/// manager (`npm view` / `pnpm view`); bun has no reliable `view`/publish
/// query outside a project directory, so `npm view` is used there — npm
/// ships with node (always present) and reads the same user npmrc as bun.
/// Returns `None` on any failure.
async fn query_latest_version(
    config: &Config,
    mirror: &MirrorConfig,
    runtime: &Runtime,
) -> Option<FullVersion> {
    let env = mirror.npm_env();
    let path = runtime.augmented_path();
    let tool = match config.dsh.pm {
        Pm::Npm | Pm::Bun => "npm",
        Pm::Pnpm => "pnpm",
    };
    let mut cmd = Command::new(platform::tool(tool));
    cmd.args(["view", "@deepseek-ai/dsh", "version"]);
    cmd.env("PATH", path);
    process::with_env(&mut cmd, &env);

    // npm view normally answers in ~1s; the timeout caps a slow/blocked
    // registry so the startup page is not held on a stall.
    let Ok(Ok(res)) =
        tokio::time::timeout(Duration::from_secs(3), process::run_async(&mut cmd)).await
    else {
        return None;
    };
    if res.success() {
        FullVersion::parse(res.stdout.trim())
    } else {
        None
    }
}

/// Build the command that will ultimately be spawned (managed) in Flow 5,
/// plus an optional fallback command to retry with if the primary fails to
/// start.
///
/// * `install` mode runs the installed `dsh` command (`dsh` / `dsh.cmd` /
///   `dsh.sh`) directly; the runner (`npx`/`bunx`/`pnpx`) is the fallback.
/// * `x` mode runs through the configured runner with the **bare** `dsh`
///   name (so an installed dsh resolves without a registry round-trip); the
///   installed `dsh` command is the fallback when the runner fails.
///
/// Runner commands always carry the configured npm mirror env vars, so any
/// actual download goes through the mirror instead of hanging on a blocked
/// default registry.
pub async fn run(
    config: &Config,
    mirror: &MirrorConfig,
    runtime: &Runtime,
) -> Result<(Command, Option<Command>)> {
    progress::step("dsh", StepStatus::Running, t!("flow.prepare.preparing"));

    let flags = crate::control::apply_pending_profile(split_args(&config.dsh.flags));

    // Command that runs the installed `dsh` command directly.
    let direct = || {
        // Resolve against the runtime prefix too: a dsh installed by pnpm
        // (`pnpm add -g`) or by the npm of a freshly installed fnm node
        // lives in a directory that may not be on the ambient PATH.
        let program = platform::which_in("dsh", &runtime.path_prefix())
            .unwrap_or_else(|| PathBuf::from(platform::with_ext("dsh")));
        let mut c = Command::new(program);
        c.args(&flags);
        c
    };

    // Command that runs dsh through the configured runner (npx/bunx/pnpx).
    //
    // The target is the BARE name `dsh` (optionally `dsh@<version>`), not the
    // package spec `@deepseek-ai/dsh`: `bunx dsh` resolves the already
    // installed command (e.g. bun's global `~/.bun/bin/dsh`) without any
    // registry round-trip, while `bunx @deepseek-ai/dsh` forces a manifest
    // lookup that hangs on a blocked/slow registry ("Resolving
    // dependencies"). The npm mirror env vars make any actual download go
    // through the configured mirror.
    let runner = || {
        let exe = exector_name(config.dsh.exector);
        let mut c = Command::new(platform::tool(exe));
        match config.dsh.exector {
            Exector::Npx | Exector::Pnpx => {
                c.arg("--yes");
                if !config.dsh.auto_update {
                    c.arg("--prefer-offline");
                }
            }
            Exector::Bunx => {}
        }
        let name = if config.dsh.wants_latest() {
            "dsh".to_string()
        } else {
            format!("dsh@{}", config.dsh.version)
        };
        c.arg(name);
        c.args(&flags);
        process::with_env(&mut c, &mirror.npm_env());
        c
    };

    let (mut cmd, mut fallback) = match config.dsh.mode {
        DshMode::Install => {
            let dsh = probe::dsh_in(&runtime.path_prefix()).await;
            if !dsh.found {
                progress::log(t!("flow.prepare.not_installed"));
                install_dsh(config, mirror, runtime).await?;
            } else if config.dsh.wants_latest() {
                // No pinned version: auto-update decides whether to refresh.
                if config.dsh.auto_update {
                    match query_latest_version(config, mirror, runtime).await {
                        Some(latest) => match FullVersion::parse(&dsh.raw) {
                            Some(installed) if installed >= latest => {
                                progress::log(t!(
                                    "flow.prepare.up_to_date",
                                    installed = installed.to_string()
                                ));
                            }
                            _ => {
                                progress::log(t!(
                                    "flow.prepare.updating",
                                    current = dsh.raw.trim(),
                                    latest = latest.to_string()
                                ));
                                install_dsh(config, mirror, runtime).await?;
                            }
                        },
                        None => {
                            progress::log(t!(
                                "flow.prepare.version_query_failed",
                                installed = dsh.raw.trim()
                            ));
                        }
                    }
                } else {
                    progress::log(t!(
                        "flow.prepare.auto_update_off",
                        installed = dsh.raw.trim()
                    ));
                }
            } else if dsh_version_ok(&dsh, &config.dsh.version) {
                progress::log(t!("flow.prepare.installed", installed = dsh.raw.trim()));
            } else {
                progress::log(t!(
                    "flow.prepare.version_mismatch",
                    wanted = config.dsh.version,
                    current = dsh.raw.trim()
                ));
                install_dsh(config, mirror, runtime).await?;
            }

            // Run the installed `dsh` command directly (dsh / dsh.cmd /
            // dsh.sh), spawned in a hidden console so no window flashes.
            // Fall back to the runner if the direct launch fails.
            (direct(), Some(runner()))
        }
        DshMode::X => {
            // x mode: run through bunx / npx / pnpx (bare `dsh` name, so an
            // installed dsh is used without a registry round-trip). If the
            // runner fails to start, retry with the installed `dsh` command
            // when one exists.
            let installed = probe::dsh_in(&runtime.path_prefix()).await;
            let target = if config.dsh.wants_latest() {
                "dsh".to_string()
            } else {
                format!("dsh@{}", config.dsh.version)
            };
            if installed.found && dsh_version_ok(&installed, &config.dsh.version) {
                progress::log(t!(
                    "flow.prepare.x_installed",
                    exector = exector_name(config.dsh.exector),
                    installed = installed.raw.trim()
                ));
            } else {
                let desc = if installed.found {
                    t!("flow.prepare.runner_resolve")
                } else {
                    t!("flow.prepare.runner_download")
                };
                progress::log(t!(
                    "flow.prepare.x_runner",
                    exector = exector_name(config.dsh.exector),
                    target = target,
                    desc = desc,
                    spec = config.dsh.package_spec()
                ));
            }
            let fallback = if installed.found {
                Some(direct())
            } else {
                None
            };
            (runner(), fallback)
        }
    };

    apply_path(&mut cmd, runtime);
    if let Some(fb) = &mut fallback {
        apply_path(fb, runtime);
    }
    // Remember the resolved runtime PATH so the control `open-terminal`
    // method can spawn a terminal with the same (dsh-like) environment.
    crate::control::store_runtime_path(&runtime.augmented_path());
    progress::step("dsh", StepStatus::Done, t!("flow.prepare.ready"));
    Ok((cmd, fallback))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::probe;

    #[test]
    fn splits_plain_flags() {
        assert_eq!(
            split_args("--profile web --host 127.0.0.1 --port 0"),
            vec!["--profile", "web", "--host", "127.0.0.1", "--port", "0"]
        );
    }

    #[test]
    fn honors_quotes() {
        assert_eq!(
            split_args("--profile \"my web\" --trusted-host 'a b'"),
            vec!["--profile", "my web", "--trusted-host", "a b"]
        );
    }

    #[test]
    fn empty_input() {
        assert_eq!(split_args(""), Vec::<String>::new());
        assert_eq!(split_args("   "), Vec::<String>::new());
    }

    #[test]
    fn dsh_version_ok_compares_prereleases() {
        let tool = |raw: &str| probe::Tool {
            name: "dsh",
            found: true,
            path: None,
            version: None,
            raw: raw.to_string(),
        };
        // Exact match, pre-release included.
        assert!(dsh_version_ok(&tool("0.1.0-rc.6"), "0.1.0-rc.6"));
        // A different release candidate is NOT ok anymore (was: both parsed
        // to 0.1.0 and matched).
        assert!(!dsh_version_ok(&tool("0.1.0-rc.5"), "0.1.0-rc.6"));
        assert!(!dsh_version_ok(&tool("0.1.0"), "0.1.0-rc.6"));
        assert!(!dsh_version_ok(&tool("0.2.0"), "0.1.0-rc.6"));
        // latest / empty never blocks.
        assert!(dsh_version_ok(&tool("0.1.0-rc.6"), "latest"));
        assert!(dsh_version_ok(&tool("0.1.0-rc.6"), ""));
        // Unparseable installed output is treated as a mismatch (reinstall).
        assert!(!dsh_version_ok(&tool("garbage"), "0.1.0-rc.6"));
    }

    #[test]
    fn full_version_update_decision() {
        use crate::version::FullVersion;
        let installed = |raw: &str| FullVersion::parse(raw);
        let latest = FullVersion::parse("0.1.0-rc.6");
        // rc.5 → rc.6 must trigger an update (the bug this fixed).
        assert!(installed("0.1.0-rc.5").unwrap() < latest.clone().unwrap());
        assert!(installed("0.1.0-rc.6").unwrap() >= latest.clone().unwrap());
        assert!(installed("0.1.0").unwrap() > latest.unwrap());
    }
}
