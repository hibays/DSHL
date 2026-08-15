//! The startup pipeline, split into five independent flows.
//!
//! 1. [`system`]       — OS / architecture.
//! 2. [`runtime_env`]  — node/bun (and fnm/cargo/nvm) with the fallback chain.
//! 3. [`mirror_check`] — domestic-mirror decision (already resolved, reported here).
//! 4. [`prepare`]      — install `@deepseek-ai/dsh` (install mode) or resolve the `npx`/`bunx`/`pnpx` runner (x mode).
//! 5. [`launch`]       — spawn `dsh` (managed), capture its URL, return it.

use std::sync::Arc;

use crate::config::Config;
use crate::error::Result;
use crate::mirror::MirrorConfig;
use crate::process::AsyncChild;
use crate::progress::{self, StepStatus};

pub mod launch;
pub mod mirror_check;
pub mod prepare;
pub mod runtime_env;
pub mod system;

/// Canonical step list shown in the startup UI.
pub const STEPS: &[(&str, &str)] = &[
    ("system", "检查系统环境与架构"),
    ("runtime", "检查运行环境 (node/bun)"),
    ("mirror", "检查国内镜像"),
    ("dsh", "准备 dsh (install/x)"),
    ("launch", "启动 dsh web"),
];

/// Outcome of a successful launch.
pub struct Launch {
    /// The URL that `dsh web` printed (e.g. `http://127.0.0.1:61239`).
    pub url: String,
    /// The managed dsh child, kept so the supervisor can drain and reap it.
    pub child: Arc<AsyncChild>,
}

macro_rules! run_step {
    ($id:expr, $fut:expr) => {
        match $fut.await {
            Ok(value) => value,
            Err(err) => {
                let msg = err.to_string();
                progress::step($id, StepStatus::Error, msg.clone());
                progress::set_error(msg);
                return Err(err);
            }
        }
    };
}

/// Run the full startup pipeline.
pub async fn run(config: &Config, mirror: &MirrorConfig) -> Result<Launch> {
    progress::reset(STEPS);
    progress::clear_error();

    run_step!("system", system::run());
    let runtime = run_step!("runtime", runtime_env::run(config, mirror));
    run_step!("mirror", mirror_check::run(mirror));
    let (command, fallback) = run_step!("dsh", prepare::run(config, mirror, &runtime));

    let launch = match launch::run(command).await {
        Ok(launch) => launch,
        Err(err) => {
            // The primary run failed: retry once through the fallback command
            // before giving up.
            let Some(fallback) = fallback else {
                return Err(err);
            };
            progress::log(format!("直接启动失败（{err}），改用 runner 重试"));
            // Stop the failed attempt's child first. It is usually already
            // dead (early exit), but on a URL timeout it is still running —
            // starting a second dsh next to it would make two processes write
            // the same session log and corrupt it. Ask it to shut down
            // gracefully (Ctrl+C) and WAIT; if it does not exit on its own,
            // abort the fallback rather than starting a second dsh next to it
            // (two writers on the same session log corrupt it permanently).
            if let Some(child) = crate::DSH_CHILD.lock().unwrap().take()
                && !child.graceful_kill(10_000)
            {
                progress::log(
                    "旧的 dsh 未响应退出请求，为避免会话日志损坏，放弃 runner 重试".to_string(),
                );
                // The old dsh is still running. Put it back into the global
                // slot so a later retry (`launch_flow`) finds it and stops it
                // via Ctrl+C before starting a new dsh — two processes writing
                // the same session log corrupt it permanently ("seq gap") and
                // the chat history becomes unloadable.
                *crate::DSH_CHILD.lock().unwrap() = Some(child);
                return Err(err);
            }
            run_step!("launch", launch::run(fallback))
        }
    };

    Ok(launch)
}
