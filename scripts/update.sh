#!/usr/bin/env bash
# Rebuilds the installed Tilde.app in place, keeping its permissions (Full Disk Access,
# Accessibility, folder access): the new build is signed with the same local certificate,
# so macOS sees the same app. If Tilde was running, it's reopened afterwards.
#
#   scripts/update.sh                       the copy in ~/Applications or /Applications
#   scripts/update.sh /path/to/Tilde.app    another copy
#
# The first update of a copy that was signed ad hoc (installed before the local
# certificate existed) needs the permissions granting once more; after that they stay.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BUNDLE_ID="io.github.tevans2.tilde"
# shellcheck source=signing.sh
source "$ROOT/scripts/signing.sh"

INSTALLED=""
for app in ${1:+"$1"} "$HOME/Applications/Tilde.app" "/Applications/Tilde.app"; do
  if [[ -d "$app" ]] && [[ "$(defaults read "$app/Contents/Info" CFBundleIdentifier 2>/dev/null)" == "$BUNDLE_ID" ]]; then
    INSTALLED="$app"
    break
  fi
done
if [[ -z "$INSTALLED" ]]; then
  echo "Tilde isn't installed in ~/Applications or /Applications. Install it with scripts/install.sh." >&2
  exit 1
fi

KEEPS=true
signed_locally "$INSTALLED" || KEEPS=false
RUNNING=false
pgrep -f "$INSTALLED/Contents/MacOS/Tilde" >/dev/null && RUNNING=true

"$ROOT/scripts/install.sh" "$(dirname "$INSTALLED")"

if $RUNNING; then
  open "$INSTALLED"
fi
if $KEEPS; then
  echo "Updated $INSTALLED; its permissions carry over."
else
  echo "Updated $INSTALLED. It was signed ad hoc before, so grant Full Disk Access (and"
  echo "Accessibility, for file dialogs) once more; later updates keep them."
fi
