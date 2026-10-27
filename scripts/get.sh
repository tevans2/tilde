#!/usr/bin/env bash
# Installs or updates Tilde from the latest GitHub release, without building anything:
#
#   curl -fsSL https://raw.githubusercontent.com/tevans2/tilde/main/scripts/get.sh | bash
#
# Installs into ~/Applications, or wherever Tilde already is (/Applications included);
# pass a folder to choose (… | bash -s -- /Applications). Running it again updates.
#
# Downloads made this way aren't quarantined, so macOS doesn't stop the first launch the
# way it does for an unnotarized app downloaded in a browser. Every release is signed
# with the same certificate, so permissions (Full Disk Access…) carry over to updates.
set -euo pipefail

REPO="tevans2/tilde"
BUNDLE_ID="io.github.tevans2.tilde"
URL="${TILDE_RELEASE_URL:-https://github.com/$REPO/releases/latest/download}"

DEST="${1:-}"
if [[ -z "$DEST" ]]; then
  DEST="$HOME/Applications"
  for app in "$HOME/Applications/Tilde.app" "/Applications/Tilde.app"; do
    if [[ "$(defaults read "$app/Contents/Info" CFBundleIdentifier 2>/dev/null)" == "$BUNDLE_ID" ]]; then
      DEST="$(dirname "$app")"
      break
    fi
  done
fi

if [[ "$(sw_vers -productVersion | cut -d. -f1)" -lt 14 ]]; then
  echo "Tilde needs macOS 14 (Sonoma) or later." >&2
  exit 1
fi

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
echo "Downloading Tilde…"
curl -fsSL "$URL/Tilde-macOS.zip" -o "$WORK/Tilde-macOS.zip"
curl -fsSL "$URL/SHA256SUMS.txt" -o "$WORK/SHA256SUMS.txt"
(cd "$WORK" && grep ' Tilde-macOS.zip$' SHA256SUMS.txt | shasum -a 256 -c --quiet -) || {
  echo "The download doesn't match its checksum; nothing was installed." >&2
  exit 1
}
ditto -x -k "$WORK/Tilde-macOS.zip" "$WORK"
VERSION="$(defaults read "$WORK/Tilde.app/Contents/Info" CFBundleShortVersionString)"

RUNNING=false
if pgrep -f "$DEST/Tilde.app/Contents/MacOS/Tilde" >/dev/null; then
  RUNNING=true
  osascript -e "tell application id \"$BUNDLE_ID\" to quit" >/dev/null 2>&1 || true
  sleep 1
fi
mkdir -p "$DEST"
rm -rf "${DEST:?}/Tilde.app"
ditto "$WORK/Tilde.app" "$DEST/Tilde.app"
touch "$DEST/Tilde.app"

echo "Installed Tilde $VERSION in $DEST."
if $RUNNING; then
  open "$DEST/Tilde.app"
else
  echo "Open it with: open \"$DEST/Tilde.app\" (or from Spotlight or Raycast)"
fi
