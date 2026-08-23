#!/usr/bin/env bash
# dshl publish — npm publishing for the Track B aggregator packages.
#
# Linux/macOS twin of scripts/publish.ps1 (keep in sync). Publishes
# @dshl/native → @dshl/pipe → @dshl/control in dependency order.
#
# NOT covered: the six @dshl/native-<platform>-<arch> subpackages (workflow-
# only, release-native.yml). Locally `npm run build:native` provides the host
# .node, which the loader prefers over published subpackages anyway.
#
# Usage:
#   scripts/publish.sh --version 0.3.0           # bump + verify + publish
#   scripts/publish.sh --version 0.3.0 --dry-run # bump + verify only
#   scripts/publish.sh                           # publish current versions
#   scripts/publish.sh ... --provenance          # needs OIDC; CI-only
set -euo pipefail

VERSION=""
DRYRUN=0
PROV=()
while [ $# -gt 0 ]; do
  case "$1" in
    --version) VERSION="$2"; shift 2 ;;
    --dry-run) DRYRUN=1; shift ;;
    --provenance) PROV+=(--provenance); shift ;;
    *) echo "unknown flag: $1" >&2; exit 2 ;;
  esac
done

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

if [ -n "$VERSION" ]; then
  echo "== bumping @dshl/* to $VERSION"
  node scripts/bump-versions.mjs "$VERSION"
else
  echo "== no --version given: publishing current package.json versions"
fi

npm run check
npm pack --workspaces --dry-run

if [ "$DRYRUN" -eq 1 ]; then
  echo "== --dry-run: skipping npm publish"
  exit 0
fi

# Control must come LAST (optionalDependencies resolvability, see workflow).
for pkg in dshl-native dshl-pipe dshl-control; do
  (cd "plugins/$pkg" && npm publish --access public ${PROV[@]+"${PROV[@]}"})
done
echo '== published'
