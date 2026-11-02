#!/usr/bin/env bash
# Downloads the SDK archive for $SDK (6.10 or 6.11), verifies it against
# SHA256SUMS, and extracts only the PC emulator runtime: the x86-64
# libinkview.so and its sibling libraries, and the hwconfig profiles.
# Also fetches the emulator's system/ data tree from the SDK repo.
set -euo pipefail

source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

command -v 7z >/dev/null || { echo "error: 7z not found (install p7zip / 7zip)" >&2; exit 1; }
command -v curl >/dev/null || { echo "error: curl not found" >&2; exit 1; }
command -v git >/dev/null || { echo "error: git not found" >&2; exit 1; }

expected=$(awk -v f="$SDK_ARCHIVE" '$2 == f { print $1 }' "$HERE/SHA256SUMS")
[ -n "$expected" ] || { echo "error: no hash for $SDK_ARCHIVE in SHA256SUMS" >&2; exit 1; }

archive="$SDK_DL/$SDK_ARCHIVE"
mkdir -p "$SDK_DL"
if [ ! -f "$archive" ]; then
    echo "downloading $SDK_ARCHIVE (several hundred MiB)"
    curl -fL --retry 3 -o "$archive.part" "$SDK_RELEASES/$SDK_TAG/$SDK_ARCHIVE"
    mv "$archive.part" "$archive"
fi

echo "verifying $SDK_ARCHIVE"
echo "$expected  $archive" | sha256sum -c --quiet - || {
    echo "error: $archive does not match the pinned hash; delete it to download again" >&2
    exit 1
}

if [ ! -f "$SDK_RUNTIME/.complete" ]; then
    echo "extracting the PC runtime"
    rm -rf "$SDK_RUNTIME" "$SDK_DIR/extract"
    mkdir -p "$SDK_DIR/extract"
    7z x -y -bd -bso0 "$archive" -o"$SDK_DIR/extract" \
        "$SDK_PREFIX/local/lib" "$SDK_PREFIX/local/share/config"
    mkdir -p "$SDK_RUNTIME"
    mv "$SDK_DIR/extract/$SDK_PREFIX/local/lib" "$SDK_RUNTIME/lib"
    mv "$SDK_DIR/extract/$SDK_PREFIX/local/share/config" "$SDK_RUNTIME/config"
    rm -rf "$SDK_DIR/extract"
    file "$SDK_RUNTIME/lib/libinkview.so" | grep -q 'x86-64' || {
        echo "error: $SDK_RUNTIME/lib/libinkview.so is not an x86-64 library" >&2
        exit 1
    }
    touch "$SDK_RUNTIME/.complete"
fi

if [ ! -d "$SDK_REPO_DIR/system" ]; then
    echo "fetching the emulator system/ tree ($SDK_REPO @ $SDK_REPO_COMMIT)"
    rm -rf "$SDK_REPO_DIR"
    git clone --quiet --filter=blob:none --no-checkout "$SDK_REPO" "$SDK_REPO_DIR"
    git -C "$SDK_REPO_DIR" sparse-checkout set --no-cone /system
    git -C "$SDK_REPO_DIR" checkout --quiet "$SDK_REPO_COMMIT"
fi

echo "SDK $SDK runtime ready in $SDK_RUNTIME"
