#!/usr/bin/env bash
# Build a macOS .app bundle and wrap it in a .dmg.
#
# Usage: build-dmg.sh <binary> <version> <outdir> [config] [outname] [icon_dir]
#   binary    path to the dshl executable
#   version   e.g. 0.1.0
#   outdir    directory to write the .dmg into
#   config    dshl.toml source; when empty the .app ships WITHOUT a config
#             (the launcher auto-generates a default template in the platform
#             config directory). Optional.
#   outname   output .dmg file name (defaults to dshl-<version>.dmg)
#   icon_dir  directory holding dsh.png (1024px, black) — the .icns is built
#             from it with sips + iconutil (defaults to packing/macos, run
#             from repo root)
set -euo pipefail

BIN="$1"
VERSION="$2"
OUTDIR="$3"
CONFIG="${4:-}"
OUTNAME="${5:-dshl-${VERSION}.dmg}"
ICON_DIR="${6:-packing/macos}"

APP="DSHL.app"
CONTENTS="$APP/Contents"

rm -rf "$APP"
mkdir -p "$CONTENTS/MacOS" "$CONTENTS/Resources"

install -m 0755 "$BIN" "$CONTENTS/MacOS/dshl"
if [ -n "$CONFIG" ]; then
  install -m 0644 "$CONFIG" "$CONTENTS/MacOS/dshl.toml"
fi

# Build the .icns from the 1024px source PNG via a standard iconset.
ICONSET="$APP/dshl.iconset"
mkdir -p "$ICONSET"
for s in 16 32 128 256 512; do
  sips -z "$s" "$s" "$ICON_DIR/dsh.png" --out "$ICONSET/icon_${s}x${s}.png" >/dev/null
  sips -z "$((s * 2))" "$((s * 2))" "$ICON_DIR/dsh.png" --out "$ICONSET/icon_${s}x${s}@2x.png" >/dev/null
done
iconutil -c icns "$ICONSET" -o "$CONTENTS/Resources/dshl.icns"
rm -rf "$ICONSET"

cat > "$CONTENTS/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key>
  <string>DSHL</string>
  <key>CFBundleDisplayName</key>
  <string>DSHL</string>
  <key>CFBundleIdentifier</key>
  <string>com.dshl.launcher</string>
  <key>CFBundleVersion</key>
  <string>${VERSION}</string>
  <key>CFBundleShortVersionString</key>
  <string>${VERSION}</string>
  <key>CFBundleExecutable</key>
  <string>dshl</string>
  <key>CFBundleIconFile</key>
  <string>dshl</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>LSMinimumSystemVersion</key>
  <string>11.0</string>
  <key>NSHighResolutionCapable</key>
  <true/>
</dict>
</plist>
EOF

mkdir -p "$OUTDIR"
DMG="$OUTDIR/$OUTNAME"
rm -f "$DMG"
hdiutil create -volname "DSHL" -srcfolder "$APP" -ov -format UDZO "$DMG" >/dev/null
rm -rf "$APP"
echo "$DMG"
