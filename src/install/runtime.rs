//! The resolved runtime model: which directories to prepend to `PATH` when
//! launching dsh.

use std::path::PathBuf;

/// Resolved runtime binaries, as directories to prepend to `PATH`.
#[derive(Debug, Clone, Default)]
pub struct Runtime {
    pub node_dir: Option<PathBuf>,
    pub bun_dir: Option<PathBuf>,
    pub extra_path: Vec<PathBuf>,
}

impl Runtime {
    /// Directories to prepend to `PATH` when launching dsh.
    pub fn path_prefix(&self) -> Vec<PathBuf> {
        let mut v = Vec::new();
        if let Some(d) = &self.node_dir {
            v.push(d.clone());
        }
        if let Some(d) = &self.bun_dir {
            v.push(d.clone());
        }
        v.extend(self.extra_path.iter().cloned());
        v
    }

    /// An augmented `PATH` value (existing PATH plus the prefix).
    pub fn augmented_path(&self) -> std::ffi::OsString {
        let mut parts: Vec<std::ffi::OsString> = self
            .path_prefix()
            .into_iter()
            .map(|p| p.into_os_string())
            .collect();
        if let Some(existing) = std::env::var_os("PATH") {
            parts.push(existing);
        }
        std::env::join_paths(parts).unwrap_or_else(|_| std::env::var_os("PATH").unwrap_or_default())
    }
}
