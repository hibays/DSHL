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

pub mod config;
pub mod debug;
pub mod error;
pub mod flow;
pub mod install;
pub mod mirror;
pub mod platform;
pub mod probe;
pub mod process;
pub mod progress;
pub mod runtime;
pub mod ui;
pub mod version;
pub mod wskeep;

use std::sync::{Arc, LazyLock, Mutex};

/// The running `dsh` child, tracked so the launcher (a supervisor) can drain
/// its output, wait for it, and kill it on shutdown.
pub static DSH_CHILD: LazyLock<Mutex<Option<Arc<process::AsyncChild>>>> =
    LazyLock::new(|| Mutex::new(None));
