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
    progress::step("mirror", StepStatus::Running, "解析镜像策略…");

    let mode = match mirror.mode {
        MirrorMode::Off => "off（禁用）",
        MirrorMode::On => "on（自动，默认）",
        MirrorMode::Force => "force（强制）",
    };
    progress::log(format!("auto-mirror = {mode}"));

    let summary = mirror.summary();
    if summary.is_empty() {
        progress::log("未配置任何国内镜像（地址均为空）");
    } else {
        for (key, value) in summary {
            progress::log(format!("镜像 {key} = {value}"));
        }
    }

    progress::step("mirror", StepStatus::Done, mode);
    Ok(())
}
