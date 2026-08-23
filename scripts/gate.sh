#!/usr/bin/env bash
# dshl gate — the CI gate logic as a runnable script (single source of truth).
#
# Linux/macOS twin of scripts/gate.ps1 — keep the two step lists in sync.
# GitHub Actions (ubuntu runners) and local Unix developers both call this.
#
# Usage:
#   scripts/gate.sh           # everything
#   scripts/gate.sh --rust    # cargo fmt + clippy + test
#   scripts/gate.sh --js      # npm run check + pack dry-run
#
# Exit code 0 iff every selected gate passed.
set -u

RUN_RUST=0
RUN_JS=0
if [ $# -eq 0 ]; then
  RUN_RUST=1
  RUN_JS=1
fi
for arg in "$@"; do
  case "$arg" in
    --rust) RUN_RUST=1 ;;
    --js) RUN_JS=1 ;;
    *) echo "unknown flag: $arg (use --rust / --js)" >&2; exit 2 ;;
  esac
done

FAILURES=()

gate() {
  local name="$1"; shift
  printf '\n==> gate: %s\n' "$name"
  if "$@"; then
    printf '==> ok:   %s\n' "$name"
  else
    printf '==> FAIL: %s\n' "$name" >&2
    FAILURES+=("$name")
  fi
}

if [ "$RUN_RUST" -eq 1 ]; then
  gate 'cargo fmt --all -- --check'      cargo fmt --all -- --check
  gate 'cargo clippy -D warnings'        cargo clippy --workspace --all-targets -- -D warnings
  gate 'cargo test --workspace --locked' cargo test --workspace --locked
fi

if [ "$RUN_JS" -eq 1 ]; then
  # Single source for the file list: package.json "check" (the same script
  # CI used to duplicate inline).
  gate 'npm run check'              npm run check
  gate 'npm pack --workspaces dry'  npm pack --workspaces --dry-run
fi

printf '\n'
if [ "${#FAILURES[@]}" -gt 0 ]; then
  printf 'GATE FAILED: %s\n' "${FAILURES[*]}" >&2
  exit 1
fi
printf 'GATE PASSED\n'
