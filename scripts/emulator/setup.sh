#!/usr/bin/env bash
set -euo pipefail

source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

require git
require curl
require python3
require cmake
require ninja
require pkg-config
require cc
require debugfs
require mke2fs
require resize2fs

pkg-config --exists glib-2.0 || die "glib development files are required to build QEMU"
pkg-config --exists pixman-1 || die "pixman development files are required to build QEMU"

if [ ! -d "$EMU_DIR/.git" ]; then
    echo "cloning eink-emulator into $EMU_DIR"
    git clone --depth 1 "$EMULATOR_REPO" "$EMU_DIR"
fi
[ -x "$EMU_DIR/eink" ] || die "$EMU_DIR does not look like the eink-emulator checkout"

# The upstream .gitmodules points QEMU at git@github.com:, which requires an SSH
# key. Use HTTPS so setup works on a normal development machine.
git -C "$EMU_DIR" config -f .gitmodules submodule.qemu.url https://github.com/katadelos/qemu.git
git -C "$EMU_DIR" submodule sync --recursive >/dev/null 2>&1 || true

if [ ! -x "$EMU_DIR/qemu/build/qemu-system-arm" ]; then
    echo "initialising QEMU submodule and building qemu-system-arm (this takes a few minutes)"
    git -C "$EMU_DIR" submodule update --init --depth 1 qemu
    "$EMU_DIR/eink" build
else
    echo "QEMU already built"
fi

mkdir -p "$EMU_DIR/downloads"
if [ ! -d "$EMU_DIR/firmware/$EMULATOR_MODEL" ]; then
    if [ ! -f "$FIRMWARE_ZIP" ]; then
        echo "downloading PB700 firmware (about 453 MiB)"
        curl -L --fail --retry 3 -o "$FIRMWARE_ZIP" "$FIRMWARE_URL"
    fi
    echo "importing PB700 firmware"
    "$EMU_DIR/eink" import "$FIRMWARE_ZIP" --model "$EMULATOR_MODEL"
else
    echo "PB700 firmware already imported"
fi

CREATED=0
if [ ! -d "$EMU_DIR/machines/$MACHINE" ]; then
    echo "creating emulator machine '$MACHINE'"
    "$EMU_DIR/eink" create "$MACHINE" --model "$EMULATOR_MODEL"
    CREATED=1
else
    echo "emulator machine '$MACHINE' already exists"
fi

if [ "$CREATED" = "1" ] && [ "${PBANKI_SKIP_WIZARD:-0}" != "1" ]; then
    echo "booting headless to complete the PocketBook setup wizard"
    log="$EMU_DIR/machines/$MACHINE/pbanki-wizard.log"
    "$EMU_DIR/eink" run "$MACHINE" --headless --qmp-socket --serial-socket \
        --ssh-port 4022 --wifi-ssh-port 4023 >"$log" 2>&1 &
    emu_pid=$!
    cleanup() {
        kill "$emu_pid" 2>/dev/null || true
    }
    trap cleanup EXIT

    if python3 "$ROOT/scripts/emulator/complete-setup.py" \
        --emulator-dir "$EMU_DIR" \
        --machine "$MACHINE" \
        --screenshot "$ROOT/target/emulator/setup-complete.png"; then
        touch "$EMU_DIR/machines/$MACHINE/.pbanki-setup-complete"
        python3 "$EMU_DIR/scripts/eink-qmp.py" --machine "$MACHINE" quit >/dev/null 2>&1 || true
        wait "$emu_pid" 2>/dev/null || true
        trap - EXIT
        echo "setup wizard completed"
    else
        die "setup wizard automation failed; see $log"
    fi
elif [ "$CREATED" = "1" ]; then
    echo "skipping setup wizard (PBANKI_SKIP_WIZARD=1)"
fi

echo
echo "emulator ready. Start it with:"
echo "  make emulator-run"
echo
echo "Then build/upload the app with:"
echo "  make emulator-sync"
echo
echo "If this is the first run and the wizard was skipped, complete it manually"
echo "or run scripts/emulator/complete-setup.py while the emulator is running."
