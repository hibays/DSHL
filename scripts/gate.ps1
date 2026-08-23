# dshl gate — the CI gate logic as a runnable script (single source of truth).
#
# Windows-native entry point; GitHub Actions (windows-* runners) and local
# Windows developers both call this. The Linux twin is scripts/gate.sh —
# keep the two step lists in sync.
#
# Usage:
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\gate.ps1          # everything
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\gate.ps1 -Rust    # fmt+clippy+test
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\gate.ps1 -Js      # node check + pack
#
# Exit code 0 iff every selected gate passed.

param(
    [switch]$Rust,
    [switch]$Js
)

$ErrorActionPreference = 'Continue'
if (-not $Rust -and -not $Js) { $Rust = $true; $Js = $true }

$script:failures = [System.Collections.Generic.List[string]]::new()

function Invoke-Gate {
    param([string]$Name, [scriptblock]$Body)
    Write-Host ""
    Write-Host "==> gate: $Name" -ForegroundColor Cyan
    & $Body
    if ($LASTEXITCODE -ne 0) {
        Write-Host "==> FAIL: $Name" -ForegroundColor Red
        $script:failures.Add($Name) | Out-Null
    } else {
        Write-Host "==> ok:   $Name" -ForegroundColor Green
    }
}

if ($Rust) {
    Invoke-Gate 'cargo fmt --all -- --check'      { cargo fmt --all -- --check }
    Invoke-Gate 'cargo clippy -D warnings'        { cargo clippy --workspace --all-targets -- -D warnings }
    Invoke-Gate 'cargo test --workspace --locked' { cargo test --workspace --locked }
}

if ($Js) {
    # Single source for the file list: package.json "check" (the same script
    # CI used to duplicate inline).
    Invoke-Gate 'npm run check'              { npm run check }
    Invoke-Gate 'npm pack --workspaces dry'  { npm pack --workspaces --dry-run }
}

Write-Host ""
if ($script:failures.Count -gt 0) {
    Write-Host ("GATE FAILED: " + ($script:failures -join ', ')) -ForegroundColor Red
    exit 1
}
Write-Host 'GATE PASSED' -ForegroundColor Green
exit 0
