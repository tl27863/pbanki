#!/usr/bin/env bash
set -euo pipefail

source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

MACHINE="${PBANKI_EMULATOR_MACHINE:-pb700}"
PORT="${PBANKI_DECK_HTTP_PORT:-8001}"
APKG="${PBANKI_DECK_APKG:-$ROOT/scripts/emulator/JLPT_N2_Grammar.apkg}"
DECK_DB="$ROOT/target/emulator/deck-collection.anki2"

[ -f "$APKG" ] || {
    echo "warning: deck package not found: $APKG (set PBANKI_DECK_APKG)" >&2
    exit 0
}

python3 - "$APKG" "$DECK_DB" <<'PY'
import sys
import zipfile
from pathlib import Path

apkg, output = Path(sys.argv[1]), Path(sys.argv[2])
output.parent.mkdir(parents=True, exist_ok=True)
with zipfile.ZipFile(apkg) as archive:
    names = archive.namelist()
    for name in ("collection.anki21", "collection.anki2"):
        if name in names:
            data = archive.read(name)
            if not data.startswith(b"SQLite format 3\x00"):
                raise SystemExit(f"{apkg}: {name} is not an uncompressed SQLite collection")
            output.write_bytes(data)
            break
    else:
        raise SystemExit(f"{apkg}: no collection database found")
PY

serial() {
    python3 "$ROOT/scripts/emulator/serial.py" \
        --emulator-dir "$EMU_DIR" --machine "$MACHINE" "$@"
}

serial 'echo ok' >/dev/null || die "emulator serial console is not available"

running="$(serial 'pidof pbanki.app 2>/dev/null' | tr -d '\r' | grep -E '^[0-9]+$' || true)"
if [ -n "$running" ]; then
    die "pbAnki is running; quit it (Home) before installing the deck"
fi

if [ "${PBANKI_REINSTALL_DECK:-0}" != "1" ]; then
    installed="$(serial 'if [ -f /mnt/ext1/applications/pbanki/collection/.pbanki-deck ]; then wc -c < /mnt/ext1/applications/pbanki/collection/collection.anki2; else echo 0; fi' | tr -d '\r' | grep -E '^[0-9]+$' | tail -1 || true)"
    if [ -n "$installed" ] && [ "$installed" -gt 100000 ]; then
        echo "deck already installed"
        exit 0
    fi
fi

python3 -m http.server "$PORT" --bind 0.0.0.0 --directory "$(dirname "$DECK_DB")" \
    >/tmp/pbanki-emulator-deck-http.log 2>&1 &
server_pid=$!
trap 'kill "$server_pid" 2>/dev/null || true' EXIT
sleep 1

echo "installing deck $(basename "$APKG")..."
PBANKI_SERIAL_TIMEOUT="${PBANKI_UPLOAD_TIMEOUT:-120}" serial \
    'mkdir -p /mnt/ext1/applications/pbanki/collection' \
    'rm -f /mnt/ext1/applications/pbanki/collection/.pbanki-deck /mnt/ext1/applications/pbanki/collection/collection.anki2-wal /mnt/ext1/applications/pbanki/collection/collection.anki2-shm' \
    "wget -O /mnt/ext1/applications/pbanki/collection/collection.anki2.new http://10.0.2.2:$PORT/deck-collection.anki2" \
    'mv /mnt/ext1/applications/pbanki/collection/collection.anki2.new /mnt/ext1/applications/pbanki/collection/collection.anki2'

size="$(serial 'wc -c < /mnt/ext1/applications/pbanki/collection/collection.anki2' | tr -d '\r' | grep -E '^[0-9]+$' | tail -1 || true)"
if [ -z "$size" ] || [ "$size" -lt 100000 ]; then
    die "deck upload failed (collection size ${size:-unknown}); check $DECK_DB and port $PORT"
fi

serial 'touch /mnt/ext1/applications/pbanki/collection/.pbanki-deck' 'sync' >/dev/null
echo "deck installed (${size} bytes). pbAnki will open it on the next launch."
