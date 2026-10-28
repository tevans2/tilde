#!/usr/bin/env bash
# Builds an optimized, signed Tilde.app at App/.build/Tilde.app. Used by install.sh and by
# the release workflow.
#
#   scripts/bundle.sh               for this Mac's architecture
#   scripts/bundle.sh --universal   Apple silicon and Intel (needs the x86_64-apple-darwin
#                                   Rust target: rustup target add x86_64-apple-darwin)
#
# Signing is as in signing.sh: the local certificate, TILDE_SIGN_IDENTITY, or TILDE_ADHOC=1.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck source=signing.sh
source "$ROOT/scripts/signing.sh"
BUNDLE_ID="io.github.tevans2.tilde"

UNIVERSAL=false
case "${1:-}" in
  --universal) UNIVERSAL=true ;;
  "") ;;
  *) echo "usage: $0 [--universal]" >&2; exit 1 ;;
esac

if $UNIVERSAL && ! rustup target list --installed | grep -q x86_64-apple-darwin; then
  echo "--universal needs the Intel Rust target: rustup target add x86_64-apple-darwin" >&2
  exit 1
fi

# The release core is universal when the x86_64 target is installed.
"$ROOT/scripts/build-core.sh" release

if $UNIVERSAL; then
  # One build per architecture, joined with lipo (SwiftPM's own multi-arch build needs Xcode).
  BINS=()
  for arch in arm64 x86_64; do
    swift build -c release --package-path "$ROOT/App" --triple "$arch-apple-macosx14.0"
    BINS+=("$(swift build -c release --package-path "$ROOT/App" --triple "$arch-apple-macosx14.0" --show-bin-path)")
  done
  BIN="${BINS[0]}"
  EXE="$ROOT/App/.build/Tilde-universal"
  lipo -create "${BINS[0]}/Tilde" "${BINS[1]}/Tilde" -output "$EXE"
else
  swift build -c release --package-path "$ROOT/App"
  BIN="$(swift build -c release --package-path "$ROOT/App" --show-bin-path)"
  EXE="$BIN/Tilde"
fi

VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT/core/Cargo.toml" | head -1)"
BUILD="$(git -C "$ROOT" rev-list --count HEAD 2>/dev/null || echo 1)"
COMMIT="$(git -C "$ROOT" rev-parse --short HEAD 2>/dev/null || echo unknown)"

APP="$ROOT/App/.build/Tilde.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$EXE" "$APP/Contents/MacOS/Tilde"
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
# The `tilde` command for terminals (Homebrew links it onto the PATH).
mkdir -p "$APP/Contents/Resources/bin"
cp "$ROOT/App/bin/tilde" "$APP/Contents/Resources/bin/tilde"
chmod +x "$APP/Contents/Resources/bin/tilde"

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
  <key>TildeCommit</key><string>$COMMIT</string>
  <key>NSHumanReadableCopyright</key><string>MIT License</string>
  <key>LSMinimumSystemVersion</key><string>14.0</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.productivity</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSPrincipalClass</key><string>NSApplication</string>
  <key>CFBundleURLTypes</key>
  <array>
    <dict>
      <key>CFBundleURLName</key><string>$BUNDLE_ID</string>
      <key>CFBundleURLSchemes</key><array><string>tilde</string></array>
    </dict>
  </array>
  <key>NSDesktopFolderUsageDescription</key><string>Tilde shows and searches the files on your Desktop.</string>
  <key>NSDocumentsFolderUsageDescription</key><string>Tilde shows and searches the files in Documents.</string>
  <key>NSDownloadsFolderUsageDescription</key><string>Tilde shows and searches your downloads, and lists new ones in Activity.</string>
  <key>NSRemovableVolumesUsageDescription</key><string>Tilde shows the files on external drives you open.</string>
  <key>NSNetworkVolumesUsageDescription</key><string>Tilde shows the files on network drives you open.</string>
</dict>
</plist>
PLIST

sign_app "$APP" "$BUNDLE_ID"
echo "Built $APP ($VERSION, build $BUILD, $(lipo -archs "$APP/Contents/MacOS/Tilde"))"
