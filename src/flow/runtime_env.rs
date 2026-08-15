//! Flow 2 — check the JS/TS runtime (node/bun) and install whatever is
//! missing, following the fallback chain in [`crate::install`].

use crate::config::Config;
use crate::error::Result;
use crate::install::{self, Runtime};
use crate::mirror::MirrorConfig;
use crate::probe::{self, Tool};
use crate::progress::{self, StepStatus};

fn describe(t: &Tool) -> String {
    if !t.found {
        return "未安装".to_string();
    }
    let ver = match t.version {
        Some(v) => v.to_string(),
        None if !t.raw.is_empty() => format!("({})", t.raw.trim()),
        None => "版本未知".to_string(),
    };
    let path = t
        .path
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    format!("{ver} @ {path}")
}

pub async fn run(config: &Config, mirror: &MirrorConfig) -> Result<Runtime> {
    progress::step(
        "runtime",
        StepStatus::Running,
        "探测 node/bun/pnpm/fnm/cargo/nvm…",
    );

    // Report the whole toolchain first (transparency). Each probe spawns a
    // child process (pnpm --version alone takes ~0.5s), so they run in
    // parallel — serially they would add ~1-2s to every startup. pnpm is
    // only probed when the config actually asks for it.
    type Probe = (&'static str, fn() -> Tool);
    let probes: Vec<Probe> = vec![
        ("node", probe::node),
        ("bun", probe::bun),
        ("fnm", probe::fnm),
        ("cargo", probe::cargo),
        ("nvm", probe::nvm),
    ];
    let mut all_probes = probes;
    if config.dsh.needs_pnpm() {
        all_probes.push(("pnpm", probe::pnpm));
    }
    let order = ["node", "bun", "pnpm", "fnm", "cargo", "nvm"];
    let (tx, rx) = std::sync::mpsc::channel();
    for (name, probe_fn) in all_probes {
        let tx = tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send((name, probe_fn()));
        });
    }
    drop(tx);
    let mut tools: Vec<(&'static str, Tool)> = rx.iter().collect();
    tools.sort_by_key(|(name, _)| order.iter().position(|o| o == name).unwrap_or(99));
    for (name, tool) in tools {
        progress::log(format!("{name:<5}: {}", describe(&tool)));
    }

    // Node is always required.
    let node_dir = install::ensure_node(mirror).await?;

    // Bun only when the config asks for it.
    let bun_dir = install::ensure_bun(config, mirror).await?;

    // pnpm only when the config asks for it (pm=pnpm / pnpx). The returned
    // dirs are where pnpm links global bins — prepend them to PATH so a
    // freshly installed dsh is reachable even when they are not on PATH.
    let pnpm_dirs = install::ensure_pnpm(config, mirror, &node_dir).await?;

    progress::step("runtime", StepStatus::Done, "运行环境就绪");
    Ok(Runtime {
        node_dir: Some(node_dir),
        bun_dir,
        extra_path: pnpm_dirs,
    })
}
