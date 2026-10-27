#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
EMU_DIR="${EINK_EMULATOR_DIR:-$ROOT/../eink-emulator}"
MACHINE="${PBANKI_EMULATOR_MACHINE:-pb700}"

QMP_SOCK="$EMU_DIR/machines/$MACHINE/$MACHINE.qmp.sock"
SERIAL_SOCK="$EMU_DIR/machines/$MACHINE/$MACHINE.serial.sock"
SERIAL_LOG="$EMU_DIR/machines/$MACHINE/$MACHINE.serial.log"
APP_ARTIFACT="$ROOT/target/emulator/pbanki.app"

EMULATOR_REPO="https://github.com/katadelos/eink-emulator.git"
EMULATOR_MODEL="pocketbook-700"
FIRMWARE_URL="https://download.pocketbook-int.com/fw/Jezetek/700/user/20260223/sw_20260223_U700_6.10.3209_user.zip"
FIRMWARE_ZIP="$EMU_DIR/downloads/sw_20260223_U700_6.10.3209_user.zip"

die() {
    echo "error: $*" >&2
    exit 1
}

require() {
    command -v "$1" >/dev/null 2>&1 || die "missing '$1' (see README emulator prerequisites)"
}
