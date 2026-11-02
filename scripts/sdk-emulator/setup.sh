#!/usr/bin/env bash
# One-time setup for the $SDK PC emulator: SDK runtime, container image, and
# a first pbAnki build.
set -euo pipefail

source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

command -v docker >/dev/null || { echo "error: docker not found" >&2; exit 1; }

"$HERE/fetch.sh"

echo "building container image $IMAGE"
docker build -q -t "$IMAGE" \
    --build-arg uid="$(id -u)" --build-arg gid="$(id -g)" \
    -f "$HERE/Dockerfile.$SDK" "$HERE"

"$HERE/build.sh"

echo "SDK $SDK emulator ready: make sdk-emulator SDK=$SDK"
