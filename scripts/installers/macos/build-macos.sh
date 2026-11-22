#!/usr/bin/env bash
# Usage: build-macos.sh <minima-binary> <version> <arch> <dist-dir>
# Builds a graphical .pkg installer and a drag-to-Applications .dmg. Runs on macOS only.
#
# Minima is a terminal program, so Minima.app is a tiny launcher that opens
# Terminal.app running `minima`. The .pkg also puts `minima` on the PATH.
# Optional signing:  MACOS_SIGN_APP="Developer ID Application: ..."  MACOS_SIGN_PKG="Developer ID Installer: ..."
set -euo pipefail
BIN="$1"; VER="$2"; ARCH="$3"; DIST="$4"
[ "$(uname -s)" = Darwin ] || { echo "macOS installers can only be built on macOS" >&2; exit 1; }

WORK="$(mktemp -d)"; trap 'rm -rf "$WORK"' EXIT
APP="$WORK/Minima.app"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/Resources/minima"
cp "$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)/assets/minima.icns" "$APP/Contents/Resources/Minima.icns"
chmod 755 "$APP/Contents/Resources/minima"

cat > "$APP/Contents/MacOS/Minima" <<'EOF'
#!/bin/bash
# Open Terminal.app running the bundled browser.
BIN="$(cd "$(dirname "$0")/../Resources" && pwd)/minima"
osascript <<OSA
tell application "Terminal"
  activate
  do script quoted form of "$BIN"
end tell
OSA
EOF
chmod 755 "$APP/Contents/MacOS/Minima"

cat > "$APP/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleName</key><string>Minima</string>
  <key>CFBundleDisplayName</key><string>Minima</string>
  <key>CFBundleIdentifier</key><string>app.minima.browser</string>
  <key>CFBundleVersion</key><string>$VER</string>
  <key>CFBundleShortVersionString</key><string>$VER</string>
  <key>CFBundleExecutable</key><string>Minima</string>
  <key>CFBundleIconFile</key><string>Minima</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>LSMinimumSystemVersion</key><string>10.13</string>
  <key>NSHighResolutionCapable</key><true/>
</dict></plist>
EOF

if [ -n "${MACOS_SIGN_APP:-}" ]; then
  codesign --force --options runtime --sign "$MACOS_SIGN_APP" "$APP/Contents/Resources/minima"
  codesign --force --options runtime --sign "$MACOS_SIGN_APP" "$APP"
else
  # ad-hoc signature: required for arm64 binaries to run at all
  codesign --force --sign - "$APP/Contents/Resources/minima" 2>/dev/null || true
  codesign --force --sign - "$APP" 2>/dev/null || true
fi

# ---- .pkg: /Applications/Minima.app + /usr/local/bin/minima -------------
ROOTFS="$WORK/pkgroot"
mkdir -p "$ROOTFS/Applications" "$ROOTFS/usr/local/bin"
cp -R "$APP" "$ROOTFS/Applications/"
cp "$BIN" "$ROOTFS/usr/local/bin/minima"; chmod 755 "$ROOTFS/usr/local/bin/minima"
COMPONENT="$WORK/minima-component.pkg"
pkgbuild --root "$ROOTFS" --identifier app.minima.browser --version "$VER" --install-location / "$COMPONENT"
PKG="$DIST/Minima-$VER-macos-$ARCH.pkg"
if [ -n "${MACOS_SIGN_PKG:-}" ]; then
  productbuild --package "$COMPONENT" --sign "$MACOS_SIGN_PKG" "$PKG"
else
  productbuild --package "$COMPONENT" "$PKG"
fi
echo "  wrote $PKG"

# ---- .dmg: drag Minima.app to Applications -----------------------------
STAGE="$WORK/dmg"; mkdir -p "$STAGE"
cp -R "$APP" "$STAGE/"
ln -s /Applications "$STAGE/Applications"
DMG="$DIST/Minima-$VER-macos-$ARCH.dmg"
rm -f "$DMG"
hdiutil create -volname "Minima $VER" -srcfolder "$STAGE" -ov -format UDZO "$DMG" >/dev/null
echo "  wrote $DMG"
