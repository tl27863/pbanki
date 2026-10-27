#!/usr/bin/env bash
set -euo pipefail

source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

MACHINE="${PBANKI_EMULATOR_MACHINE:-pb700}"
PORT="${PBANKI_EMULATOR_HTTP_PORT:-8000}"

if [ "${PBANKI_SKIP_BUILD:-0}" != "1" ]; then
    "$ROOT/scripts/emulator/build-app.sh"
fi
[ -f "$APP_ARTIFACT" ] || die "missing $APP_ARTIFACT"

serial() {
    python3 "$ROOT/scripts/emulator/serial.py" \
        --emulator-dir "$EMU_DIR" --machine "$MACHINE" "$@"
}

serial 'echo ok' >/dev/null \
    || die "emulator serial console is not available; start it with 'make emulator-run'"

if [ ! -S "$QMP_SOCK" ]; then
    die "QMP socket not found; start the emulator with 'make emulator-run'"
fi

echo "serving $APP_ARTIFACT on port $PORT"
python3 -m http.server "$PORT" --bind 0.0.0.0 --directory "$(dirname "$APP_ARTIFACT")" \
    >/tmp/pbanki-emulator-http.log 2>&1 &
server_pid=$!
trap 'kill "$server_pid" 2>/dev/null || true' EXIT
sleep 1

iface=""
for candidate in eth0 wlan0; do
    if serial "ifconfig $candidate" 2>/dev/null | grep -q 'inet addr:'; then
        iface="$candidate"
        break
    fi
done

if [ -z "$iface" ]; then
    die "Wi-Fi is not up in the emulator. Open Settings > Wi-Fi, enable Network Access, join PocketBook-QEMU, then rerun 'make emulator-sync'."
fi

status="$(serial "wpa_cli -i $iface status" 2>&1 || true)"
if ! grep -q 'wpa_state=COMPLETED' <<<"$status"; then
    echo "selecting the saved PocketBook-QEMU network on $iface"
    serial "wpa_cli -i $iface select_network 0" "sleep 4" >/dev/null || true
fi

if ! serial "ifconfig $iface" 2>/dev/null | grep -q 'inet addr:'; then
    echo "requesting a DHCP lease on $iface"
    serial "udhcpc -i $iface -q -n" >/dev/null || true
fi

serial "ifconfig $iface" | grep -q 'inet addr:' \
    || die "Wi-Fi has no IP address; check Settings > Wi-Fi in the emulator"

echo "uploading pbanki.app"
PBANKI_SERIAL_TIMEOUT="${PBANKI_UPLOAD_TIMEOUT:-300}" serial \
    "wget -O /mnt/ext1/applications/pbanki.app.new http://10.0.2.2:$PORT/pbanki.app" \
    "mv /mnt/ext1/applications/pbanki.app.new /mnt/ext1/applications/pbanki.app" \
    'sync' \
    'ls -l /mnt/ext1/applications/pbanki.app'

echo
echo "pbanki.app updated. Quit pbAnki (Home) and relaunch it from Apps."
