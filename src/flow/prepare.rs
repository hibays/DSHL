//! Flow 4 — make sure `dsh` is available and build its launch command.
//!
//! dsh is a node script: it is always launched directly as `node <entry>`
//! from an installed copy — either the user's global `dsh` (in `hybrid`/`global`
//! mode) or dshl's private cache install.

use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use crate::config::{Config, DshMode, Pm};
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
        Pm::Nub => "nub",
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

/// Probe the user's global `dsh`: ambient PATH first, then the runtime prefix.
async fn probe_global(runtime: &Runtime) -> probe::Tool {
    match probe::dsh().await {
        p if p.found => p,
        _ => probe::dsh_in(&runtime.path_prefix()).await,
    }
}

/// Localized label for *which* dsh a decision log line talks about (the
/// user's global one vs. dshl's cache install) — without it, hybrid-mode
/// timelines read as two contradictory statements about one dsh.
fn src_label(global: bool) -> String {
    if global {
        t!("flow.prepare.src_global").to_string()
    } else {
        t!("flow.prepare.src_cache").to_string()
    }
}

/// Hybrid mode: use the global dsh when it satisfies `version`, else fall
/// back to a cache install.
async fn hybrid_use_global(config: &Config, target: &str, runtime: &Runtime) -> bool {
    let dsh = probe_global(runtime).await;
    if !dsh.found {
        progress::log(t!(
            "flow.prepare.not_installed",
            source = t!("flow.prepare.src_global")
        ));
        return false;
    }
    let src = src_label(true);
    if !config.dsh.wants_latest() {
        // Pinned version: use the global only when it matches.
        if dsh_version_ok(&dsh, &config.dsh.version) {
            progress::log(t!(
                "flow.prepare.installed",
                source = src,
                installed = dsh.raw.trim()
            ));
            return true;
        }
        progress::log(t!(
            "flow.prepare.version_mismatch",
            source = src,
            wanted = config.dsh.version,
            current = dsh.raw.trim()
        ));
        return false;
    }
    if !config.dsh.auto_update || target == "latest" {
        // latest, but auto-update is off (or the latest could not be learned)
        // — keep the global as-is.
        progress::log(t!(
            "flow.prepare.installed",
            source = src,
            installed = dsh.raw.trim()
        ));
        return true;
    }
    // latest + auto-update: use the global only if it is already up to date;
    // otherwise fall through to a fresh cache install so a stale global dsh
    // is not run forever.
    match (FullVersion::parse(&dsh.raw), FullVersion::parse(target)) {
        (Some(installed), Some(latest)) if installed >= latest => {
            progress::log(t!(
                "flow.prepare.up_to_date",
                source = src,
                installed = installed.to_string()
            ));
            true
        }
        _ => {
            progress::log(t!(
                "flow.prepare.updating",
                source = src,
                current = dsh.raw.trim(),
                latest = target.to_string()
            ));
            false
        }
    }
}

/// dshl's cache install of dsh: `<cache>/dshl`. dsh is a node module, so the
/// `--prefix` install drops it straight into `<cache>/dshl/node_modules` (the
/// package entry at `@deepseek-ai/dsh`) — no extra per-version directory and
/// no touch of the user's global environment or PATH. Version pinning still
/// applies to the installed spec, but there is deliberately no version
/// isolation — one dsh kernel per machine is enough.
pub fn dsh_dir() -> std::path::PathBuf {
    crate::platform::cache_dir().join("dshl")
}

/// The `.bin` dir of the cache install (holds `dsh`, `dsh.cmd`, …).
fn dsh_bin_dir() -> std::path::PathBuf {
    dsh_dir().join("node_modules").join(".bin")
}

/// The `@deepseek-ai/dsh` package directory inside the cache install.
fn dsh_pkg_dir() -> std::path::PathBuf {
    dsh_dir()
        .join("node_modules")
        .join("@deepseek-ai")
        .join("dsh")
}

/// The `bin` entry of an npm package, read from its `package.json`. This is
/// the file `node` runs (e.g. `lib/bin.js`) — dsh is a node script, so
/// launching it is just `node <entry>`; no shim, link or runner is needed.
///
/// There is deliberately no fallback: a missing or malformed manifest must be
/// an install failure, not a guess (a wrong guess used to spawn
/// `node ...\node_modules\.bin\dsh.exe` and die with MODULE_NOT_FOUND).
fn package_entry(pkg: &std::path::Path) -> Option<std::path::PathBuf> {
    let manifest = std::fs::read_to_string(pkg.join("package.json")).ok()?;
    let json: serde_json::Value = serde_json::from_str(&manifest).ok()?;
    let file = match json.get("bin")? {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Object(map) => map
            .get("dsh")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .or_else(|| map.values().find_map(|v| v.as_str().map(str::to_string)))?,
        _ => return None,
    };
    let path = pkg.join(file);
    path.is_file().then_some(path)
}

/// The version of the dsh package in the cache, read from its
/// `package.json`. `None` when the cache holds no parseable dsh.
fn cached_dsh_version(pkg: &std::path::Path) -> Option<FullVersion> {
    let manifest = std::fs::read_to_string(pkg.join("package.json")).ok()?;
    let json: serde_json::Value = serde_json::from_str(&manifest).ok()?;
    FullVersion::parse(json.get("version")?.as_str()?)
}

/// Should the cache install be refreshed to reach `target`?
///
/// Pure decision over (cached version, target spec) so it stays unit-testable:
/// * target `latest`: any usable cache entry passes — the registry may have a
///   newer release, but re-installing on every start would hit the network
///   each launch; auto-update only moves the cache when the *global* probe
///   already forced this code path.
/// * pinned target: refresh unless the cached version equals it exactly.
fn cache_needs_install(cached: Option<&FullVersion>, target: &str) -> bool {
    match cached {
        None => true, // nothing usable in the cache
        Some(_) if target == "latest" || target.is_empty() => false,
        Some(v) => match FullVersion::parse(target) {
            // Can't parse the request; keep what we have rather than churn.
            None => false,
            Some(wanted) => *v != wanted,
        },
    }
}

/// Install `spec` into the cache dir (never `-g`, which would pollute the
/// user's global `node_modules` / PATH). The install is local to the prefix,
/// and the resulting package entry is picked up by [`package_entry`].
async fn install_dsh(
    config: &Config,
    mirror: &MirrorConfig,
    runtime: &Runtime,
    spec: &str,
) -> Result<()> {
    let dir = dsh_dir();
    let pm = pm_name(config.dsh.pm);
    std::fs::create_dir_all(&dir).ok();

    // Pin the install to the cache directory. Without a manifest here, bun and
    // pnpm walk up looking for the nearest project root — dsh then lands in
    // whatever package.json is above (observed: the user's home directory),
    // and dshl's own cache stays empty.
    let manifest = dir.join("package.json");
    if !manifest.is_file() {
        std::fs::write(
            &manifest,
            "{\n  \"name\": \"dshl-cache\",\n  \"private\": true,\n  \"dependencies\": {}\n}\n",
        )
        .map_err(|e| {
            crate::error::Error(
                t!(
                    "flow.prepare.cache_manifest_failed",
                    path = manifest.display().to_string(),
                    err = e
                )
                .to_string(),
            )
        })?;
    }

    progress::log(t!(
        "flow.prepare.installing",
        spec = spec,
        pm = pm,
        dir = dir.display().to_string()
    ));

    let mut cmd = match config.dsh.pm {
        Pm::Npm => {
            let mut c = Command::new(platform::tool("npm"));
            c.args(["install", "--prefix"]);
            c.arg(&dir);
            c.args(["--no-save", spec]);
            c
        }
        Pm::Bun => {
            let mut c = Command::new(platform::tool("bun"));
            c.arg("add");
            c.arg("--cwd");
            c.arg(&dir);
            c.arg(spec);
            c
        }
        Pm::Pnpm => {
            let mut c = Command::new(platform::tool("pnpm"));
            c.arg("add");
            c.arg("--dir");
            c.arg(&dir);
            c.arg(spec);
            c
        }
        // nub add has no --cwd/--dir flag (verified against 0.7.5 help):
        // run it with the cache dir as the working directory instead.
        Pm::Nub => {
            let mut c = Command::new(platform::tool("nub"));
            c.arg("add");
            c.arg(spec);
            c.current_dir(&dir);
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
        // nub exposes `view` too, but npm is always present and reads the
        // same registry config — keep the query on the proven path.
        Pm::Npm | Pm::Bun => "npm",
        // nub has a native `view` (registry query) - use the configured PM.
        Pm::Nub => "nub",
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

/// Build the command that will ultimately be spawned (managed) in Flow 5.
///
/// dsh is a node script, so it is always launched as `node <entry>` from an
/// installed copy — never through a `npx`/`bunx`/`pnpx` runner and never
/// linked into the user's global environment.
///
/// The source is selected by [`Dsh::mode`]:
/// * `global`: the user's global `dsh` is required; an error if it is missing.
/// * `hybrid` (default): the user's global `dsh` (ambient PATH, then the
///   runtime prefix) is used when it satisfies `version`; otherwise dsh is
///   installed into dshl's private cache.
/// * `private`: dsh is always installed into dshl's cache, never touching the
///   user's global environment.
///
/// Whatever the source, the spawned dsh inherits a PATH that prepends the
/// resolved toolchain (node, bun, pnpm, and the cache `.bin` when running
/// from the cache) so dsh can run its web server and install plugins without
/// requiring the user to have those on their global PATH.
pub async fn run(config: &Config, mirror: &MirrorConfig, runtime: &Runtime) -> Result<Command> {
    progress::step("dsh", StepStatus::Running, t!("flow.prepare.preparing"));

    let flags = crate::control::apply_pending_profile(split_args(&config.dsh.flags));

    // Resolve the target version: a pinned version, or the latest release
    // (queried only when auto-update is on).
    let target = if !config.dsh.wants_latest() {
        config.dsh.version.clone()
    } else if config.dsh.auto_update {
        match query_latest_version(config, mirror, runtime).await {
            Some(latest) => latest.to_string(),
            None => "latest".to_string(),
        }
    } else {
        "latest".to_string()
    };
    let spec = if target == "latest" {
        "@deepseek-ai/dsh".to_string()
    } else {
        format!("@deepseek-ai/dsh@{target}")
    };

    // Choose the dsh source according to `dsh.mode` (global / hybrid / private).
    let global = match config.dsh.mode {
        DshMode::Private => false,
        DshMode::Global => {
            let dsh = probe_global(runtime).await;
            if !dsh.found {
                return Err(crate::error::Error(
                    t!("flow.prepare.global_requires_dsh").to_string(),
                ));
            }
            progress::log(t!(
                "flow.prepare.installed",
                source = t!("flow.prepare.src_global"),
                installed = dsh.raw.trim()
            ));
            true
        }
        DshMode::Hybrid => hybrid_use_global(config, &target, runtime).await,
    };

    let mut cmd = if global {
        // Run the user's global `dsh` directly (dsh / dsh.cmd / dsh.sh),
        // spawned in a hidden console so no window flashes.
        let program = platform::which("dsh")
            .or_else(|| platform::which_in("dsh", &runtime.path_prefix()))
            .unwrap_or_else(|| PathBuf::from(platform::with_ext("dsh")));
        let mut c = Command::new(program);
        c.args(&flags);
        c
    } else {
        // Cache install: decide from the *cached package's* version (not just
        // its presence) whether an install/update is needed, then run
        // `node <package-bin-entry>`. The cache probe is logged either way,
        // so the timeline shows the full global → cache decision chain.
        let cached = cached_dsh_version(&dsh_pkg_dir());
        let src = src_label(false);
        if cache_needs_install(cached.as_ref(), &target) {
            match cached {
                Some(v) => progress::log(t!(
                    "flow.prepare.version_mismatch",
                    source = src,
                    wanted = target,
                    current = v.to_string()
                )),
                None => progress::log(t!(
                    "flow.prepare.not_installed",
                    source = t!("flow.prepare.src_cache")
                )),
            }
            install_dsh(config, mirror, runtime, &spec).await?;
        } else if let Some(v) = cached {
            // Cache hit: the installed copy already satisfies the target.
            progress::log(t!(
                "flow.prepare.installed",
                source = src,
                installed = v.to_string()
            ));
        }
        // Strict: after a successful install the entry must exist. A fallback
        // guess here would spawn `node <something wrong>` downstream.
        let entry = package_entry(&dsh_pkg_dir()).ok_or_else(|| {
            crate::error::Error(
                t!(
                    "flow.prepare.entry_missing",
                    dir = dsh_pkg_dir().display().to_string()
                )
                .to_string(),
            )
        })?;
        let node = platform::which_in("node", &runtime.path_prefix())
            .unwrap_or_else(|| platform::tool("node"));
        let mut c = Command::new(node);
        c.arg(entry);
        c.args(&flags);
        c
    };

    // Inject the resolved toolchain into the dsh process's PATH so it can run
    // its web server and install plugins: node, bun, pnpm, and (when running
    // from the cache) the cache install's .bin. This is temporary and scoped
    // to the dsh process — the user's own environment is untouched.
    let mut parts: Vec<std::ffi::OsString> = Vec::new();
    if !global {
        parts.push(dsh_bin_dir().into_os_string());
    }
    parts.extend(runtime.path_prefix().into_iter().map(Into::into));
    if let Some(existing) = std::env::var_os("PATH") {
        parts.push(existing);
    }
    cmd.env(
        "PATH",
        std::env::join_paths(parts)
            .unwrap_or_else(|_| std::env::var_os("PATH").unwrap_or_default()),
    );
    process::with_env(&mut cmd, &mirror.npm_env());

    // Remember the resolved runtime PATH so the control `open-terminal`
    // method can spawn a terminal with the same (dsh-like) environment.
    crate::control::store_runtime_path(&runtime.augmented_path());
    progress::step("dsh", StepStatus::Done, t!("flow.prepare.ready"));
    Ok(cmd)
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

    #[test]
    fn cache_needs_install_by_version() {
        let v = |s: &str| FullVersion::parse(s);
        // Nothing usable in the cache: always install.
        assert!(cache_needs_install(None, "latest"));
        assert!(cache_needs_install(None, "0.1.1-rc.2"));
        // Latest (update info unavailable): keep a usable cache entry.
        assert!(!cache_needs_install(v("0.1.0-rc.6").as_ref(), "latest"));
        assert!(!cache_needs_install(v("0.1.0-rc.6").as_ref(), ""));
        // Pinned target: exact match keeps, anything else refreshes.
        assert!(!cache_needs_install(v("0.1.1-rc.2").as_ref(), "0.1.1-rc.2"));
        assert!(cache_needs_install(v("0.1.0-rc.7").as_ref(), "0.1.1-rc.2"));
        assert!(cache_needs_install(v("0.1.1-rc.1").as_ref(), "0.1.1-rc.2"));
        // Unparseable request: don't churn the cache.
        assert!(!cache_needs_install(v("0.1.0-rc.6").as_ref(), "garbage"));
    }

    #[test]
    fn cached_dsh_version_reads_manifest() {
        let dir = std::env::temp_dir().join(format!("dshl-test-cached-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let pkg = dir.join("@deepseek-ai").join("dsh");
        std::fs::create_dir_all(&pkg).unwrap();
        // No manifest yet.
        assert_eq!(cached_dsh_version(&pkg), None);
        std::fs::write(
            pkg.join("package.json"),
            r#"{"name":"@deepseek-ai/dsh","version":"0.1.1-rc.2"}"#,
        )
        .unwrap();
        assert_eq!(cached_dsh_version(&pkg), FullVersion::parse("0.1.1-rc.2"));
        // Garbage manifest → None (treated as "needs install").
        std::fs::write(pkg.join("package.json"), "not json").unwrap();
        assert_eq!(cached_dsh_version(&pkg), None);
        std::fs::remove_dir_all(&dir).ok();
    }
}
