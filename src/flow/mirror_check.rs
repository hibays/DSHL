//! Flow 3 — report the domestic-mirror decision.
//!
//! The mirrors were resolved up-front (they must be known before any network
//! install in Flow 2), so this step only *presents* the decision and the
//! active mirror addresses. Mirrors are always applied temporarily (env/CLI)
//! and never written to disk.

use crate::config::MirrorMode;
use crate::error::Result;
use crate::mirror::MirrorConfig;
use crate::progress::{self, StepStatus};

pub async fn run(mirror: &MirrorConfig) -> Result<()> {
    progress::step("mirror", StepStatus::Running, t!("flow.mirror.resolving"));

    let mode = match mirror.mode {
        MirrorMode::Off => t!("flow.mirror.off"),
        MirrorMode::On => t!("flow.mirror.on"),
        MirrorMode::Force => t!("flow.mirror.force"),
    };
    progress::log(format!("auto-mirror = {mode}"));

    let summary = mirror.summary();
    if summary.is_empty() {
        progress::log(t!("flow.mirror.none"));
    } else {
        for (key, value) in summary {
            progress::log(t!("flow.mirror.entry", key = key, value = value));
        }
    }

    progress::step("mirror", StepStatus::Done, mode);
    Ok(())
}
