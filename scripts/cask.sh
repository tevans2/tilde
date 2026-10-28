#!/usr/bin/env bash
# Prints the Homebrew cask for a release, for the tap (github.com/tevans2/homebrew-tilde):
#
#   scripts/cask.sh 0.2.0 <sha256 of Tilde-macOS.zip> > Casks/tilde.rb
#
# The release workflow runs it after publishing, so `brew upgrade` sees each release.
set -euo pipefail

if [[ $# -ne 2 ]]; then
  echo "usage: $0 <version> <sha256>" >&2
  exit 1
fi
VERSION="$1"
SHA256="$2"

cat <<EOF
cask "tilde" do
  version "$VERSION"
  sha256 "$SHA256"

  url "https://github.com/tevans2/tilde/releases/download/v#{version}/Tilde-macOS.zip"
  name "Tilde"
  desc "Keyboard-first file navigator"
  homepage "https://github.com/tevans2/tilde"

  livecheck do
    url :url
    strategy :github_latest
  end

  depends_on macos: :sonoma

  app "Tilde.app"
  binary "#{appdir}/Tilde.app/Contents/Resources/bin/tilde"

  # Tilde isn't notarized, so a quarantined copy wouldn't open the first time.
  # Every release is signed with the same certificate (see scripts/signing.sh).
  postflight_steps do
    run "/usr/bin/xattr",
        args:           ["-dr", "com.apple.quarantine", "{{appdir}}/Tilde.app"],
        writable_paths: ["{{appdir}}/Tilde.app"]
  end

  uninstall quit: "io.github.tevans2.tilde"

  # The config file (~/.config/tilde) is yours and stays.
  zap trash: [
    "~/Library/Application Support/Tilde",
    "~/Library/Caches/Tilde",
    "~/Library/Preferences/io.github.tevans2.tilde.plist",
    "~/Library/Saved Application State/io.github.tevans2.tilde.savedState",
  ]
end
EOF
