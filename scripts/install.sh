#!/usr/bin/env bash
# Builds an optimized Tilde.app and installs it.
#
#   scripts/install.sh                 install into ~/Applications
#   scripts/install.sh /Applications   or somewhere else
#
# Signs with a local certificate made on first use (see scripts/signing.sh), so
# permissions such as Full Disk Access carry over to later installs and updates.
# scripts/update.sh rebuilds an installed copy in place.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck source=signing.sh
source "$ROOT/scripts/signing.sh"
DEST="${1:-$HOME/Applications}"
BUNDLE_ID="io.github.tevans2.tilde"

"$ROOT/scripts/build-core.sh" release
swift build -c release --package-path "$ROOT/App"
BIN="$(swift build -c release --package-path "$ROOT/App" --show-bin-path)"

VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT/core/Cargo.toml" | head -1)"
BUILD="$(git -C "$ROOT" rev-list --count HEAD 2>/dev/null || echo 1)"

APP="$ROOT/App/.build/Tilde.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN/Tilde" "$APP/Contents/MacOS/Tilde"
# SwiftPM resource bundles (SwiftTerm's) go in Resources, where they're looked up.
for bundle in "$BIN"/*.bundle; do
  [[ -e "$bundle" ]] && cp -R "$bundle" "$APP/Contents/Resources/"
done

# Icon: Finder and Launchpad show Tilde.icns (the dark variant; macOS 15 icons can't
# follow the appearance), and the running app swaps its Dock icon between the PNGs.
ICONSET="$ROOT/App/.build/Tilde.iconset"
rm -rf "$ICONSET"
mkdir -p "$ICONSET"
for s in 16 32 128 256 512; do
  sips -z "$s" "$s" "$ROOT/App/Icon/Tilde-dark.png" --out "$ICONSET/icon_${s}x${s}.png" >/dev/null
  sips -z "$((s * 2))" "$((s * 2))" "$ROOT/App/Icon/Tilde-dark.png" --out "$ICONSET/icon_${s}x${s}@2x.png" >/dev/null
done
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/Tilde.icns"
cp "$ROOT/App/Icon/Tilde-light.png" "$ROOT/App/Icon/Tilde-dark.png" "$APP/Contents/Resources/"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Tilde</string>
  <key>CFBundleDisplayName</key><string>Tilde</string>
  <key>CFBundleIdentifier</key><string>$BUNDLE_ID</string>
  <key>CFBundleExecutable</key><string>Tilde</string>
  <key>CFBundleIconFile</key><string>Tilde</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleVersion</key><string>$BUILD</string>
  <key>LSMinimumSystemVersion</key><string>14.0</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.productivity</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSPrincipalClass</key><string>NSApplication</string>
  <key>NSDesktopFolderUsageDescription</key><string>Tilde shows and searches the files on your Desktop.</string>
  <key>NSDocumentsFolderUsageDescription</key><string>Tilde shows and searches the files in Documents.</string>
  <key>NSDownloadsFolderUsageDescription</key><string>Tilde shows and searches your downloads, and lists new ones in Activity.</string>
  <key>NSRemovableVolumesUsageDescription</key><string>Tilde shows the files on external drives you open.</string>
  <key>NSNetworkVolumesUsageDescription</key><string>Tilde shows the files on network drives you open.</string>
</dict>
</plist>
PLIST

sign_app "$APP" "$BUNDLE_ID"

# Replace the installed copy, quitting it first if it's running.
if pgrep -f "$DEST/Tilde.app/Contents/MacOS/Tilde" >/dev/null; then
  osascript -e "tell application id \"$BUNDLE_ID\" to quit" >/dev/null 2>&1 || true
  sleep 1
fi
mkdir -p "$DEST"
rm -rf "${DEST:?}/Tilde.app"
cp -R "$APP" "$DEST/"
# Finder caches icons; touching the bundle makes it look again.
touch "$DEST/Tilde.app"
echo "Installed $DEST/Tilde.app ($VERSION, build $BUILD)"
echo "Open it with: open \"$DEST/Tilde.app\""
