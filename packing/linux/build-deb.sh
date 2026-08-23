#!/usr/bin/env bash
# Build a Debian package for dshl.
#
# Usage: build-deb.sh <binary> <version> <arch> <outdir> [config] [icon_dir]
#   binary    path to the dshl executable
#   version   e.g. 0.1.0
#   arch      Debian architecture (amd64 / arm64)
#   outdir    directory to write the .deb into
#   config    dshl.toml source; when empty the package ships WITHOUT a config
#             (the launcher auto-generates a default template in the platform
#             config directory). Optional.
#   icon_dir  directory holding dsh.png / dsh-white.png / dsh-512.png /
#             dsh-white-512.png (defaults to packing/linux, run from repo root)
set -euo pipefail

BIN="$1"
VERSION="$2"
ARCH="$3"
OUTDIR="$4"
CONFIG="${5:-}"
ICON_DIR="${6:-packing/linux}"

NAME="dshl"
PKG="${NAME}_${VERSION}_${ARCH}"
ROOT="${PKG}"

rm -rf "$ROOT"
mkdir -p "$ROOT/DEBIAN" \
         "$ROOT/usr/bin" \
         "$ROOT/usr/share/applications" \
         "$ROOT/usr/share/doc/$NAME" \
         "$ROOT/usr/share/icons/hicolor/256x256/apps" \
         "$ROOT/usr/share/icons/hicolor/512x512/apps"

install -m 0755 "$BIN" "$ROOT/usr/bin/dshl"
if [ -n "$CONFIG" ]; then
  mkdir -p "$ROOT/etc/dshl"
  install -m 0644 "$CONFIG" "$ROOT/etc/dshl/dshl.toml"
fi

# App icons: black (default) + white (night/dark docks), 256px and 512px.
install -m 0644 "$ICON_DIR/dsh.png"         "$ROOT/usr/share/icons/hicolor/256x256/apps/dshl.png"
install -m 0644 "$ICON_DIR/dsh-white.png"   "$ROOT/usr/share/icons/hicolor/256x256/apps/dshl-white.png"
install -m 0644 "$ICON_DIR/dsh-512.png"     "$ROOT/usr/share/icons/hicolor/512x512/apps/dshl.png"
install -m 0644 "$ICON_DIR/dsh-white-512.png" "$ROOT/usr/share/icons/hicolor/512x512/apps/dshl-white.png"

cat > "$ROOT/usr/share/applications/dshl.desktop" <<'EOF'
[Desktop Entry]
Name=DSHL
Comment=DeepSeek Harness Launcher
Exec=/usr/bin/dshl
Icon=dshl
Type=Application
Categories=Utility;
Terminal=false
EOF

cat > "$ROOT/usr/share/doc/$NAME/copyright" <<'EOF'
DSHL — DeepSeek Harness web launcher
Licensed under the MIT License.
EOF

# NOTE: unquoted <<EOF so ${VERSION} expands; backticks in the description
# are escaped (\`) so they are NOT executed as command substitution.
cat > "$ROOT/DEBIAN/control" <<EOF
Package: dshl
Version: ${VERSION}
Section: utils
Priority: optional
Architecture: ${ARCH}
Maintainer: dshl maintainers <dshl@users.noreply.github.com>
Description: DeepSeek Harness web launcher
 dshl is a webui.me wrapper that checks the runtime, installs dsh when needed,
 boots \`dsh web\`, and routes the browser to it.
EOF

mkdir -p "$OUTDIR"
dpkg-deb --build --root-owner-group "$ROOT" "$OUTDIR/${PKG}.deb"
rm -rf "$ROOT"
echo "$OUTDIR/${PKG}.deb"
