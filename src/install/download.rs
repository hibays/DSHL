//! Zip download + extraction, registry-tarball helpers shared by the
//! installers (fnm auto-install, nub/bun registry installs), and the github
//! proxy prefix logic.

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::mirror::MirrorConfig;
use crate::platform;

use super::stream::run_streaming;

/// Download and extract a zip archive. The download goes through
/// [`http_download`] (resumable, retries), then extraction uses the platform's
/// built-in tools (PowerShell `Expand-Archive` on Windows, `unzip` on Unix).
pub(crate) async fn download_zip(url: &str, dest_dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dest_dir).map_err(|e| Error(e.to_string()))?;
    let tmp = dest_dir.join(".dshl-download.zip");
    http_download(url, &tmp).await?;
    let result = if platform::os() == platform::Os::Windows {
        let mut cmd = platform::shell_command();
        cmd.arg(format!(
            "Expand-Archive -Path '{zip}' -DestinationPath '{dest}' -Force",
            zip = tmp.display(),
            dest = dest_dir.display(),
        ));
        run_streaming(cmd, "extract").await
    } else {
        let mut cmd = platform::shell_command();
        cmd.arg(format!(
            "unzip -q -o '{zip}' -d '{dest}'",
            zip = tmp.display(),
            dest = dest_dir.display(),
        ));
        run_streaming(cmd, "extract").await
    };
    let _ = std::fs::remove_file(&tmp);
    result
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
    std::fs::create_dir_all(&bin).map_err(|e| {
        Error(
            t!(
                "install.download.mkdir_failed",
                path = bin.display(),
                err = e
            )
            .to_string(),
        )
    })?;

    // Asset names mirror upstream's own installer (.ci/install.sh): Windows
    // has a single asset for every arch (upstream ships no per-arch Windows
    // zips), macOS is universal via brew/zip, Linux splits arm64 out.
    let asset = match (platform::os(), platform::arch()) {
        (platform::Os::Windows, _) => "fnm-windows.zip",
        (platform::Os::Macos, _) => "fnm-macos.zip",
        (platform::Os::Linux, platform::Arch::Aarch64) => "fnm-arm64.zip",
        (platform::Os::Linux, _) => "fnm-linux.zip",
    };
    let original = format!("https://github.com/Schniz/fnm/releases/latest/download/{asset}");
    let url = proxied_github(mirror, &original);

    download_zip(&url, &bin).await?;

    let target = platform::with_ext("fnm");
    let located = locate_file(&bin, "fnm")
        .or_else(|| locate_file(&bin, &target))
        .ok_or_else(|| Error(t!("install.download.no_fnm").to_string()))?;

    let final_path = bin.join(target);
    if located != final_path {
        std::fs::copy(&located, &final_path)
            .map_err(|e| Error(t!("install.download.copy_failed", err = e).to_string()))?;
    }
    make_executable(&final_path);
    Ok(final_path)
}

// ---------------------------------------------------------------------------
// Registry-direct downloads (npm tarballs without spawning npm) — shared by
// the nub and bun installers.
// ---------------------------------------------------------------------------

/// Registry base URL (configured mirror wins; falls back to npmjs.org).
pub(crate) fn registry_base(mirror: &MirrorConfig) -> String {
    if mirror.enabled()
        && let Some(reg) = &mirror.npm
    {
        return reg.trim_end_matches('/').to_string();
    }
    "https://registry.npmjs.org".to_string()
}

/// Resumable HTTP(S) download: `-C -` continues an existing partial file and
/// the retry loop keeps trying up to three times. A failed resume drops the
/// partial so the next attempt starts clean instead of failing forever.
pub(crate) async fn http_download(url: &str, dest: &Path) -> Result<()> {
    let mut last: Option<Error> = None;
    for _ in 0..3 {
        let mut cmd = platform::shell_command();
        // On Windows, PowerShell aliases `curl` to `Invoke-WebRequest`,
        // which does not understand real curl flags. Use `curl.exe` to
        // hit the actual binary.
        let curl = if platform::os() == platform::Os::Windows {
            "curl.exe"
        } else {
            "curl"
        };
        cmd.arg(format!(
            "{curl} -fL -C - --retry 2 --retry-delay 2 -o {q}{dest}{q} {q}{url}{q}",
            q = '"',
            dest = dest.display(),
            url = url
        ));
        match run_streaming(cmd, "download").await {
            Ok(()) => return Ok(()),
            Err(e) => {
                let _ = std::fs::remove_file(dest);
                last = Some(e);
            }
        }
    }
    Err(last.unwrap_or(Error("download failed".into())))
}

/// Plain-text GET via curl (small JSON documents like `/latest`).
pub(crate) async fn http_get_text(url: &str) -> Result<String> {
    let tmp = std::env::temp_dir().join(format!("dshl-get-{}", std::process::id()));
    http_download(url, &tmp).await?;
    let text = std::fs::read_to_string(&tmp).map_err(|e| Error(e.to_string()))?;
    let _ = std::fs::remove_file(&tmp);
    Ok(text)
}

/// Pull the first `"key": "value"` string out of a small flat JSON document
/// without pulling in a JSON parser.
pub(crate) fn extract_json_string(json: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\":");
    let start = json.find(&needle)? + needle.len();
    let rest = json[start..].trim_start().strip_prefix('"')?;
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

/// Extract a .tgz into `dest_dir` using the system bsdtar/GNU tar.
pub(crate) async fn extract_tgz(tgz: &Path, dest_dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dest_dir).map_err(|e| Error(e.to_string()))?;
    let mut cmd = platform::shell_command();
    cmd.arg(format!(
        "tar -xzf {q}{tgz}{q} -C {q}{dest}{q}",
        q = '"',
        tgz = tgz.display(),
        dest = dest_dir.display()
    ));
    run_streaming(cmd, "extract").await
}

/// The registry tarball URL for a package version — single source of truth
/// shared by [`fetch_package_extracted`] and the installers' progress logs,
/// so a logged URL can never drift from the downloaded one.
pub(crate) fn package_tgz_url(mirror: &MirrorConfig, name: &str, version: &str) -> String {
    let base = registry_base(mirror);
    let short = short_name(name);
    format!("{base}/{name}/-/{short}-{version}.tgz")
}

/// Fetch an npm package tarball straight from the registry and extract it.
///
/// Returns the extracted `package/` directory (the tarball's single root).
/// `name` may be scoped (`@scope/pkg`) - the metadata endpoint uses %2F,
/// the tarball endpoint uses literal slashes (both are what registries serve).
pub(crate) async fn fetch_package_extracted(
    mirror: &MirrorConfig,
    name: &str,
    version: &str,
    stage_dir: &Path,
) -> Result<std::path::PathBuf> {
    let tgz_url = package_tgz_url(mirror, name, version);
    let tgz = stage_dir.join("pkg.tgz");
    http_download(&tgz_url, &tgz).await?;
    let out = stage_dir.join("extracted");
    extract_tgz(&tgz, &out).await?;
    let _ = std::fs::remove_file(&tgz);
    Ok(out.join("package"))
}

fn short_name(name: &str) -> String {
    name.rsplit('/').next().unwrap_or(name).to_string()
}
