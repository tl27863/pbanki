#!/usr/bin/env bash
# Runs pbAnki in the $SDK PC emulator window.
#
# The working directory holds the device data tree as ./system (the SDK's PC
# InkView reads everything relative to it); system/mnt/ext1 is also mounted as
# /mnt/ext1 so pbAnki's device paths (config, collection) work unchanged.
# PROFILE selects the hwconfig profile (default: 700 for 6.10, 1030 for 6.11).
set -euo pipefail

source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

[ -f "$SDK_RUNTIME/.complete" ] && [ -d "$SDK_REPO_DIR/system" ] || {
    echo "error: SDK $SDK runtime missing; run: make sdk-emulator-setup SDK=$SDK" >&2
    exit 1
}
[ -x "$APP_BIN" ] || {
    echo "error: $APP_BIN missing; run: scripts/sdk-emulator/build.sh" >&2
    exit 1
}
docker image inspect "$IMAGE" >/dev/null 2>&1 || {
    echo "error: image $IMAGE missing; run: make sdk-emulator-setup SDK=$SDK" >&2
    exit 1
}
profile_file="$SDK_RUNTIME/config/hwconfig_$PROFILE"
[ -f "$profile_file" ] || {
    echo "error: no hwconfig profile '$PROFILE'; available:" >&2
    ls "$SDK_RUNTIME/config" | sed -n 's/^hwconfig_//p' | tr '\n' ' ' >&2
    echo >&2
    exit 1
}
[ -n "${DISPLAY:-}" ] || { echo "error: DISPLAY is not set (needs X11 or Xwayland)" >&2; exit 1; }

# First run: the device data tree from the SDK repo.
if [ ! -d "$WORK_DIR/system" ]; then
    mkdir -p "$WORK_DIR"
    cp -r "$SDK_REPO_DIR/system" "$WORK_DIR/system"
fi
cp "$SDK_RUNTIME/config"/hwconfig_* "$WORK_DIR/system/config/"
cp "$profile_file" "$WORK_DIR/system/config/hwconfig"
# The window size comes from the emulator's Display Mode (also in its
# right-click menu), stored as emu_resmode in global.cfg: 16 = 1264x1680 at
# 300 dpi (PB700/PB710 panel), 17 = the same at half window size, 9 = 1448x1072,
# 12 = 1404x1872, 2 = 758x1024.
sed -i "s/^emu_resmode=.*/emu_resmode=$RESMODE/" "$WORK_DIR/system/config/global.cfg"
mkdir -p "$EXT1_DIR/applications/pbanki/collection"

# First run: install a deck if one is available (PBANKI_DECK_APKG, default
# the local, untracked JLPT_N2_Grammar.apkg); init_session imports *.apkg on
# start.
deck="${PBANKI_DECK_APKG:-$HERE/JLPT_N2_Grammar.apkg}"
marker="$EXT1_DIR/applications/pbanki/.sample-deck"
if [ ! -e "$marker" ] && [ -f "$deck" ]; then
    cp "$deck" "$EXT1_DIR/applications/pbanki/collection/"
    touch "$marker"
fi

x11_args=(-e DISPLAY="$DISPLAY" -v /tmp/.X11-unix:/tmp/.X11-unix:ro)
if [ -n "${XAUTHORITY:-}" ] && [ -f "$XAUTHORITY" ]; then
    x11_args+=(-e XAUTHORITY=/tmp/.Xauthority -v "$XAUTHORITY:/tmp/.Xauthority:ro")
fi

tty_args=()
[ -t 0 ] && tty_args=(-it)

# Default: bridge network; the host is host.docker.internal. EMU_NET=host
# shares the host network so a dev sync server bound to 127.0.0.1 is reachable
# at http://127.0.0.1:27701/, as on the desktop.
if [ "${EMU_NET:-bridge}" = host ]; then
    net_args=(--network host)
else
    net_args=(--add-host=host.docker.internal:host-gateway)
fi

echo "starting pbAnki in the SDK $SDK emulator (profile $PROFILE, display mode $RESMODE)"
echo "data: $EXT1_DIR (= /mnt/ext1); log on stderr"
# PB_SDK_DIR: the PC InkView loads helper libraries from $PB_SDK_DIR/usr/local/lib.
exec docker run --rm "${tty_args[@]}" \
    --name "pbanki-sdk-emulator-${SDK/./-}" \
    "${x11_args[@]}" \
    "${net_args[@]}" \
    -v "$SDK_RUNTIME/lib:/opt/pbsdk/usr/local/lib:ro" \
    -v "$WORK_DIR:/work" \
    -v "$EXT1_DIR:/mnt/ext1" \
    -v "$APP_BIN:/work/pbanki.app:ro" \
    -e LD_LIBRARY_PATH=/opt/pbsdk/usr/local/lib \
    -e PB_SDK_DIR=/opt/pbsdk \
    ${INKVIEW_SLINT_UPDATES:+-e INKVIEW_SLINT_UPDATES="$INKVIEW_SLINT_UPDATES"} \
    ${INKVIEW_SLINT_TRACE_EVENTS:+-e INKVIEW_SLINT_TRACE_EVENTS=1} \
    -w /work \
    "$IMAGE" ./pbanki.app "$@"
