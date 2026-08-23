# dshl publish — npm publishing for the Track B aggregator packages.
#
# Local twin of release-plugins.yml (keep the steps in sync). Publishes
# @dshl/native → @dshl/pipe → @dshl/control in dependency order.
#
# NOT covered here: the six @dshl/native-<platform>-<arch> subpackages — they
# need a per-platform .node build matrix and are workflow-only
# (release-native.yml). Locally, `npm run build:native` drops the host .node
# into plugins/dshl-native/native/, which the loader prefers over the
# published subpackages anyway.
#
# Usage:
#   scripts\publish.ps1 -Version 0.3.0            # bump + verify + publish
#   scripts\publish.ps1 -Version 0.3.0 -DryRun    # bump + verify only
#   scripts\publish.ps1                           # publish current versions, no bump
#   scripts\publish.ps1 ... -Provenance           # add --provenance (needs OIDC; CI only)

param(
    [string]$Version = '',
    [switch]$DryRun,
    [switch]$Provenance
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

if ($Version) {
    Write-Host "== bumping @dshl/* to $Version (optionalDependencies pinned to ^$Version)" -ForegroundColor Cyan
    node scripts/bump-versions.mjs $Version
    if ($LASTEXITCODE -ne 0) { exit 1 }
}
else {
    Write-Host '== no -Version given: publishing current package.json versions' -ForegroundColor Yellow
}

npm run check
if ($LASTEXITCODE -ne 0) { exit 1 }
npm pack --workspaces --dry-run
if ($LASTEXITCODE -ne 0) { exit 1 }

if ($DryRun) {
    Write-Host '== -DryRun: skipping npm publish' -ForegroundColor Yellow
    exit 0
}

$prov = @()
if ($Provenance) { $prov = @('--provenance') }

# Control must come LAST: it lists native+pipe as optionalDependencies and npm
# rejects publish if their published versions are not yet resolvable.
foreach ($pkg in 'dshl-native', 'dshl-pipe', 'dshl-control') {
    Push-Location "plugins/$pkg"
    npm publish --access public @prov
    $code = $LASTEXITCODE
    Pop-Location
    if ($code -ne 0) { Write-Error "npm publish failed for $pkg"; exit 1 }
}
Write-Host '== published' -ForegroundColor Green
