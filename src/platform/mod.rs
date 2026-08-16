//! Platform detection, shell selection, path discovery and OS helpers.
//!
//! Everything that differs between Windows / Linux / macOS is centralised
//! here so the rest of the code stays platform-agnostic. The module is split
//! into small cohesive submodules — the public facade below re-exports their
//! items so callers keep using `crate::platform::…`:
//!
//! - [`detect`]: OS / architecture / shell detection.
//! - [`paths`]: home/cache/config directories and executable lookup.
//! - [`actions`]: OS-level actions such as opening a file in the file
//!   manager.
//! - [`process`]: process liveness, tree kill and process discovery.
//! - [`dpi`]: DPI awareness and scale factors (Win32 / X11).
//! - [`theme`]: OS dark-mode detection and window theming (Win32).
//! - [`window`]: Win32 window helpers (geometry, focus, discovery).
//! - [`single_instance`]: launcher-level single-instance lock + activation.
//!
//! Windows system APIs go through the `windows` crate (windows-rs 0.62) —
//! there is deliberately no hand-written `#[link] extern "system"` FFI here
//! anymore.

pub mod actions;
pub mod detect;
pub mod dpi;
pub mod paths;
pub mod process;
pub mod single_instance;
pub mod theme;
pub mod window;

pub use actions::{open_path, open_url};
pub use detect::{Arch, Os, Shell, arch, arch_name, os, os_name, shell, shell_command};
pub use dpi::{dpi_scale, dpi_scale_for_window, make_dpi_aware, screen_size};
pub use paths::{
    bin_dir, cache_dir, config_dir, current_exe_dir, default_pnpm_bin_dir, executable_ext,
    home_dir, known_tool_dirs, tool, which, which_in, with_ext,
};
pub use process::{dsh_instance_running, find_process_by_cmdline, kill_tree, process_alive};
pub use theme::{apply_window_theme, is_dark_mode, set_dark_titlebar, set_window_icon};
pub use window::{WindowRect, find_hwnd_by_pid, focus_window, is_window_alive, window_rect};
