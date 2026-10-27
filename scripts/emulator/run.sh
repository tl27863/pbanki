#!/usr/bin/env bash
set -euo pipefail

source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

[ -d "$EMU_DIR/machines/$MACHINE" ] || {
    echo "error: emulator machine '$MACHINE' not found" >&2
    echo "run: make emulator-setup" >&2
    exit 1
}

args=(run "$MACHINE" --qmp-socket --serial-socket)
if [ "${PBANKI_EMULATOR_HEADLESS:-0}" = "1" ]; then
    args+=(--headless)
fi

exec "$EMU_DIR/eink" "${args[@]}"
