//! Domestic-mirror resolution.
//!
//! Mirrors are applied *temporarily* (environment variables / CLI flags) and
//! are never written back to any global config file. An empty mirror address
//! means "this mirror is not used".

use crate::config::{Config, MirrorMode};

/// Fully-resolved mirror configuration.
#[derive(Debug, Clone, Default)]
pub struct MirrorConfig {
    pub mode: MirrorMode,
    pub npm: Option<String>,
    pub cargo: Option<String>,
    pub nodejs_release: Option<String>,
    pub bun_download: Option<String>,
    pub github: Option<String>,
}

fn non_empty(s: &str) -> Option<String> {
    let t = s.trim();
    if t.is_empty() {
        None
    } else {
        Some(t.to_string())
    }
}

impl MirrorConfig {
    pub fn resolve(config: &Config) -> Self {
        let m = &config.mirrors;
        Self {
            mode: config.auto_mirror,
            npm: non_empty(&m.npm),
            cargo: non_empty(&m.cargo),
            nodejs_release: non_empty(&m.nodejs_release),
            bun_download: non_empty(&m.bun_download),
            github: non_empty(&m.github),
        }
    }

    /// Are mirrors enabled at all?
    pub fn enabled(&self) -> bool {
        self.mode != MirrorMode::Off
    }

    /// Is `force` mode active?
    pub fn forced(&self) -> bool {
        self.mode == MirrorMode::Force
    }

    /// Environment variables for npm (and bun, which reads the npm registry).
    pub fn npm_env(&self) -> Vec<(String, String)> {
        let mut env = Vec::new();
        if self.enabled()
            && let Some(reg) = &self.npm
        {
            env.push(("npm_config_registry".into(), reg.clone()));
            env.push(("NPM_CONFIG_REGISTRY".into(), reg.clone()));
            env.push(("BUN_CONFIG_REGISTRY".into(), reg.clone()));
        }
        env
    }

    /// Environment variables for nub. Its Node downloads honor the same
    /// NODEJS_ORG_MIRROR convention as fnm/nvm (verified in the 0.7.5
    /// binary), and its own registry ops read the npm registry config.
    pub fn nub_env(&self) -> Vec<(String, String)> {
        let mut v = Vec::new();
        if self.enabled() {
            if let Some(base) = &self.nodejs_release {
                v.push(("NODEJS_ORG_MIRROR".into(), base.clone()));
            }
            if let Some(reg) = &self.npm {
                v.push(("npm_config_registry".into(), reg.clone()));
                v.push(("NPM_CONFIG_REGISTRY".into(), reg.clone()));
            }
        }
        v
    }

    /// Environment variables for cargo (sparse crates.io index).
    pub fn cargo_env(&self) -> Vec<(String, String)> {
        let mut env = Vec::new();
        if self.enabled()
            && let Some(index) = &self.cargo
        {
            env.push(("CARGO_REGISTRIES_CRATES_IO_INDEX".into(), index.clone()));
            env.push((
                "CARGO_REGISTRIES_CRATES_IO_PROTOCOL".into(),
                "sparse".into(),
            ));
        }
        env
    }

    /// Environment variables for fnm's Node distribution download.
    pub fn fnm_env(&self) -> Vec<(String, String)> {
        let mut env = Vec::new();
        if self.enabled()
            && let Some(base) = &self.nodejs_release
        {
            env.push(("FNM_NODE_DIST_MIRROR".into(), base.clone()));
        }
        env
    }

    /// Environment variables for nvm's Node distribution download.
    pub fn nvm_env(&self) -> Vec<(String, String)> {
        let mut env = Vec::new();
        if self.enabled()
            && let Some(base) = &self.nodejs_release
        {
            env.push(("NVM_NODEJS_ORG_MIRROR".into(), base.clone()));
        }
        env
    }

    /// A human-readable summary of the active mirrors (for the UI / logs).
    pub fn summary(&self) -> Vec<(String, String)> {
        let mut v = Vec::new();
        if self.enabled() {
            if let Some(s) = &self.npm {
                v.push(("npm".into(), s.clone()));
            }
            if let Some(s) = &self.cargo {
                v.push(("cargo".into(), s.clone()));
            }
            if let Some(s) = &self.nodejs_release {
                v.push(("nodejs-release".into(), s.clone()));
            }
            if let Some(s) = &self.bun_download {
                v.push(("bun-download".into(), s.clone()));
            }
            if let Some(s) = &self.github {
                v.push(("github".into(), s.clone()));
            }
        }
        v
    }
}
