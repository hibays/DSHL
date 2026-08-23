//! Shared launcher entry point — used by both the Track A binary (`dshl` bin)
//! and the Track B cdylib addon (`dshl-native` napi wrapper).
//!
//! Both tracks are the **same kernel**: they go through the identical DPI +
//! locale + debug + single-instance + sigint + setup/flow/run_loop pipeline.
//! The only difference is how the resulting control flow is handed back:
//!
//! * Track A binary (`run_cli`): mirrors the old `main()` exactly — flags
//!   that previously exited the process are instead returned as `RunOutcome`
//!   (`HelpPrinted`, `VersionPrinted`, `ArgsError`), so the binary main can
//!   call `.exit_code()` but the cdylib wrapper can route them into JS-side
//!   returns without ever calling `std::process::exit` (which would kill the
//!   hosting Node process).
//! * Track B addon (`run_with_options` + `RunHandle`): skips CLI flag parsing
//!   entirely, accepts the same options as a struct, and drives the pipeline
//!   on a managed background thread so the Node event loop stays alive.
//!   [`RunHandle`] exposes shutdown/request_restart/window/tray/flow controls
//!   that map 1:1 onto the same `ui::*` / `tray::*` / `platform::*` entry
//!   points the supervisor loop already uses internally.
//!
//! Module layout:
//! - [`options`]: `RunOptions` + `RunOutcome` (the CLI-branch result).
//! - [`handle`]: `RunHandle` opaque handle + `RunHandleInner`.
//! - [`run`]: `run_cli` + `run_with_options` + `ensure_core_init` + the
//!   CLI / HANDLE static locks + `apply_run_options`.
//! - [`signal`]: `install_ctrlc` + `reset_runtime_state` helpers.
//! - [`control`]: thin shims over `dshl_core::{ui, tray, platform}` — the
//!   surface `dshl-native` reaches for when JS calls `windowShow()` etc.

mod control;
mod handle;
mod options;
pub mod run;
mod signal;

pub use control::{
    is_launched, kill_dsh, open_path, open_terminal, open_url, request_restart, request_shutdown,
    tray_hide, tray_is_visible, tray_set_icon, tray_show, window_hide, window_is_visible,
    window_navigate, window_show,
};
pub use handle::RunHandle;
pub use options::{RunOptions, RunOutcome};
pub use run::{ensure_core_init, run_cli, run_with_options};

pub const USAGE: &str = "\
DSHL — DeepSeek Harness web launcher

USAGE:
    dshl [OPTIONS]

OPTIONS:
    -c, --config <path>    Path to dshl.toml
    -d, --debug            Print runtime logs to stderr (also DSHL_LOG=1)
    -V, --version          Print version
    -h, --help             Print help
";
