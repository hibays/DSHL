//! DSHL — DeepSeek Harness web launcher.
//!
//! A [`webui.me`](https://webui.me) wrapper that:
//! 1. checks the OS / architecture,
//! 2. ensures the JS runtime (node, and optionally bun),
//! 3. resolves domestic mirrors (temporarily),
//! 4. installs / resolves `@deepseek-ai/dsh`,
//! 5. boots `dsh web` and routes the browser to its URL.
//!
//! Everything is configurable through `dshl.toml`, and the launcher uses
//! dependency-free native async (`std::future`) — no tokio.
//!
//! Module map (loosely coupled, one concern per module):
//! - [`platform`]: OS primitives (detection, paths, processes, DPI, theme,
//!   window helpers, single-instance lock) — split into submodules, Windows
//!   APIs via windows-rs 0.62.
//! - [`tray`]: the close-to-tray status icon, one implementation per OS
//!   behind a 6-function interface.
//! - [`i18n`]: locale detection + the startup translation init.
//! - [`ui`]: the window layer (assets, bindings, lifecycle, launch flow,
//!   supervisor loop).
//! - [`flow`]: the startup pipeline (prepare → install → launch).
//! - everything else: config, mirror resolution, probes, progress, keep-alive.

// Load I18n macro so `t!` is usable crate-wide.
#[macro_use]
extern crate rust_i18n;

pub mod config;
pub mod debug;
pub mod error;
pub mod flow;
pub mod i18n;
pub mod install;
pub mod mirror;
pub mod platform;
pub mod probe;
pub mod process;
pub mod progress;
pub mod runtime;
pub mod tray;
pub mod ui;
pub mod version;
pub mod wskeep;

// Init the translations (locales/ dir) with zh-CN as the fallback so any
// untranslated key degrades to Chinese (the app's original language) instead
// of a blank string.
i18n!("locales", fallback = "zh-CN");

use std::sync::{Arc, LazyLock, Mutex};

/// The running `dsh` child, tracked so the launcher (a supervisor) can drain
/// its output, wait for it, and kill it on shutdown.
pub static DSH_CHILD: LazyLock<Mutex<Option<Arc<process::AsyncChild>>>> =
    LazyLock::new(|| Mutex::new(None));
