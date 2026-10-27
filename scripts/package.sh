#!/usr/bin/env bash
# Packages App/.build/Tilde.app (made by bundle.sh) for a release, into dist/:
#
#   Tilde-macOS.zip   what get.sh installs
#   Tilde.dmg         drag to Applications
#   SHA256SUMS.txt
#
# The names don't carry the version, so .../releases/latest/download/<name> always works.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
APP="$ROOT/App/.build/Tilde.app"
DIST="$ROOT/dist"
[[ -d "$APP" ]] || { echo "No $APP: run scripts/bundle.sh first" >&2; exit 1; }

rm -rf "$DIST"
mkdir -p "$DIST"
# ditto keeps the signature and extended attributes intact, unlike zip.
ditto -c -k --keepParent "$APP" "$DIST/Tilde-macOS.zip"

STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT
ditto "$APP" "$STAGE/Tilde.app"
ln -s /Applications "$STAGE/Applications"
hdiutil create -quiet -volname Tilde -srcfolder "$STAGE" -fs HFS+ -format UDZO -ov "$DIST/Tilde.dmg"

(cd "$DIST" && shasum -a 256 Tilde-macOS.zip Tilde.dmg > SHA256SUMS.txt)
ls -lh "$DIST"
