//! The startup pipeline, split into five independent flows.
//!
//! 1. [`system`]       — OS / architecture.
//! 2. [`runtime_env`]  — node/bun (and fnm/cargo/nvm) with the fallback chain.
//! 3. [`mirror_check`] — domestic-mirror decision (already resolved, reported here).
//! 4. [`prepare`]      — make sure `dsh` is available (global or cache install) and build its launch command.
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

/// Canonical step list shown in the startup UI (id, translation key).
pub const STEPS: &[(&str, &str)] = &[
    ("system", "flow.steps.system"),
    ("runtime", "flow.steps.runtime"),
    ("mirror", "flow.steps.mirror"),
    ("dsh", "flow.steps.dsh"),
    ("launch", "flow.steps.launch"),
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
    // Localize the step titles before handing them to the UI (t! reads the
    // current locale; the keys are the `flow.steps.*` literals from STEPS).
    let steps: Vec<(&'static str, String)> = STEPS
        .iter()
        .map(|&(id, key)| (id, t!(key).to_string()))
        .collect();
    progress::reset(&steps);
    progress::clear_error();

    run_step!("system", system::run());
    let runtime = run_step!("runtime", runtime_env::run(config, mirror));
    run_step!("mirror", mirror_check::run(mirror));
    let command = run_step!("dsh", prepare::run(config, mirror, &runtime));

    let launch = launch::run(command).await?;

    Ok(launch)
}
