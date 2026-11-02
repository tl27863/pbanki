#!/bin/sh
# Blitz render benchmark (bench/blitz-render). Runs headless and exits; the
# screen does not change. Results: blitz-bench-out/blitz-bench.log and the
# PGM frames beside it. Takes about a minute on the PB710.

# Same tunable as pbanki.app. The bench does not load libinkview.so, so it is
# not strictly needed; set it so the environment matches pbAnki.
export GLIBC_TUNABLES="${GLIBC_TUNABLES}${GLIBC_TUNABLES:+:}glibc.rtld.execstack=2"

DIR="$(CDPATH= cd "$(dirname "$0")" && pwd)"
cd "$DIR" || exit 1

OUT="$DIR/blitz-bench-out"
mkdir -p "$OUT" || exit 1

for path in "$DIR"/blitz-bench*.bin; do
    [ -x "$path" ] || continue
    bin="${path##*/}"
    name="${bin%.bin}"
    mkdir -p "$OUT/$name"
    "$DIR/$bin" --out "$OUT/$name" "$@" >"$OUT/$name/stderr.txt" 2>&1
    rc=$?
    printf '%s %s exited with status %s\n' "$(date '+%Y-%m-%d %H:%M:%S')" "$bin" "$rc" \
        >>"$OUT/runs.txt"
done
