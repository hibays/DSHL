# dshl package — build the Track A installer/portable for the CURRENT host.
#
# Local/single-host twin of the release.yml packaging steps (the workflow adds
# cross-compilation + artifact upload on top). Requires the platform packer:
#   windows: NSIS (makensis on PATH or %ProgramFiles(x86)%\NSIS)
#   linux:   dpkg-deb (used by packing/linux/build-deb.sh)
#   macos:   hdiutil (used by packing/macos/build-dmg.sh)
#
# Usage:
#   scripts\package.ps1                  # release build + installer (+ portable zip on Windows)
#   scripts\package.ps1 -Version 0.3.0   # override version (default: workspace Cargo.toml)
#   scripts\package.ps1 -NoInstaller     # stage + portable only, skip NSIS

param(
    [string]$Version = '',
    [switch]$NoInstaller
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

if (-not $Version) {
    # First `version = "..."` inside the [workspace.package] table.
    $wsLine = (Select-String -Path Cargo.toml -Pattern '\[workspace\.package\]').LineNumber
    $line = Select-String -Path Cargo.toml -Pattern 'version\s*=\s*"([^"]+)"' |
        Where-Object { $_.LineNumber -gt $wsLine } | Select-Object -First 1
    if (-not $line) { Write-Error 'cannot resolve version from Cargo.toml'; exit 1 }
    $Version = $line.Matches[0].Groups[1].Value
}

$arch = if ($env:PROCESSOR_ARCHITECTURE -eq 'ARM64') { 'aarch64' } else { 'x86_64' }
Write-Host "== dshl package: version=$Version arch=$arch" -ForegroundColor Cyan

cargo build --release --locked -p dshl
if ($LASTEXITCODE -ne 0) { exit 1 }

# Installer staging: binary + READMEs only — dshl.toml ships ONLY in the
# portable zip (mirrors release.yml).
$stage = Join-Path $root 'stage'
if (Test-Path $stage) { Remove-Item -Recurse -Force $stage }
New-Item -ItemType Directory -Force -Path $stage | Out-Null
Copy-Item target/release/dshl.exe $stage/
Copy-Item README.md $stage/
Copy-Item README_en.md $stage/

    # $IsWindows does not exist in Windows PowerShell 5.1 (StrictMode would
    # throw on the undefined variable) — $env:OS is present everywhere.
    if ($env:OS -eq 'Windows_NT') {
    # Portable zip: stage contents + a default dshl.toml.
    $portable = Join-Path $root 'portable'
    if (Test-Path $portable) { Remove-Item -Recurse -Force $portable }
    New-Item -ItemType Directory -Force -Path $portable | Out-Null
    Copy-Item "$stage/*" $portable
    Copy-Item dshl.example.toml (Join-Path $portable 'dshl.toml')
    Compress-Archive -Force -Path "$portable/*" -DestinationPath "dshl-$Version-windows-$arch.zip"
    Remove-Item -Recurse -Force $portable
    Write-Host "== wrote dshl-$Version-windows-$arch.zip" -ForegroundColor Green

    if (-not $NoInstaller) {
        Copy-Item packing/windows/dsh.ico $stage/
        $makensis = (Get-Command makensis -ErrorAction SilentlyContinue).Source
        if (-not $makensis) {
            $makensis = Join-Path ${env:ProgramFiles(x86)} 'NSIS\makensis.exe'
        }
        if (-not (Test-Path $makensis)) { Write-Error 'makensis not found'; exit 1 }
        & $makensis -V3 `
            "-DSTAGE_DIR=$stage" `
            "-DPRODUCT_VERSION=$Version" `
            "-DOUTFILE=$root\dshl-$Version-windows-$arch-setup.exe" `
            packing/windows/dshl.nsi
        if ($LASTEXITCODE -ne 0) { exit 1 }
        Write-Host "== wrote dshl-$Version-windows-$arch-setup.exe" -ForegroundColor Green
    }
}
else {
    Write-Error 'Use scripts/package.sh on Linux/macOS.'
}
