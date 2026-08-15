//! Bun: [`ensure_bun`] and the install fallback chain.
//!
//! Bun is installed only when the config's `pm`/`exector` asks for it. Chain:
//! direct binary download (bun-download mirror → github proxy → github) →
//! official install script → `npm i -g bun` (respects the npm registry
//! mirror).

use std::path::PathBuf;
use std::process::Command;

use crate::config::Config;
use crate::error::{Error, Result};
use crate::mirror::MirrorConfig;
use crate::platform;
use crate::probe;
use crate::process;
use crate::progress;

use super::BUN_MIN;
use super::download;
use super::stream::run_streaming;

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
    if let Ok(()) = download::download_zip(&url, &install_dir).await
        && let Some(found) = download::locate_file(&install_dir, "bun")
    {
        let dest = bin.join(platform::with_ext("bun"));
        std::fs::create_dir_all(&bin).ok();
        if found != dest {
            let _ = std::fs::copy(&found, &dest);
        }
        download::make_executable(&dest);
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
    download::proxied_github(mirror, &original)
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
