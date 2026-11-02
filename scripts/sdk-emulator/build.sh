#!/usr/bin/env bash
# Builds pbAnki for the $SDK PC emulator: x86-64, linked against the glibc of
# the SDK's libinkview.so, with the matching SDK bindings. Nothing from the
# SDK is needed at link time: inkview-rs loads libinkview.so at runtime.
set -euo pipefail

source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

# Zig and cargo-zigbuild from mise.toml, also when mise is not activated.
if command -v mise >/dev/null 2>&1; then
    mise_paths="$(cd "$ROOT" && mise bin-paths 2>/dev/null | paste -sd: -)"
    [ -z "$mise_paths" ] || export PATH="$mise_paths:$PATH"
fi

for tool in zig cargo-zigbuild; do
    command -v "$tool" >/dev/null 2>&1 || {
        echo "error: $tool not found; run \`mise install\` in $ROOT" >&2
        exit 1
    }
done

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
