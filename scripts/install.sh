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
DEST="${1:-$HOME/Applications}"
BUNDLE_ID="io.github.tevans2.tilde"

"$ROOT/scripts/bundle.sh"
APP="$ROOT/App/.build/Tilde.app"
VERSION="$(defaults read "$APP/Contents/Info" CFBundleShortVersionString)"
BUILD="$(defaults read "$APP/Contents/Info" CFBundleVersion)"

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
