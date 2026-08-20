//! Flow 2 — check the JS/TS runtime (node/bun) and install whatever is
//! missing, following the fallback chain in [`crate::install`].

use crate::config::Config;
use crate::error::Result;
use crate::install::{self, Runtime};
use crate::mirror::MirrorConfig;
use crate::probe::{self, Tool};
use crate::progress::{self, StepStatus};
use crate::runtime;

fn describe(t: &Tool) -> String {
    if !t.found {
        return t!("flow.runtime.not_installed").to_string();
    }
    let ver = match t.version {
        Some(v) => v.to_string(),
        None if !t.raw.is_empty() => format!("({})", t.raw.trim()),
        None => t!("flow.runtime.version_unknown").to_string(),
    };
    let path = t
        .path
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    format!("{ver} @ {path}")
}

pub async fn run(config: &Config, mirror: &MirrorConfig) -> Result<Runtime> {
    progress::step("runtime", StepStatus::Running, t!("flow.runtime.probing"));

    // Report the whole toolchain first (transparency). Each probe spawns a
    // child process (pnpm --version alone takes ~0.5s), so they run
    // concurrently on the tokio runtime — serially they would add ~1-2s to
    // every startup. pnpm is only probed when the config actually asks for it.
    let mut names: Vec<&'static str> = vec!["node", "bun", "fnm", "cargo", "nvm"];
    if config.dsh.needs_pnpm() {
        names.push("pnpm");
    }
    let mut handles: Vec<(&'static str, _)> = Vec::new();
    for name in names {
        let handle = match name {
            "node" => runtime::spawn(probe::node()),
            "bun" => runtime::spawn(probe::bun()),
            "fnm" => runtime::spawn(probe::fnm()),
            "cargo" => runtime::spawn(probe::cargo()),
            "nvm" => runtime::spawn(probe::nvm()),
            "pnpm" => runtime::spawn(probe::pnpm()),
            _ => unreachable!(),
        };
        handles.push((name, handle));
    }
    let missing = |name: &'static str| Tool {
        name,
        found: false,
        path: None,
        version: None,
        raw: String::new(),
    };
    let order = ["node", "bun", "pnpm", "fnm", "cargo", "nvm"];
    let mut tools: Vec<(&'static str, Tool)> = Vec::with_capacity(handles.len());
    for (name, handle) in handles {
        tools.push((name, handle.await.unwrap_or_else(|_| missing(name))));
    }
    tools.sort_by_key(|(name, _)| order.iter().position(|o| o == name).unwrap_or(99));
    for (name, tool) in tools {
        progress::log(format!("{name:<5}: {}", describe(&tool)));
    }

    // Node is always required.
    let node_dir = install::ensure_node(mirror).await?;

    // Bun only when the config asks for it.
    let bun_dir = install::ensure_bun(config, mirror).await?;

    // pnpm only when the config asks for it (pm=pnpm). The returned dirs are
    // where pnpm lives — prepend them to PATH so a freshly installed pnpm is
    // reachable even when it is not on PATH.
    let mut extra_path = install::ensure_pnpm(config, mirror, &node_dir).await?;

    // fnm binaries dshl may have installed into the cache (cargo --root, or
    // the ~/.cache/bin auto-install) — put them on the runtime PATH too so the
    // whole toolchain stays reachable without the user's global PATH.
    for dir in [
        crate::platform::cache_dir()
            .join("dshl")
            .join("fnm-cargo")
            .join("bin"),
        crate::platform::bin_dir(),
    ] {
        if dir.is_dir() && !extra_path.contains(&dir) {
            extra_path.push(dir);
        }
    }

    progress::step("runtime", StepStatus::Done, t!("flow.runtime.ready"));
    Ok(Runtime {
        node_dir: Some(node_dir),
        bun_dir,
        extra_path,
    })
}
