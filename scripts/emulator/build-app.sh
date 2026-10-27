#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
TARGET="armv7-unknown-linux-gnueabi"
ZIG_VERSION="0.14.1"
ZIG="${CARGO_ZIGBUILD_ZIG_PATH:-$HOME/.local/opt/zig-x86_64-linux-$ZIG_VERSION/zig}"

export PATH="$HOME/.cargo/bin:$HOME/.local/bin:$PATH"
export CARGO_ZIGBUILD_ZIG_PATH="$ZIG"

command -v cargo >/dev/null 2>&1 || {
    echo "error: cargo not found; run scripts/emulator/install-toolchain.sh" >&2
    exit 1
}
command -v cargo-zigbuild >/dev/null 2>&1 || {
    echo "error: cargo-zigbuild not found; run scripts/emulator/install-toolchain.sh" >&2
    exit 1
}
[ -x "$ZIG" ] || {
    echo "error: zig not found at $ZIG; run scripts/emulator/install-toolchain.sh" >&2
    exit 1
}

mkdir -p "$ROOT/target/emulator"
cd "$ROOT"

cargo zigbuild \
    --target "$TARGET.2.23" \
    --profile dev \
    -p pbanki \
    --features=sdk-6-8

cp "$ROOT/target/$TARGET/debug/pbanki" "$ROOT/target/emulator/pbanki.app"
if command -v llvm-strip >/dev/null 2>&1; then
    llvm-strip --strip-unneeded "$ROOT/target/emulator/pbanki.app"
fi

ls -lh "$ROOT/target/emulator/pbanki.app"
