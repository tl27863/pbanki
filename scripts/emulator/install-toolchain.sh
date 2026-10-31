#!/usr/bin/env bash
set -euo pipefail

ZIG_VERSION="0.14.1"
ZIG_ROOT="$HOME/.local/opt"
ZIG_DIR="$ZIG_ROOT/zig-x86_64-linux-$ZIG_VERSION"
ZIG_SHA256="24aeeec8af16c381934a6cd7d95c807a8cb2cf7df9fa40d359aa884195c4716c"
ZIG_URL="https://ziglang.org/download/$ZIG_VERSION/zig-x86_64-linux-$ZIG_VERSION.tar.xz"
ZIGBUILD_VERSION="0.24.0"
BIN_DIR="$HOME/.local/bin"

mkdir -p "$ZIG_ROOT" "$BIN_DIR"

tmp=""
cleanup() {
    [ -n "$tmp" ] && rm -rf "$tmp"
}
trap cleanup EXIT

if [ ! -x "$ZIG_DIR/zig" ]; then
    echo "installing zig $ZIG_VERSION to $ZIG_DIR"
    tmp="$(mktemp -d)"
    curl -L --fail --retry 3 -o "$tmp/zig.tar.xz" "$ZIG_URL"
    echo "$ZIG_SHA256  $tmp/zig.tar.xz" | sha256sum -c -
    tar -xf "$tmp/zig.tar.xz" -C "$ZIG_ROOT"
else
    echo "zig $ZIG_VERSION already installed at $ZIG_DIR"
fi

if [ ! -x "$BIN_DIR/cargo-zigbuild" ]; then
    echo "installing cargo-zigbuild $ZIGBUILD_VERSION to $BIN_DIR"
    tmp="$(mktemp -d)"
    url="https://github.com/rust-cross/cargo-zigbuild/releases/download/v$ZIGBUILD_VERSION/cargo-zigbuild-x86_64-unknown-linux-gnu.tar.xz"
    curl -L --fail --retry 3 -o "$tmp/cargo-zigbuild.tar.xz" "$url"
    tar -xf "$tmp/cargo-zigbuild.tar.xz" -C "$tmp"
    cp "$tmp/cargo-zigbuild-x86_64-unknown-linux-gnu/cargo-zigbuild" "$BIN_DIR/"
    chmod +x "$BIN_DIR/cargo-zigbuild"
else
    echo "cargo-zigbuild already installed at $BIN_DIR/cargo-zigbuild"
fi

if [ ! -x "$HOME/.cargo/bin/rustup" ]; then
    echo "installing rustup (1.99.1 + armv7 target)"
    tmp="$(mktemp -d)"
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs -o "$tmp/rustup-init.sh"
    sh "$tmp/rustup-init.sh" -y --no-modify-path --profile minimal \
        --default-toolchain 1.99.1 --target armv7-unknown-linux-gnueabi
else
    "$HOME/.cargo/bin/rustup" toolchain install 1.99.1 --profile minimal
    "$HOME/.cargo/bin/rustup" target add armv7-unknown-linux-gnueabi --toolchain 1.99.1
fi

echo
echo "toolchain ready. build with: make emulator-build"
