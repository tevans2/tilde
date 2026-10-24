#!/usr/bin/env bash
# Builds the Rust core and the app, then runs Tilde (debug).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
"$ROOT/scripts/build-core.sh" debug
swift build --package-path "$ROOT/App"
exec "$ROOT/App/.build/debug/Tilde"
