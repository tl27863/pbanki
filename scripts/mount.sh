#!/usr/bin/env bash
# Mounts the PocketBook's applications folder over CIFS (USB networking).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MOUNT_POINT="$ROOT/target/armv7-unknown-linux-gnueabi/debug/applications"

mkdir -p "$MOUNT_POINT"
sudo mount -t cifs -o username=root,vers=1.0 -v //169.254.0.1/ext1/applications "$MOUNT_POINT"
