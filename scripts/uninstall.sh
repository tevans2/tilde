#!/usr/bin/env bash
# Removes an installed Tilde.app (from ~/Applications, /Applications or the Trash) and what macOS
# keeps about it: privacy permissions, preferences and the Launch Services entry.
#
#   scripts/uninstall.sh           keep Tilde's data (the dev build uses it too)
#   scripts/uninstall.sh --purge   also delete the index, undo history and sort orders
#
# The config file (~/.config/tilde/config.toml) is never deleted.
set -euo pipefail

BUNDLE_ID="io.github.tevans2.tilde"
PURGE=false
case "${1:-}" in
  "") ;;
  --purge) PURGE=true ;;
  *) echo "usage: $0 [--purge]" >&2; exit 1 ;;
esac

# Quit it if it's running from an installed copy.
if pgrep -f "Tilde.app/Contents/MacOS/Tilde" | grep -qv "\.build"; then
  osascript -e "tell application id \"$BUNDLE_ID\" to quit" >/dev/null 2>&1 || true
  sleep 1
  pkill -f "(Applications|Trash)/Tilde.app/Contents/MacOS/Tilde" 2>/dev/null || true
fi

# Permissions granted to the app (Full Disk Access, Desktop, Documents, Downloads). Reset
# while the app is still there: tccutil looks the bundle up.
tccutil reset All "$BUNDLE_ID" >/dev/null 2>&1 && echo "Reset its privacy permissions" || true

LSREGISTER=/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister
removed=false
# A copy in the Trash still launches from Spotlight and Raycast, so it goes too.
for app in "$HOME/Applications/Tilde.app" "/Applications/Tilde.app" \
  "$HOME/.Trash/Tilde.app" "$HOME/.Trash/Applications/Tilde.app"; do
  # Only a Tilde we installed, never another app with the same name.
  if [[ -d "$app" ]] && [[ "$(defaults read "$app/Contents/Info" CFBundleIdentifier 2>/dev/null)" == "$BUNDLE_ID" ]]; then
    "$LSREGISTER" -u "$app" 2>/dev/null || true
    rm -rf "$app"
    echo "Removed $app"
    removed=true
  fi
done
$removed || echo "No installed Tilde.app found"

# Its preferences (the welcome window's "seen" flag, window positions).
defaults delete "$BUNDLE_ID" >/dev/null 2>&1 && echo "Removed its preferences" || true
rm -rf "$HOME/Library/Saved Application State/$BUNDLE_ID.savedState"

if $PURGE; then
  rm -rf "$HOME/Library/Caches/Tilde" "$HOME/Library/Application Support/Tilde"
  echo "Deleted the index, undo history and sort orders"
else
  echo "Kept ~/Library/Caches/Tilde and ~/Library/Application Support/Tilde (--purge deletes them)"
fi
echo "Kept your config (~/.config/tilde)"
