#!/usr/bin/env bash
# Builds pbAnki for the $SDK PC emulator: x86-64, linked against the glibc of
# the SDK's libinkview.so, with the matching SDK bindings. Nothing from the
# SDK is needed at link time: inkview-rs loads libinkview.so at runtime.
set -euo pipefail

source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

ZIG_VERSION="0.14.1"
ZIG_DIR="$HOME/.local/opt/zig-x86_64-linux-$ZIG_VERSION"
export PATH="$HOME/.cargo/bin:$HOME/.local/bin:$ZIG_DIR:$PATH"

command -v cargo-zigbuild >/dev/null 2>&1 || {
    echo "error: cargo-zigbuild not found; cargo install cargo-zigbuild --locked" >&2
    exit 1
}
command -v zig >/dev/null 2>&1 || {
    echo "error: zig not found; install Zig $ZIG_VERSION (e.g. to $ZIG_DIR)" >&2
    exit 1
}

cd "$ROOT"
cargo zigbuild \
    --target "$RUST_TARGET.$GLIBC" \
    --profile "${PROFILE_CARGO:-dev}" \
    -p pbanki \
    --features="$SDK_FEATURE${FEATURES:+,$FEATURES}"

out="debug"
[ "${PROFILE_CARGO:-dev}" = "dev" ] || out="${PROFILE_CARGO}"
mkdir -p "$EMU_DIR"
cp "$ROOT/target/$RUST_TARGET/$out/pbanki" "$APP_BIN"
ls -lh "$APP_BIN"
