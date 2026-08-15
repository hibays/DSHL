//! Runtime installation and the fallback chain.
//!
//! Importance order (highest first): nodejs → bun → fnm → cargo → nvm.
//!   * nodejs is **required** (dsh runs on Node); min 24.15.0, we install 26.
//!   * bun is installed only when the config's `pm`/`exector` asks for it.
//!   * node 26 is installed via fnm first, then `cargo install fnm`, then nvm,
//!     then a best-effort fnm auto-install into `~/.cache/bin`; if everything
//!     fails the UI is told to install fnm manually.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::config::Config;
use crate::error::{Error, Result};
use crate::mirror::MirrorConfig;
use crate::platform;
use crate::probe;
use crate::process::{self, AsyncChild, Output};
use crate::progress;
use crate::version::Version;

/// Minimum Node.js version required by dsh.
pub const NODE_MIN: Version = Version::new(24, 15, 0);
/// Minimum bun version.
pub const BUN_MIN: Version = Version::new(1, 3, 14);
/// Node.js major version to install when missing.
pub const NODE_INSTALL_VERSION: &str = "26";
/// fnm manual-install guide shown when every fallback fails.
pub const FNM_GUIDE_URL: &str = "https://www.fnmnode.com/zh-cn/guide/install";

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

/// Stream a command's output into the progress log and fail on non-zero exit.
pub async fn run_streaming(mut cmd: Command, label: &str) -> Result<()> {
    let child =
        AsyncChild::spawn(&mut cmd).map_err(|e| Error(format!("failed to start {label}: {e}")))?;
    while let Some(line) = child.next_line().await {
        match line {
            Output::Stdout(l) => {
                let t = l.trim();
                if !t.is_empty() {
                    progress::log(t);
                }
            }
            Output::Stderr(l) => {
                let t = l.trim();
                if !t.is_empty() {
                    progress::log(t);
                }
            }
        }
    }
    match child.exit_code() {
        Some(0) => Ok(()),
        Some(code) => Err(Error(format!("{label} failed (exit {code})"))),
        None => Err(Error(format!("{label} exited without a status"))),
    }
}

/// Ensure Node.js is present and recent enough. Returns the directory that
/// contains the `node` executable.
///
/// Node is always required — dsh runs on it regardless of the chosen `pm` or
/// `exector` — so this does not depend on the config.
pub async fn ensure_node(mirror: &MirrorConfig) -> Result<PathBuf> {
    let node = probe::node();
    if node.found {
        if let Some(v) = node.version {
            if v >= NODE_MIN {
                let dir = node
                    .path
                    .as_ref()
                    .and_then(|p| p.parent())
                    .map(|p| p.to_path_buf());
                if let Some(d) = dir {
                    progress::log(format!("node {v} 满足要求 (>= {NODE_MIN})"));
                    return Ok(d);
                }
            } else {
                progress::log(format!(
                    "node {v} 过旧 (需要 >= {NODE_MIN})，将安装 {NODE_INSTALL_VERSION}"
                ));
            }
        } else {
            progress::log("已找到 node 但无法解析版本，将重新安装".to_string());
        }
    } else {
        progress::log("未找到 node，需要安装".to_string());
    }
    install_node(mirror).await
}

/// Install node `NODE_INSTALL_VERSION` through the fallback chain.
async fn install_node(mirror: &MirrorConfig) -> Result<PathBuf> {
    progress::log(format!("开始安装 Node.js {NODE_INSTALL_VERSION}"));

    // 1. fnm already present
    if let Some(fnm) = platform::which("fnm") {
        progress::log(format!("使用现有 fnm ({})", fnm.display()));
        if let Ok(dir) = install_node_with_fnm(&fnm, mirror).await {
            return Ok(dir);
        }
        progress::log("fnm 安装失败，尝试下一级回退".to_string());
    }

    // 2. cargo install fnm
    if probe::cargo().found {
        progress::log("尝试 cargo install fnm".to_string());
        match install_fnm_via_cargo(mirror).await {
            Ok(fnm) => {
                if let Ok(dir) = install_node_with_fnm(&fnm, mirror).await {
                    return Ok(dir);
                }
                progress::log("cargo 安装的 fnm 安装 node 失败，继续回退".to_string());
            }
            Err(e) => progress::log(format!("cargo install fnm 失败: {e}")),
        }
    }

    // 3. nvm
    if probe::nvm().found {
        progress::log("尝试使用 nvm 安装 node".to_string());
        if let Ok(dir) = install_node_with_nvm(mirror).await {
            return Ok(dir);
        }
        progress::log("nvm 安装失败，继续回退".to_string());
    }

    // 4. best-effort auto-install fnm into ~/.cache/bin
    match install_fnm_binary(mirror).await {
        Ok(fnm) => {
            progress::log(format!(
                "已自动安装 fnm 到 {}，继续安装 node",
                fnm.display()
            ));
            if let Ok(dir) = install_node_with_fnm(&fnm, mirror).await {
                return Ok(dir);
            }
        }
        Err(e) => progress::log(format!("fnm 自动安装失败: {e}")),
    }

    Err(Error(format!(
        "无法自动安装 Node.js {NODE_INSTALL_VERSION}。\n请手动安装 fnm（{}）到 ~/.cache/bin 后重新启动。",
        FNM_GUIDE_URL
    )))
}

async fn install_node_with_fnm(fnm: &Path, mirror: &MirrorConfig) -> Result<PathBuf> {
    let mut cmd = Command::new(fnm);
    cmd.args(["install", NODE_INSTALL_VERSION]);
    process::with_env(&mut cmd, &mirror.fnm_env());
    run_streaming(cmd, "fnm install").await?;

    if let Some(dir) = find_node_bin() {
        progress::log(format!("node 安装完成，位于 {}", dir.display()));
        return Ok(dir);
    }
    Err(Error("fnm 安装 node 成功，但未能定位 node 目录".into()))
}

async fn install_fnm_via_cargo(mirror: &MirrorConfig) -> Result<PathBuf> {
    let mut cmd = Command::new("cargo");
    cmd.args(["install", "fnm"]);
    process::with_env(&mut cmd, &mirror.cargo_env());
    run_streaming(cmd, "cargo install fnm").await?;

    // fnm lands in ~/.cargo/bin
    if let Some(home) = platform::home_dir() {
        let candidate = home
            .join(".cargo")
            .join("bin")
            .join(platform::with_ext("fnm"));
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    if let Some(p) = platform::which("fnm") {
        return Ok(p);
    }
    Err(Error(
        "cargo install fnm 成功，但找不到 fnm 可执行文件".into(),
    ))
}

async fn install_node_with_nvm(mirror: &MirrorConfig) -> Result<PathBuf> {
    if platform::os() == platform::Os::Windows {
        let mut cmd = Command::new("nvm");
        cmd.args(["install", NODE_INSTALL_VERSION]);
        process::with_env(&mut cmd, &mirror.nvm_env());
        run_streaming(cmd, "nvm install").await?;

        let mut use_cmd = Command::new("nvm");
        use_cmd.args(["use", NODE_INSTALL_VERSION]);
        run_streaming(use_cmd, "nvm use").await?;
    } else {
        // nvm is a shell function; source its script first.
        let nvm_sh = probe::nvm().path.unwrap_or_else(|| {
            platform::home_dir()
                .unwrap_or_default()
                .join(".nvm")
                .join("nvm.sh")
        });
        let script = format!(
            "source '{}' >/dev/null 2>&1 && nvm install {} && nvm use {}",
            nvm_sh.display(),
            NODE_INSTALL_VERSION,
            NODE_INSTALL_VERSION
        );
        let mut cmd = platform::shell_command();
        cmd.arg(script);
        process::with_env(&mut cmd, &mirror.nvm_env());
        run_streaming(cmd, "nvm install").await?;
    }

    if let Some(dir) = find_node_bin() {
        return Ok(dir);
    }
    Err(Error("nvm 安装 node 成功，但未能定位 node 目录".into()))
}

/// Download the fnm release binary into `~/.cache/bin`.
async fn install_fnm_binary(mirror: &MirrorConfig) -> Result<PathBuf> {
    let bin = platform::bin_dir();
    std::fs::create_dir_all(&bin)
        .map_err(|e| Error(format!("创建目录失败 {}: {e}", bin.display())))?;

    let asset = match platform::os() {
        platform::Os::Windows => "fnm-windows.zip",
        platform::Os::Macos => "fnm-macos.zip",
        platform::Os::Linux => "fnm-linux.zip",
    };
    let original = format!("https://github.com/Schniz/fnm/releases/latest/download/{asset}");
    let url = proxied_github(mirror, &original);

    download_zip(&url, &bin).await?;

    let target = platform::with_ext("fnm");
    let located = locate_file(&bin, "fnm")
        .or_else(|| locate_file(&bin, &target))
        .ok_or_else(|| Error("fnm 下载解压完成，但未找到 fnm 二进制".into()))?;

    let final_path = bin.join(target);
    if located != final_path {
        std::fs::copy(&located, &final_path).map_err(|e| Error(format!("复制 fnm 失败: {e}")))?;
    }
    make_executable(&final_path);
    Ok(final_path)
}

/// Ensure bun is installed when the config requires it.
pub async fn ensure_bun(config: &Config, mirror: &MirrorConfig) -> Result<Option<PathBuf>> {
    if !config.dsh.needs_bun() {
        return Ok(None);
    }

    let bun = probe::bun();
    if bun.found {
        if let Some(v) = bun.version {
            if v >= BUN_MIN {
                let dir = bun
                    .path
                    .as_ref()
                    .and_then(|p| p.parent())
                    .map(|p| p.to_path_buf());
                progress::log(format!("bun {v} 满足要求 (>= {BUN_MIN})"));
                return Ok(dir);
            }
            progress::log(format!("bun {v} 过旧 (需要 >= {BUN_MIN})，将安装新版"));
        }
    } else {
        progress::log("未找到 bun，需要安装".to_string());
    }

    install_bun(mirror).await.map(Some)
}

/// Ensure pnpm is installed when the config requires it (`pm = "pnpm"` or
/// `exector = "pnpx"`).
///
/// pnpm is installed through `npm i -g pnpm` when missing — node (and with it
/// npm) is always present at this point. Returns the pnpm global bin
/// directory/ies (where `pnpm add -g` links executables) so the caller can
/// prepend them to PATH; a freshly installed pnpm is not on the ambient PATH
/// and neither is the dsh it links.
pub async fn ensure_pnpm(
    config: &Config,
    mirror: &MirrorConfig,
    node_dir: &Path,
) -> Result<Vec<PathBuf>> {
    if !config.dsh.needs_pnpm() {
        return Ok(Vec::new());
    }

    let pnpm = probe::pnpm();
    if pnpm.found {
        match pnpm.version {
            Some(v) => progress::log(format!("pnpm {v} 满足要求")),
            None => {
                let path = pnpm
                    .path
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default();
                progress::log(format!("已找到 pnpm ({path})"));
            }
        }
    } else {
        progress::log("未找到 pnpm，通过 npm 安装".to_string());
        let mut cmd = Command::new(platform::tool("npm"));
        cmd.args(["install", "-g", "pnpm"]);
        // The npm of a freshly installed fnm node lives in `node_dir`, which
        // is not on the ambient PATH — augment it so the install works and
        // the resulting pnpm is resolvable by the next step.
        let rt = Runtime {
            node_dir: Some(node_dir.to_path_buf()),
            bun_dir: None,
            extra_path: Vec::new(),
        };
        cmd.env("PATH", rt.augmented_path());
        process::with_env(&mut cmd, &mirror.npm_env());
        run_streaming(cmd, "npm i -g pnpm").await?;
    }

    Ok(pnpm_bin_dirs(node_dir))
}

/// Resolve pnpm's global bin directory (where `pnpm add -g` links bins).
///
/// `pnpm bin -g` is authoritative, but on machines where that directory is
/// not on PATH it exits non-zero and prints an error that still quotes the
/// configured path. Fallback chain: printed line → path quoted in the error
/// → platform default. Directories that do not exist yet are created so a
/// freshly installed pnpm is findable right after the first `add -g`.
fn pnpm_bin_dirs(node_dir: &Path) -> Vec<PathBuf> {
    let rt = Runtime {
        node_dir: Some(node_dir.to_path_buf()),
        bun_dir: None,
        extra_path: Vec::new(),
    };
    let mut cmd = Command::new(platform::tool("pnpm"));
    cmd.args(["bin", "-g"]);
    cmd.env("PATH", rt.augmented_path());
    let text = process::run(&mut cmd)
        .map(|res| format!("{}{}", res.stdout, res.stderr))
        .unwrap_or_default();

    let mut dirs: Vec<PathBuf> = Vec::new();
    let push_dir = |dirs: &mut Vec<PathBuf>, dir: PathBuf| {
        if dirs.iter().all(|d| d != &dir) {
            let _ = std::fs::create_dir_all(&dir);
            dirs.push(dir);
        }
    };

    // 1. Success: the whole first line is the path.
    let first = text
        .lines()
        .next()
        .unwrap_or("")
        .trim()
        .trim_matches('"')
        .trim();
    if looks_like_dir(first) {
        push_dir(&mut dirs, PathBuf::from(first));
    }
    // 2. Error output still quotes the configured path, e.g.
    //    [ERROR] The configured global bin directory "C:..." is not in PATH
    if dirs.is_empty()
        && let Some(start) = text.find('"')
        && let Some(rel) = text[start + 1..].find('"')
    {
        let quoted = text[start + 1..start + 1 + rel].trim();
        if looks_like_dir(quoted) {
            push_dir(&mut dirs, PathBuf::from(quoted));
        }
    }
    // 3. Platform defaults (both the pnpm 10 and pnpm 11 Windows layouts).
    if cfg!(windows)
        && let Some(base) = std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
    {
        push_dir(&mut dirs, base.join("pnpm").join("bin"));
        push_dir(&mut dirs, base.join("pnpm"));
    } else {
        let home = platform::home_dir().unwrap_or_default();
        push_dir(&mut dirs, home.join(".local").join("share").join("pnpm"));
    }
    dirs
}

/// A crude "this looks like an absolute path" check used to pick the pnpm
/// bin dir out of command output.
fn looks_like_dir(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    if cfg!(windows) {
        let b = s.as_bytes();
        (b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':')
            || s.starts_with("\\")
            || s.starts_with('/')
    } else {
        s.starts_with('/') || s.starts_with("~")
    }
}

async fn install_bun(mirror: &MirrorConfig) -> Result<PathBuf> {
    let install_dir = platform::cache_dir().join("bun");
    let bin = install_dir.join("bin");
    std::fs::create_dir_all(&install_dir).map_err(|e| Error(e.to_string()))?;

    // 1. Direct binary download. Resolution order:
    //    a) `bun-download` mirror (highest priority),
    //    b) github through the `github` proxy prefix (when configured),
    //    c) github directly.
    let url = bun_download_url(mirror);
    progress::log(format!("下载 bun: {url}"));
    if let Ok(()) = download_zip(&url, &install_dir).await
        && let Some(found) = locate_file(&install_dir, "bun")
    {
        let dest = bin.join(platform::with_ext("bun"));
        std::fs::create_dir_all(&bin).ok();
        if found != dest {
            let _ = std::fs::copy(&found, &dest);
        }
        make_executable(&dest);
        if dest.is_file() {
            return Ok(bin);
        }
    }
    progress::log("bun 直连下载失败，回退到官方脚本".to_string());

    // 2. Official install script.
    let script = if platform::os() == platform::Os::Windows {
        format!(
            "$env:BUN_INSTALL = '{}'; irm bun.sh/install.ps1 | iex",
            install_dir.display()
        )
    } else {
        format!(
            "export BUN_INSTALL='{}'; curl -fsSL https://bun.sh/install | bash",
            install_dir.display()
        )
    };
    let mut cmd = platform::shell_command();
    cmd.arg(script);
    process::with_env(&mut cmd, &mirror.npm_env());
    let _ = run_streaming(cmd, "bun install").await;

    if bin.join(platform::with_ext("bun")).is_file() {
        return Ok(bin);
    }

    // 3. npm fallback (respects the npm registry mirror).
    progress::log("官方脚本未成功，尝试 npm i -g bun".to_string());
    let mut npm = Command::new(platform::tool("npm"));
    npm.args(["install", "-g", "bun"]);
    process::with_env(&mut npm, &mirror.npm_env());
    run_streaming(npm, "npm i -g bun").await?;
    if let Some(p) = platform::which("bun")
        && let Some(parent) = p.parent()
    {
        return Ok(parent.to_path_buf());
    }

    Err(Error("bun 安装失败".into()))
}

/// Resolve the direct bun binary download URL.
///
/// `bun-download` takes priority when set; otherwise bun is fetched from its
/// GitHub release, going through the `github` proxy prefix when one is
/// configured (empty `github` = download from github directly).
fn bun_download_url(mirror: &MirrorConfig) -> String {
    let target = bun_zip_target();
    if mirror.enabled()
        && let Some(base) = &mirror.bun_download
    {
        return format!("{}/{}", base.trim_end_matches('/'), target);
    }
    let original = format!("https://github.com/oven-sh/bun/releases/latest/download/{target}");
    proxied_github(mirror, &original)
}

/// Prepend the `github` proxy prefix to a github URL when one is configured.
///
/// A prefix such as `https://ghproxy.com/` is joined to the github URL with a
/// single `/` (trailing slashes are normalised away first), so both
/// `https://ghproxy.com` and `https://ghproxy.com/` produce
/// `https://ghproxy.com/https://github.com/...`.
fn proxied_github(mirror: &MirrorConfig, original: &str) -> String {
    match &mirror.github {
        Some(g) if mirror.enabled() => format!("{}/{}", g.trim_end_matches('/'), original),
        _ => original.to_string(),
    }
}

fn bun_zip_target() -> &'static str {
    match (platform::os(), platform::arch()) {
        (platform::Os::Windows, _) => "bun-windows-x64.zip",
        (platform::Os::Macos, platform::Arch::Aarch64) => "bun-darwin-aarch64.zip",
        (platform::Os::Macos, _) => "bun-darwin-x64.zip",
        (platform::Os::Linux, platform::Arch::Aarch64) => "bun-linux-aarch64.zip",
        (platform::Os::Linux, _) => "bun-linux-x64.zip",
    }
}

/// Recursively search for a directory containing `node`/`node.exe` under the
/// fnm/nvm install roots.
fn find_node_bin() -> Option<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Some(home) = platform::home_dir() {
        roots.push(home.join(".fnm").join("node-versions"));
        roots.push(home.join(".nvm").join("versions").join("node"));
    }
    if let Ok(appdata) = std::env::var("APPDATA") {
        let appdata = PathBuf::from(appdata);
        roots.push(appdata.join("fnm").join("node-versions"));
        roots.push(appdata.join("nvm"));
    }
    for root in roots {
        if let Some(dir) = find_node_in(&root, 0) {
            return Some(dir);
        }
    }
    None
}

fn find_node_in(dir: &Path, depth: u32) -> Option<PathBuf> {
    if depth > 6 {
        return None;
    }
    let name = platform::with_ext("node");
    if dir.join(&name).is_file() {
        return Some(dir.to_path_buf());
    }
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir()
            && let Some(found) = find_node_in(&path, depth + 1)
        {
            return Some(found);
        }
    }
    None
}

/// Locate a file named `name` (or with `.exe`) anywhere under `dir`.
fn locate_file(dir: &Path, name: &str) -> Option<PathBuf> {
    if !dir.is_dir() {
        return None;
    }
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        if let Ok(entries) = std::fs::read_dir(&current) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if let Some(file_name) = path.file_name().and_then(|n| n.to_str())
                    && (file_name == name || file_name == platform::with_ext(name))
                {
                    return Some(path);
                }
            }
        }
    }
    None
}

/// Download and extract a zip archive using the platform's built-in tools.
async fn download_zip(url: &str, dest_dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dest_dir).map_err(|e| Error(e.to_string()))?;
    let tmp = dest_dir.join(".dshl-download.zip");
    let script = if platform::os() == platform::Os::Windows {
        format!(
            "[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12; \
             Invoke-WebRequest -Uri '{}' -OutFile '{}'; \
             Expand-Archive -Path '{}' -DestinationPath '{}' -Force; \
             Remove-Item '{}' -ErrorAction SilentlyContinue",
            url,
            tmp.display(),
            tmp.display(),
            dest_dir.display(),
            tmp.display()
        )
    } else {
        format!(
            "curl -fsSL '{}' -o '{}' && unzip -q -o '{}' -d '{}' && rm -f '{}'",
            url,
            tmp.display(),
            tmp.display(),
            dest_dir.display(),
            tmp.display()
        )
    };
    let mut cmd = platform::shell_command();
    cmd.arg(script);
    run_streaming(cmd, "download").await
}

/// Mark a file executable on unix.
fn make_executable(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(path) {
            let mut perms = meta.permissions();
            perms.set_mode(perms.mode() | 0o111);
            let _ = std::fs::set_permissions(path, perms);
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::MirrorMode;

    fn mirror(bun_download: &str, github: &str, mode: MirrorMode) -> MirrorConfig {
        MirrorConfig {
            mode,
            npm: None,
            cargo: None,
            nodejs_release: None,
            bun_download: if bun_download.is_empty() {
                None
            } else {
                Some(bun_download.into())
            },
            github: if github.is_empty() {
                None
            } else {
                Some(github.into())
            },
        }
    }

    #[test]
    fn bun_url_prefers_bun_download() {
        let url = bun_download_url(&mirror("https://mirror.example/bun", "", MirrorMode::On));
        assert!(url.starts_with("https://mirror.example/bun/"));
        assert!(url.ends_with(bun_zip_target()));
    }

    #[test]
    fn bun_url_uses_github_proxy_when_bun_download_empty() {
        let url = bun_download_url(&mirror("", "https://ghproxy.example/", MirrorMode::On));
        assert!(url.starts_with(
            "https://ghproxy.example/https://github.com/oven-sh/bun/releases/latest/download/"
        ));
    }

    #[test]
    fn bun_url_falls_back_to_direct_github() {
        let url = bun_download_url(&mirror("", "", MirrorMode::On));
        assert_eq!(
            url,
            format!(
                "https://github.com/oven-sh/bun/releases/latest/download/{}",
                bun_zip_target()
            )
        );
    }

    #[test]
    fn bun_url_ignores_mirrors_when_off() {
        let url = bun_download_url(&mirror(
            "https://mirror.example/bun",
            "https://ghproxy.example/",
            MirrorMode::Off,
        ));
        assert_eq!(
            url,
            format!(
                "https://github.com/oven-sh/bun/releases/latest/download/{}",
                bun_zip_target()
            )
        );
    }
}
