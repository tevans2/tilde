#!/usr/bin/env bash
# Builds the Rust core and generates its Swift bindings for the App package.
#
#   scripts/build-core.sh            debug build, host architecture
#   scripts/build-core.sh release    release build (universal if the x86_64 target is installed)
#
# Outputs (gitignored) under App/Vendor/:
#   lib/libtilde_ffi.a               static library linked by the TildeFFI target
#   include/                         C header + module map (TildeFFIC target)
#   swift/tilde_ffi.swift            generated Swift bindings (TildeFFI target)
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PROFILE="${1:-debug}"
VENDOR="$ROOT/App/Vendor"
cd "$ROOT/core"

case "$PROFILE" in
  debug) CARGO_FLAGS=() ;;
  release) CARGO_FLAGS=(--release) ;;
  *) echo "usage: $0 [debug|release]" >&2; exit 1 ;;
esac

HOST_TARGET="$(rustc -vV | sed -n 's/^host: //p')"
TARGETS=("$HOST_TARGET")
if [[ "$PROFILE" == release ]] && rustup target list --installed | grep -q x86_64-apple-darwin \
   && [[ "$HOST_TARGET" != x86_64-apple-darwin ]]; then
  TARGETS+=(x86_64-apple-darwin)
fi

export MACOSX_DEPLOYMENT_TARGET=14.0
for t in "${TARGETS[@]}"; do
  cargo build -p tilde-ffi --target "$t" ${CARGO_FLAGS[@]+"${CARGO_FLAGS[@]}"}
done

mkdir -p "$VENDOR/lib" "$VENDOR/include" "$VENDOR/swift"
LIBS=()
for t in "${TARGETS[@]}"; do LIBS+=("target/$t/$PROFILE/libtilde_ffi.a"); done
if (( ${#LIBS[@]} > 1 )); then
  lipo -create "${LIBS[@]}" -output "$VENDOR/lib/libtilde_ffi.a"
else
  cp "${LIBS[0]}" "$VENDOR/lib/libtilde_ffi.a"
fi

# Bindings are generated from the dylib's metadata. A separate target dir keeps the
# bindgen feature from rebuilding the libraries above.
GEN="$(mktemp -d)"
trap 'rm -rf "$GEN"' EXIT
CARGO_TARGET_DIR="$ROOT/core/target/bindgen" cargo run -q -p tilde-ffi --features bindgen --bin uniffi-bindgen -- \
  generate --library "target/$HOST_TARGET/$PROFILE/libtilde_ffi.dylib" --language swift --out-dir "$GEN"

cp "$GEN/tilde_ffi.swift" "$VENDOR/swift/"
cp "$GEN/tilde_ffiFFI.h" "$VENDOR/include/"
cp "$GEN/tilde_ffiFFI.modulemap" "$VENDOR/include/module.modulemap"
echo "core: built ${TARGETS[*]} ($PROFILE) → App/Vendor"
