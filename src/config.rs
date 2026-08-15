//! `dshl.toml` configuration model, discovery and default template.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Domestic-mirror policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum MirrorMode {
    /// Never use mirrors.
    Off,
    /// Use a mirror when its address is non-empty (default), falling back to
    /// the original source on failure.
    #[default]
    On,
    /// Strictly use the configured mirrors (no fallback to the original).
    Force,
}

/// How dsh should be obtained / executed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum DshMode {
    /// Install `@deepseek-ai/dsh` globally, then run the `dsh` binary.
    #[default]
    Install,
    /// Run directly via `npx` / `bunx` / `pnpx`.
    X,
}

/// JavaScript package manager used for installing dsh.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Pm {
    #[default]
    Npm,
    Bun,
    Pnpm,
}

/// Package-runner prefix used in `x` mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Exector {
    #[default]
    Npx,
    Bunx,
    Pnpx,
}

/// Domestic mirror addresses. An empty string means "do not use this mirror".
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Mirrors {
    /// npm registry (also used by bun's package installs).
    pub npm: String,
    /// cargo sparse registry index (e.g. `sparse+https://rsproxy.cn/index/`).
    pub cargo: String,
    /// Base URL used by fnm/nvm to download Node releases.
    #[serde(rename = "nodejs-release")]
    pub nodejs_release: String,
    /// Base URL used to download the bun runtime binary.
    #[serde(rename = "bun-download")]
    pub bun_download: String,
    /// GitHub proxy prefix (e.g. `https://ghproxy.com/`).
    pub github: String,
}

/// `[dsh]` section — every part of the dsh launch is configurable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Dsh {
    /// Flags forwarded to `dsh` (default boots the web profile on an
    /// ephemeral port).
    pub flags: String,
    /// `install` (default) or `x`.
    pub mode: DshMode,
    /// Package manager for the install mode: `npm` / `bun` / `pnpm`.
    pub pm: Pm,
    /// Runner prefix for the `x` mode: `npx` / `bunx` / `pnpx`.
    #[serde(alias = "executor")]
    pub exector: Exector,
    /// Version suffix for `@deepseek-ai/dsh`. `latest` (default) means no
    /// suffix; anything else becomes `@deepseek-ai/dsh@<version>`.
    pub version: String,
    /// Keep `@deepseek-ai/dsh` up-to-date automatically.
    ///
    /// * `true` (default): when `version` is `latest`, the launcher checks for
    ///   a newer release and updates it (install mode), or lets the runner
    ///   fetch the latest (x mode).
    /// * `false`: use the already-installed / cached version and never update.
    #[serde(rename = "auto-update")]
    pub auto_update: bool,
    /// Only allow one dsh instance system-wide. When `true`, the launcher
    /// refuses to start dsh if another dsh process is already running
    /// (launched manually or by another dshl) — two processes writing the
    /// same session log corrupt it permanently. `false` (default) keeps the
    /// per-launcher behaviour: every dshl runs its own dsh.
    #[serde(rename = "single-instance")]
    pub single_instance: bool,
}

impl Default for Dsh {
    fn default() -> Self {
        Self {
            flags: "--profile web --host 127.0.0.1 --port 0".to_string(),
            mode: DshMode::Install,
            pm: Pm::Npm,
            exector: Exector::Npx,
            version: "latest".to_string(),
            auto_update: true,
            single_instance: false,
        }
    }
}

impl Dsh {
    /// True when no specific version is pinned (`latest`).
    pub fn wants_latest(&self) -> bool {
        self.version == "latest" || self.version.is_empty()
    }

    /// The package spec: `@deepseek-ai/dsh` or `@deepseek-ai/dsh@1.2.3`.
    pub fn package_spec(&self) -> String {
        if self.wants_latest() {
            "@deepseek-ai/dsh".to_string()
        } else {
            format!("@deepseek-ai/dsh@{}", self.version)
        }
    }

    /// True when the chosen pm/runner needs bun installed.
    pub fn needs_bun(&self) -> bool {
        self.pm == Pm::Bun || self.exector == Exector::Bunx
    }

    /// True when the chosen pm/runner needs pnpm installed.
    pub fn needs_pnpm(&self) -> bool {
        self.pm == Pm::Pnpm || self.exector == Exector::Pnpx
    }
}

/// Which window backend is *preferred*. Every value falls back to the other
/// backend when the preferred one is unavailable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum UiMode {
    /// Prefer the embedded WebView (WebView2 / WKWebView / WebKitGTK), then
    /// fall back to an external browser.
    #[default]
    Webview,
    /// Prefer an external web browser, then fall back to the WebView.
    Browser,
}

/// `[ui]` section.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Ui {
    /// `webview` / `browser` — a preference, not a hard choice.
    pub mode: UiMode,
    /// When `true` (Windows only), closing the launcher window hides it to
    /// the system tray instead of exiting, so dsh keeps running in the
    /// background. Restore from the tray icon; use its menu (or Ctrl+C on
    /// the console) to quit. Ignored on platforms without tray support.
    #[serde(rename = "close-to-tray")]
    pub close_to_tray: bool,
    /// Only allow one dshl instance on this machine (a launcher-level
    /// mutex, distinct from `[dsh] single-instance` which guards dsh itself).
    /// When another dshl is already running, this instance does not start:
    /// it activates the existing one instead — restoring it from the tray
    /// if it is hidden, or focusing its window if it is visible.
    #[serde(rename = "single-instance")]
    pub single_instance: bool,
}

/// Root configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// `off` / `on` / `force` (default `on`).
    #[serde(rename = "auto-mirror")]
    pub auto_mirror: MirrorMode,
    pub mirrors: Mirrors,
    pub dsh: Dsh,
    pub ui: Ui,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            auto_mirror: MirrorMode::On,
            mirrors: Mirrors::default(),
            dsh: Dsh::default(),
            ui: Ui::default(),
        }
    }
}

/// Result of locating and parsing the configuration.
#[derive(Debug, Clone)]
pub struct Loaded {
    pub config: Config,
    /// Path of the file the config was read from (`None` = built-in defaults).
    pub path: Option<PathBuf>,
    /// When a config file existed but failed to parse, this carries the
    /// human-readable error (the built-in defaults are used in that case).
    pub parse_error: Option<String>,
}

/// Discover and load `dshl.toml`.
///
/// Search order:
/// 1. explicit `--config <path>` CLI argument (error if missing),
/// 2. `./dshl.toml` (current working directory),
/// 3. `<exe dir>/dshl.toml`,
/// 4. the platform config directory.
pub fn load(cli_path: Option<&Path>) -> Loaded {
    let candidates: Vec<PathBuf> = if let Some(p) = cli_path {
        vec![p.to_path_buf()]
    } else {
        let mut v = Vec::new();
        v.push(PathBuf::from("dshl.toml"));
        if let Some(exe) = crate::platform::current_exe_dir() {
            v.push(exe.join("dshl.toml"));
        }
        v.push(crate::platform::config_dir().join("dshl.toml"));
        // System-wide config installed by the Linux package (lowest priority).
        #[cfg(target_os = "linux")]
        v.push(PathBuf::from("/etc/dshl/dshl.toml"));
        v
    };

    for path in candidates {
        if path.is_file() {
            return match std::fs::read_to_string(&path) {
                Ok(text) => match toml::from_str::<Config>(&text) {
                    Ok(config) => Loaded {
                        config,
                        path: Some(path),
                        parse_error: None,
                    },
                    Err(e) => Loaded {
                        config: Config::default(),
                        path: Some(path),
                        parse_error: Some(format!("{}", e)),
                    },
                },
                Err(e) => Loaded {
                    config: Config::default(),
                    path: Some(path),
                    parse_error: Some(format!("read failed: {e}")),
                },
            };
        }
    }

    if let Some(p) = cli_path {
        // Explicit path that does not exist is a hard, visible error.
        return Loaded {
            config: Config::default(),
            path: Some(p.to_path_buf()),
            parse_error: Some(format!("config file not found: {}", p.display())),
        };
    }

    // No config anywhere: generate a default template so the user has a file
    // to edit (and the "open config" button always points somewhere real).
    let path = default_config_path();
    match write_template(&path) {
        Ok(()) => Loaded {
            config: Config::default(),
            path: Some(path),
            parse_error: None,
        },
        Err(e) => Loaded {
            config: Config::default(),
            path: Some(path),
            parse_error: Some(format!("failed to write default config: {e}")),
        },
    }
}

/// The path where a missing config would be written.
pub fn default_config_path() -> PathBuf {
    crate::platform::config_dir().join("dshl.toml")
}

/// Write the commented default template to `path`.
pub fn write_template(path: &Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, DEFAULT_TEMPLATE)
}

/// The commented default template: the crate-root `dshl.example.toml`,
/// embedded at compile time so it can never drift from the packaged example.
pub const DEFAULT_TEMPLATE: &str = include_str!("../dshl.example.toml");
