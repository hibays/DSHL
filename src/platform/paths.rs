//! Home/cache/config directory discovery and executable lookup.

use std::env;
use std::path::{Path, PathBuf};

use super::detect::Os;
use super::detect::os;

/// Home directory.
pub fn home_dir() -> Option<PathBuf> {
    if let Ok(h) = env::var("USERPROFILE")
        && !h.is_empty()
    {
        return Some(PathBuf::from(h));
    }
    if let Ok(h) = env::var("HOME")
        && !h.is_empty()
    {
        return Some(PathBuf::from(h));
    }
    None
}

/// The launcher cache directory: `~/.cache/dshl`.
///
/// The `~/.cache/bin` convention from the fallback guide lives under
/// [`bin_dir`] and is intentionally the same on every platform so the fnm
/// manual-install instructions stay uniform.
pub fn cache_dir() -> PathBuf {
    if let Ok(c) = env::var("DSHL_CACHE")
        && !c.is_empty()
    {
        return PathBuf::from(c);
    }
    home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".cache")
}

/// `~/.cache/bin` — used for the fnm auto-install fallback.
pub fn bin_dir() -> PathBuf {
    cache_dir().join("bin")
}

/// The directory that holds `dshl.toml` by convention (config home).
pub fn config_dir() -> PathBuf {
    match os() {
        Os::Windows => env::var("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|_| home_dir().unwrap_or_default())
            .join("dshl"),
        Os::Macos => home_dir()
            .unwrap_or_default()
            .join("Library")
            .join("Application Support")
            .join("dshl"),
        Os::Linux => env::var("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| home_dir().unwrap_or_default().join(".config"))
            .join("dshl"),
    }
}

/// Executable extension for binaries on this platform.
pub fn executable_ext() -> &'static str {
    if cfg!(target_os = "windows") {
        ".exe"
    } else {
        ""
    }
}

/// Append the executable extension to `name` if the platform needs one.
pub fn with_ext(name: &str) -> String {
    format!("{name}{}", executable_ext())
}

/// Resolve a CLI tool to a runnable path/name.
///
/// On Windows, Node tools (`npm`, `npx`, `pnpm`, `pnpx`) are `.cmd` shims, and
/// `CreateProcess` only auto-finds `.exe`, so they must be resolved to their
/// `.cmd` path. Returns the full path when found, else the name (+ `.cmd`).
pub fn tool(name: &str) -> PathBuf {
    which(name).unwrap_or_else(|| {
        if cfg!(target_os = "windows") {
            PathBuf::from(format!("{name}.cmd"))
        } else {
            PathBuf::from(name)
        }
    })
}

/// Locate an executable by name on `extra_dirs` first, then `PATH` plus the
/// well-known tool locations.
///
/// `extra_dirs` lets callers find tools that were installed by an earlier
/// flow step (fnm's node bin, pnpm's global bin, …) even when those
/// directories are not on the ambient `PATH`.
pub fn which_in(name: &str, extra_dirs: &[PathBuf]) -> Option<PathBuf> {
    let candidates = if cfg!(target_os = "windows") {
        vec![
            with_ext(name),
            format!("{name}.cmd"),
            format!("{name}.bat"),
            name.to_string(),
        ]
    } else {
        vec![name.to_string()]
    };

    let mut dirs: Vec<PathBuf> = extra_dirs.to_vec();
    dirs.extend(search_dirs());
    for dir in dirs {
        for candidate in &candidates {
            let full = dir.join(candidate);
            if full.is_file() && is_executable(&full) {
                return Some(full);
            }
        }
    }
    None
}

/// Locate an executable by name on `PATH` plus the well-known tool locations.
pub fn which(name: &str) -> Option<PathBuf> {
    which_in(name, &[])
}

/// Directories searched by [`which`], in priority order.
fn search_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(path) = env::var_os("PATH") {
        dirs.extend(env::split_paths(&path));
    }
    dirs.extend(known_tool_dirs());
    dirs
}

/// Well-known install locations for the tools dshl manages.
pub fn known_tool_dirs() -> Vec<PathBuf> {
    let home = home_dir().unwrap_or_default();
    let mut dirs = vec![
        home.join(".bun").join("bin"),
        home.join(".fnm"),
        home.join(".local").join("bin"),
        bin_dir(),
        home.join(".nvm").join("versions").join("node"),
    ];
    if cfg!(target_os = "windows") {
        dirs.push(
            env::var("APPDATA")
                .map(PathBuf::from)
                .unwrap_or_default()
                .join("npm"),
        );
        dirs.push(
            env::var("APPDATA")
                .map(PathBuf::from)
                .unwrap_or_default()
                .join("fnm"),
        );
        // pnpm global bin: pnpm 10 used %LOCALAPPDATA%\pnpm, pnpm 11 uses
        // %LOCALAPPDATA%\pnpm\bin.
        let local = env::var("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_default();
        dirs.push(local.join("pnpm"));
        dirs.push(local.join("pnpm").join("bin"));
    } else {
        dirs.push(PathBuf::from("/usr/local/bin"));
        dirs.push(PathBuf::from("/opt/homebrew/bin"));
        dirs.push(home.join(".local").join("share").join("pnpm"));
    }
    dirs
}

/// pnpm's default global bin directory (pnpm 10/11 layout), used as a
/// fallback when `pnpm bin -g` cannot be queried.
pub fn default_pnpm_bin_dir() -> Option<PathBuf> {
    if cfg!(target_os = "windows") {
        env::var("LOCALAPPDATA")
            .ok()
            .map(|d| PathBuf::from(d).join("pnpm").join("bin"))
    } else {
        home_dir().map(|h| h.join(".local").join("share").join("pnpm"))
    }
}

/// Return the path of the current executable.
pub fn current_exe_dir() -> Option<PathBuf> {
    env::current_exe().ok()?.parent().map(|p| p.to_path_buf())
}

fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(path) {
            return meta.is_file() && meta.permissions().mode() & 0o111 != 0;
        }
        false
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}
