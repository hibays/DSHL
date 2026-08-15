//! Zip download + extraction and small file helpers shared by the installers
//! (fnm auto-install, bun download) and the github proxy prefix logic.

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::mirror::MirrorConfig;
use crate::platform;

use super::stream::run_streaming;

/// Download and extract a zip archive using the platform's built-in tools.
pub(crate) async fn download_zip(url: &str, dest_dir: &Path) -> Result<()> {
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

/// Locate a file named `name` (or with `.exe`) anywhere under `dir`.
pub(crate) fn locate_file(dir: &Path, name: &str) -> Option<PathBuf> {
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

/// Mark a file executable on unix.
pub(crate) fn make_executable(path: &Path) {
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

/// Prepend the `github` proxy prefix to a github URL when one is configured.
///
/// A prefix such as `https://ghproxy.com/` is joined to the github URL with a
/// single `/` (trailing slashes are normalised away first), so both
/// `https://ghproxy.com` and `https://ghproxy.com/` produce
/// `https://ghproxy.com/https://github.com/...`.
pub(crate) fn proxied_github(mirror: &MirrorConfig, original: &str) -> String {
    match &mirror.github {
        Some(g) if mirror.enabled() => format!("{}/{}", g.trim_end_matches('/'), original),
        _ => original.to_string(),
    }
}

/// Download the fnm release binary into `~/.cache/bin` (the last resort of the
/// node install chain, see [`super::node`]).
pub(crate) async fn install_fnm_binary(mirror: &MirrorConfig) -> Result<PathBuf> {
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
