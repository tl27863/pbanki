#!/bin/sh
# PocketBook firmware 6.11.1480+ refuses to load system libraries that request
# an executable stack. KOReader uses the same glibc tunable workaround.
export GLIBC_TUNABLES="${GLIBC_TUNABLES}${GLIBC_TUNABLES:+:}glibc.rtld.execstack=2"

DIR="$(CDPATH= cd "$(dirname "$0")" && pwd)"
LOG="${DIR}/pbanki.log"

printf '%s starting pbAnki from %s\n' "$(date '+%Y-%m-%d %H:%M:%S')" "$DIR" >>"$LOG"
exec "$DIR/pbanki.bin" "$@" >>"$LOG" 2>&1
