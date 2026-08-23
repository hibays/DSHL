//! Flow 2 — check the JS/TS runtime (node/bun/nub) and assemble whatever is
//! missing, following the mirror-aware multi-tier fallback described below.
//!
//! 装配顺序总览（详见 README「多级回退装配系统」）：
//!
//! * **尊重既有环境**：系统 node 满足 `NODE_MIN` 时直接使用，绝不重装。
//! * **pm=nub 且启用镜像**（`mirrors.npm` 已配置）：优先
//!   「npm 镜像安装 nub → nub 经 `NODEJS_ORG_MIRROR` 提供 node」；
//!   任一环节失败立即回退既有 fnm → cargo → nvm 链。
//! * **未启用镜像 / pm≠nub**：fnm → cargo → nvm → 手动指引（fnm 因体积最小
//!   而排在最前）。

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
    // every startup. pnpm/nub are only probed when the config asks for them.
    let mut names: Vec<&'static str> = vec!["node", "bun", "fnm", "cargo", "nvm"];
    if config.dsh.needs_pnpm() {
        names.push("pnpm");
    }
    if config.dsh.needs_nub() {
        names.push("nub");
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
            "nub" => runtime::spawn(probe::nub()),
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
    let order = ["node", "bun", "pnpm", "nub", "fnm", "cargo", "nvm"];
    let mut tools: Vec<(&'static str, Tool)> = Vec::with_capacity(handles.len());
    for (name, handle) in handles {
        tools.push((name, handle.await.unwrap_or_else(|_| missing(name))));
    }
    tools.sort_by_key(|(name, _)| order.iter().position(|o| o == name).unwrap_or(99));
    for (name, tool) in tools.clone() {
        progress::log(format!("{name:<5}: {}", describe(&tool)));
    }

    let node_tool = tools
        .iter()
        .find(|(n, _)| *n == "node")
        .map(|(_, t)| t.clone());

    // ---- Node assembly (mirror-aware, respects the existing environment) --
    //
    // 尊重既有环境：系统 node 满足 NODE_MIN 时直接采用其目录，不做任何安装。
    let node_satisfies = node_tool
        .as_ref()
        .is_some_and(|t| t.found && t.version.is_some_and(|v| v >= install::NODE_MIN));

    let mut nub_dirs: Vec<std::path::PathBuf> = Vec::new();

    // 1) pm=nub：确保 nub 本体存在。优先用户全局 nub，其次缓存安装
    //    （npm 镜像）；失败不致命——回退内置链并把告警写进日志。
    if config.dsh.needs_nub() {
        let hint = node_tool
            .as_ref()
            .and_then(|t| t.path.as_ref())
            .and_then(|p| p.parent())
            .map(|p| p.to_path_buf());
        match install::nub::ensure_nub(config, mirror, hint.as_deref()).await {
            Ok(mut dirs) => nub_dirs.append(&mut dirs),
            Err(e) => progress::log(t!("flow.runtime.nub_failed", err = e.to_string())),
        }
    }

    // 2) Node 目录解析。
    //    - 系统已满足：沿用系统目录。
    //    - 缺失/过旧时：
    //        a) pm=nub 且启用 npm 镜像且 nub 可运行 →
    //           `nub node install` 经 NODEJS_ORG_MIRROR 提供 node（优先）；
    //        b) 否则回退既有 fnm → cargo → nvm → 手动指引链。
    let mut node_dir = if node_satisfies {
        node_tool
            .as_ref()
            .and_then(|t| t.path.as_ref())
            .and_then(|p| p.parent())
            .map(|p| p.to_path_buf())
    } else {
        None
    };

    if node_dir.is_none() {
        // a) nub 提供层（仅 pm=nub 且镜像在位且 nub 可运行）。
        if config.dsh.needs_nub()
            && mirror.enabled()
            && mirror.npm.is_some()
            && (!nub_dirs.is_empty() || probe::nub().await.found)
            && let Some(dir) =
                install::nub::provision_node(mirror, install::NODE_INSTALL_VERSION).await
        {
            progress::log(t!("flow.runtime.node_via_nub", dir = dir.display()));
            node_dir = Some(dir);
        }
        // b) 既有 fnm → cargo → nvm 链。
        if node_dir.is_none() {
            node_dir = Some(install::ensure_node(mirror).await?);
        }
    }
    let node_dir = node_dir.expect("node dir resolved above");

    // Bun only when the config asks for it.
    let bun_dir = install::ensure_bun(config, mirror).await?;

    // pnpm only when the config asks for it (pm=pnpm). The returned dirs are
    // where pnpm lives — prepend them to PATH so a freshly installed pnpm is
    // reachable even when it is not on PATH.
    let mut extra_path = install::ensure_pnpm(config, mirror, &node_dir).await?;

    // nub 的缓存 bin 目录（pm=nub 且为缓存安装时非空；全局 nub 无需注入）。
    extra_path.extend(nub_dirs);

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
