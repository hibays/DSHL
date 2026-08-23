#!/usr/bin/env bash
# dshl package — Track A packaging steps, usable BOTH end-to-end locally and
# per-step from release.yml (the workflow calls the same subcommands with an
# already-built cross-compiled binary, so CI and local never drift).
#
# Subcommands:
#   all     [--version V] [--no-installer]      # current host: build + everything below
#   stage   --bin PATH                          # fill stage/: binary + READMEs (+ icon on win)
#   portable --zip NAME.zip                     # stage/* + default dshl.toml -> NAME.zip
#   nsis    --version V --artifact NAME         # stage/ -> dshl-<NAME>-setup.exe (needs NSIS)
#   deb     --version V --deb-arch amd64|arm64  # stage/dshl -> .deb (needs dpkg-deb)
#   dmg     --version V --artifact NAME         # stage/dshl -> dshl-<NAME>.dmg (needs macOS)
#
# dshl.toml ships ONLY in the portable zip: installers are packaged without a
# config — the launcher auto-generates a commented template when none is found.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

resolve_version() {
  awk '/\[workspace\.package\]/{f=1} f && /^version/{gsub(/"|version| |=/,""); print; exit}' Cargo.toml
}

need_stage() {
  [ -f stage/dshl ] || [ -f stage/dshl.exe ] || { echo "stage/ has no dshl binary; run 'stage' first" >&2; exit 1; }
}

find_makensis() {
  if command -v makensis >/dev/null 2>&1; then
    command -v makensis
  elif [ -f "/c/Program Files (x86)/NSIS/makensis.exe" ]; then
    echo "/c/Program Files (x86)/NSIS/makensis.exe"
  else
    echo "makensis not found (PATH or Program Files)" >&2
    return 1
  fi
}

CMD="${1:-all}"
[ $# -gt 0 ] && shift
case "$CMD" in
  stage)
    BIN=""
    while [ $# -gt 0 ]; do case "$1" in
      --bin) BIN="$2"; shift 2 ;;
      *) echo "unknown flag: $1" >&2; exit 2 ;;
    esac; done
    [ -n "$BIN" ] && [ -f "$BIN" ] || { echo "--bin PATH required (and must exist)" >&2; exit 2; }
    rm -rf stage
    mkdir -p stage
    # Normalise the binary name to dshl(.exe): installers and deb/dmg expect it.
    case "$BIN" in
      *.exe) cp "$BIN" stage/dshl.exe ;;
      *) cp "$BIN" stage/dshl ;;
    esac
    cp README.md README_en.md stage/
    ;;

  portable)
    ZIP=""
    while [ $# -gt 0 ]; do case "$1" in
      --zip) ZIP="$2"; shift 2 ;;
      --version) _pv="$2"; shift 2 ;;
      *) echo "unknown flag: $1" >&2; exit 2 ;;
    esac; done
    # Default name embeds the version (asset-naming convention:
    # dshl-<version>-<os>-<arch>.zip); an explicit --zip still wins.
    if [ -z "$ZIP" ]; then
      case "$(uname -s)" in
        Linux)  _os=linux ;;
        Darwin) _os=macos ;;
        *)      _os=windows ;;
      esac
      ZIP="dshl-${_pv:-0.0.0}-${_os}.zip"
    fi
    need_stage
    tmp="$(mktemp -d)"
    cp stage/* "$tmp/"
    cp dshl.example.toml "$tmp/dshl.toml"
    if PY="$(command -v python3 || command -v python)"; then
      "$PY" - "$tmp" "$ZIP" <<'PY'
import os, sys, zipfile
stage, out = sys.argv[1], sys.argv[2]
with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED) as z:
    for root, _, files in os.walk(stage):
        for f in files:
            full = os.path.join(root, f)
            z.write(full, os.path.relpath(full, stage))
PY
    elif tar -a -cf /dev/null . >/dev/null 2>&1; then
      # bsdtar (Windows 10+ / macOS): -a picks zip from the extension.
      (cd "$tmp" && tar -a -cf "$OLDPWD/$ZIP" .)
    else
      echo "need python or bsdtar to build the portable zip" >&2
      exit 1
    fi
    echo "== wrote $ZIP"
    ;;

  nsis)
    VERSION="" ARTIFACT=""
    while [ $# -gt 0 ]; do case "$1" in
      --version) VERSION="$2"; shift 2 ;;
      --artifact) ARTIFACT="$2"; shift 2 ;;
      *) echo "unknown flag: $1" >&2; exit 2 ;;
    esac; done
    [ -n "$VERSION" ] && [ -n "$ARTIFACT" ] || { echo "--version/--artifact required" >&2; exit 2; }
    need_stage
    exe="$(ls stage/dshl.exe 2>/dev/null || true)"
    [ -n "$exe" ] || { echo "NSIS needs stage/dshl.exe" >&2; exit 1; }
    cp packing/windows/dsh.ico stage/
    # Absolute paths: some makensis builds resolve File/Icon paths against the
    # script's directory instead of the working directory. Under MSYS/git-bash
    # the native exe needs Windows-style paths (cygpath), POSIX elsewhere.
    stage_dir="$root/stage"
    outfile="$root/dshl-${VERSION}-${ARTIFACT}-setup.exe"
    if command -v cygpath >/dev/null 2>&1; then
      stage_dir="$(cygpath -w "$stage_dir")"
      outfile="$(cygpath -w "$outfile")"
    fi
    "$(find_makensis)" -V3 \
      "-DSTAGE_DIR=$stage_dir" \
      "-DPRODUCT_VERSION=$VERSION" \
      "-DOUTFILE=$outfile" \
      packing/windows/dshl.nsi
    echo "== wrote dshl-$ARTIFACT-setup.exe"
    ;;

  deb)
    VERSION="" DEB_ARCH=""
    while [ $# -gt 0 ]; do case "$1" in
      --version) VERSION="$2"; shift 2 ;;
      --deb-arch) DEB_ARCH="$2"; shift 2 ;;
      *) echo "unknown flag: $1" >&2; exit 2 ;;
    esac; done
    [ -n "$VERSION" ] && [ -n "$DEB_ARCH" ] || { echo "--version/--deb-arch required" >&2; exit 2; }
    need_stage
    bash packing/linux/build-deb.sh stage/dshl "$VERSION" "$DEB_ARCH" .
    echo "== wrote .deb ($DEB_ARCH)"
    ;;

  dmg)
    VERSION="" ARTIFACT=""
    while [ $# -gt 0 ]; do case "$1" in
      --version) VERSION="$2"; shift 2 ;;
      --artifact) ARTIFACT="$2"; shift 2 ;;
      *) echo "unknown flag: $1" >&2; exit 2 ;;
    esac; done
    [ -n "$VERSION" ] && [ -n "$ARTIFACT" ] || { echo "--version/--artifact required" >&2; exit 2; }
    need_stage
    bash packing/macos/build-dmg.sh stage/dshl "$VERSION" . "" "dshl-${VERSION}-${ARTIFACT}.dmg"
    ;;

  all)
    NO_INSTALLER=0
    while [ $# -gt 0 ]; do case "$1" in
      --version) VERSION="$2"; shift 2 ;;
      --no-installer) NO_INSTALLER=1; shift ;;
      *) echo "unknown flag: $1" >&2; exit 2 ;;
    esac; done
    [ -n "${VERSION:-}" ] || VERSION="$(resolve_version)"
    case "$(uname -m)" in x86_64) arch=x86_64 ;; aarch64|arm64) arch=aarch64 ;; *) echo "unsupported machine" >&2; exit 1 ;; esac
    echo "== dshl package: version=$VERSION arch=$arch"
    cargo build --release --locked -p dshl
    # NOTE: [ x = GLOB ] is a LITERAL compare in test(1) — must be case/glob.
    case "$(uname -s)" in
      MINGW*|MSYS*|CYGWIN*) ext=.exe ;;
      *) ext= ;;
    esac
    case "$(uname -s)" in
      Linux)  _os=linux ;;
      Darwin) _os=macos ;;
      *)      _os=windows ;;
    esac
    "$0" stage --bin "target/release/dshl$ext"
    "$0" portable --zip "dshl-${VERSION}-${_os}-${arch}.zip"
    if [ "$NO_INSTALLER" -eq 0 ]; then
      case "$(uname -s)" in
        Linux) case "$arch" in x86_64) da=amd64 ;; *) da=arm64 ;; esac; "$0" deb --version "$VERSION" --deb-arch "$da" ;;
        Darwin) "$0" dmg --version "$VERSION" --artifact "$arch" ;;
        MINGW*|MSYS*) "$0" nsis --version "$VERSION" --artifact "windows-$arch" ;;
      esac
    fi
    ;;

  *)
    echo "usage: package.sh {all|stage|portable|nsis|deb|dmg} [options]" >&2
    exit 2
    ;;
esac
