#!/usr/bin/env bash
set -euo pipefail

source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

if pgrep -f -- "qemu-system-arm.*-name ${MACHINE}([[:space:]]|$)" >/dev/null 2>&1; then
    die "emulator '$MACHINE' is already running"
fi

# Ensure the checkout, QEMU build, firmware and machine exist. The setup wizard
# is intentionally left for the visible emulator below.
PBANKI_SKIP_WIZARD=1 "$ROOT/scripts/emulator/setup.sh"

"$ROOT/scripts/emulator/build-app.sh"

if [ "${PBANKI_EMULATOR_HEADLESS:-0}" != "1" ]; then
    if [ -z "${DISPLAY:-}${WAYLAND_DISPLAY:-}" ]; then
        die "no DISPLAY/WAYLAND_DISPLAY; run with PBANKI_EMULATOR_HEADLESS=1 and use make emulator-run"
    fi
fi

log="${PBANKI_EMULATOR_LOG:-/tmp/pbanki-emulator-dev.log}"
args=(run "$MACHINE" --qmp-socket --serial-socket)
if [ "${PBANKI_EMULATOR_HEADLESS:-0}" = "1" ]; then
    args+=(--headless)
fi

echo "starting emulator window..."
"$EMU_DIR/eink" "${args[@]}" >"$log" 2>&1 &
emu_pid=$!

cleanup() {
    python3 "$EMU_DIR/scripts/eink-qmp.py" --machine "$MACHINE" quit >/dev/null 2>&1 || true
    kill "$emu_pid" 2>/dev/null || true
}
trap cleanup EXIT INT TERM

sleep 2
if ! kill -0 "$emu_pid" 2>/dev/null; then
    tail -30 "$log" >&2 || true
    die "emulator failed to start; see $log"
fi

echo "completing setup if needed..."
python3 "$ROOT/scripts/emulator/complete-setup.py" \
    --emulator-dir "$EMU_DIR" \
    --machine "$MACHINE" \
    --only-if-running \
    --screenshot "$ROOT/target/emulator/setup-complete.png"

echo "ensuring Wi-Fi..."
if ! python3 "$ROOT/scripts/emulator/enable-wifi.py" \
    --emulator-dir "$EMU_DIR" \
    --machine "$MACHINE"; then
    echo "warning: Wi-Fi is not connected; fix it in Settings and run 'make emulator-sync'" >&2
    echo "the emulator is still running; press Ctrl+C to stop"
    wait "$emu_pid"
    exit 1
fi

echo "uploading pbAnki..."
if ! PBANKI_SKIP_BUILD=1 "$ROOT/scripts/emulator/sync-app.sh"; then
    echo "warning: upload failed; the emulator is still running" >&2
    wait "$emu_pid"
    exit 1
fi

echo "launching pbAnki..."
python3 "$ROOT/scripts/emulator/launch-app.py" \
    --emulator-dir "$EMU_DIR" \
    --machine "$MACHINE" \
    --screenshot "$ROOT/target/emulator/pbanki-running.png" || true

echo
echo "pbAnki should be visible in the emulator window. Press Ctrl+C to stop."
wait "$emu_pid"
