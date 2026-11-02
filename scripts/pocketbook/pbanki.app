#!/bin/sh
# PocketBook firmware 6.11.1480+ refuses to load system libraries that request
# an executable stack. KOReader uses the same glibc tunable workaround.
export GLIBC_TUNABLES="${GLIBC_TUNABLES}${GLIBC_TUNABLES:+:}glibc.rtld.execstack=2"

DIR="$(CDPATH= cd "$(dirname "$0")" && pwd)"

# Normally this is the directory containing pbanki.bin. Check that it exists
# explicitly so a bad invocation cannot silently make relative paths resolve
# somewhere else.
if [ -z "$DIR" ] || [ ! -d "$DIR" ]; then
    printf '%s error: application directory not found (from %s)\n' \
        "$(date '+%Y-%m-%d %H:%M:%S')" "$0" >&2
    exit 1
fi

LOG_DIR="$DIR/pbanki/logs"
LOG="$LOG_DIR/pbanki.log"
PIDFILE="/tmp/pbanki.pid"

# Keep the log inside the pbanki data folder. Create it when missing; if the
# app partition is read-only or full, fall back to /tmp so pbAnki still starts.
# The subshell keeps a redirection failure on the special builtin `:` from
# aborting the whole script (POSIX makes that fatal).
PRIMARY_LOG="$LOG"
if mkdir -p "$LOG_DIR" 2>/dev/null; then
    # One-time move from the old location beside the launcher.
    if [ -f "$DIR/pbanki.log" ] && [ ! -e "$LOG" ]; then
        mv -f "$DIR/pbanki.log" "$LOG" 2>/dev/null || true
    fi
fi

if ! ( : >>"$LOG" ) 2>/dev/null; then
    LOG="/tmp/pbanki.log"
    if ! ( : >>"$LOG" ) 2>/dev/null; then
        printf '%s error: cannot write %s or %s\n' \
            "$(date '+%Y-%m-%d %H:%M:%S')" "$PRIMARY_LOG" "$LOG" >&2
        exit 1
    fi
    printf '%s warning: cannot use %s; logging to %s\n' \
        "$(date '+%Y-%m-%d %H:%M:%S')" "$PRIMARY_LOG" "$LOG" >>"$LOG"
fi

# A second launch should focus the running window instead of opening the
# collection twice. /tmp survives crashes, so validate a stale pid first.
if [ -f "$PIDFILE" ]; then
    running=$(cat "$PIDFILE" 2>/dev/null)
    case "$running" in
        '' | *[!0-9]*) running= ;;
    esac
    if [ -n "$running" ] && [ -d "/proc/$running" ]; then
        cmdline=$(tr '\0' ' ' <"/proc/$running/cmdline" 2>/dev/null)
        case "$cmdline" in
            *pbanki.app* | *pbanki.bin*)
                printf '%s already running (pid %s), activating\n' \
                    "$(date '+%Y-%m-%d %H:%M:%S')" "$running" >>"$LOG"
                iv2sh=/usr/bin/iv2sh
                [ -x "$iv2sh" ] || iv2sh=/ebrmain/bin/iv2sh
                if [ -x "$iv2sh" ]; then
                    if ! "$iv2sh" SetActiveTask "$running" 0 >>"$LOG" 2>&1; then
                        printf '%s warning: could not activate pid %s\n' \
                            "$(date '+%Y-%m-%d %H:%M:%S')" "$running" >>"$LOG"
                    fi
                else
                    printf '%s warning: iv2sh not found; cannot activate pid %s\n' \
                        "$(date '+%Y-%m-%d %H:%M:%S')" "$running" >>"$LOG"
                fi
                exit 0
                ;;
        esac
    fi
    rm -f "$PIDFILE"
fi

# Cap the log at 1 MiB, keeping the tail where crashes are.
size=$(wc -c <"$LOG" 2>/dev/null || echo 0)
if [ "$size" -gt 1048576 ] 2>/dev/null; then
    if tail -c 262144 "$LOG" >"$LOG.new"; then
        mv -f "$LOG.new" "$LOG" || rm -f "$LOG.new"
    else
        rm -f "$LOG.new"
    fi
fi

printf '%s starting pbAnki (pid %s) from %s\n' \
    "$(date '+%Y-%m-%d %H:%M:%S')" "$$" "$DIR" >>"$LOG"

if [ ! -x "$DIR/pbanki.bin" ]; then
    printf '%s error: %s is missing or not executable\n' \
        "$(date '+%Y-%m-%d %H:%M:%S')" "$DIR/pbanki.bin" >>"$LOG"
    exit 1
fi

# Relative collection_path/PBANKI_CONFIG values resolve to the app directory,
# as documented in README.md.
if ! cd "$DIR"; then
    printf '%s error: cannot enter %s\n' \
        "$(date '+%Y-%m-%d %H:%M:%S')" "$DIR" >>"$LOG"
    exit 1
fi

if ! printf '%s\n' "$$" >"$PIDFILE" 2>/dev/null; then
    printf '%s warning: cannot write %s; single-instance guard disabled\n' \
        "$(date '+%Y-%m-%d %H:%M:%S')" "$PIDFILE" >>"$LOG"
fi

# Keep a tiny supervisor, like KOReader: exiting via exec would discard this
# shell, losing the exit status and leaving the pidfile behind.
"$DIR/pbanki.bin" "$@" >>"$LOG" 2>&1
rc=$?
rm -f "$PIDFILE"

if [ "$rc" -eq 0 ]; then
    result="exited normally"
elif [ "$rc" -gt 128 ]; then
    result="terminated by signal $((rc - 128))"
else
    result="exited with status $rc"
fi
printf '%s pbAnki %s\n' "$(date '+%Y-%m-%d %H:%M:%S')" "$result" >>"$LOG"
# 126/127: the kernel or loader refused the binary. The usual cause is the
# wrong bundle: target/pocketbook is soft-float (B288/B300), and
# target/pocketbook-rk3566 is hard-float (RK3566).
if [ "$rc" -eq 126 ] || [ "$rc" -eq 127 ]; then
    if [ -e /lib/ld-linux-armhf.so.3 ]; then
        hint="this device is hard-float (RK3566): install the target/pocketbook-rk3566 bundle"
    else
        hint="this device is soft-float (B288/B300): install the target/pocketbook bundle"
    fi
    printf '%s hint: %s\n' "$(date '+%Y-%m-%d %H:%M:%S')" "$hint" >>"$LOG"
fi
exit "$rc"
