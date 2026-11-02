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

# One log file per launch: pbanki/logs/pbanki-YYYYMMDD-HHMMSS.log. The names
# sort by start time; the newest KEEP_LOGS files are kept.
LOG_DIR="$DIR/pbanki/logs"
KEEP_LOGS=20
PIDFILE="/tmp/pbanki.pid"

if mkdir -p "$LOG_DIR" 2>/dev/null; then
    # One-time move of the single log of older launchers (beside the launcher,
    # then in pbanki/logs/). The name does not match the session pattern, so
    # pruning never deletes it.
    for old in "$LOG_DIR/pbanki.log" "$DIR/pbanki.log"; do
        if [ -f "$old" ] && [ ! -e "$LOG_DIR/pbanki-legacy.log" ]; then
            mv -f "$old" "$LOG_DIR/pbanki-legacy.log" 2>/dev/null || true
        fi
    done
fi

# A second launch should focus the running window instead of opening the
# collection twice. /tmp survives crashes, so validate a stale pid first. The
# pidfile holds the pid and, on its second line, the session log of that
# instance, which receives the activation messages.
if [ -f "$PIDFILE" ]; then
    running=$(sed -n 1p "$PIDFILE" 2>/dev/null)
    running_log=$(sed -n 2p "$PIDFILE" 2>/dev/null)
    case "$running" in
        '' | *[!0-9]*) running= ;;
    esac
    if [ -z "$running_log" ] || ! ( : >>"$running_log" ) 2>/dev/null; then
        running_log=/dev/null
    fi
    if [ -n "$running" ] && [ -d "/proc/$running" ]; then
        cmdline=$(tr '\0' ' ' <"/proc/$running/cmdline" 2>/dev/null)
        case "$cmdline" in
            *pbanki.app* | *pbanki.bin*)
                printf '%s already running (pid %s), activating\n' \
                    "$(date '+%Y-%m-%d %H:%M:%S')" "$running" >>"$running_log"
                iv2sh=/usr/bin/iv2sh
                [ -x "$iv2sh" ] || iv2sh=/ebrmain/bin/iv2sh
                if [ -x "$iv2sh" ]; then
                    if ! "$iv2sh" SetActiveTask "$running" 0 >>"$running_log" 2>&1; then
                        printf '%s warning: could not activate pid %s\n' \
                            "$(date '+%Y-%m-%d %H:%M:%S')" "$running" >>"$running_log"
                    fi
                else
                    printf '%s warning: iv2sh not found; cannot activate pid %s\n' \
                        "$(date '+%Y-%m-%d %H:%M:%S')" "$running" >>"$running_log"
                fi
                exit 0
                ;;
        esac
    fi
    rm -f "$PIDFILE"
fi

SESSION="pbanki-$(date '+%Y%m%d-%H%M%S')"
LOG="$LOG_DIR/$SESSION.log"
[ -e "$LOG" ] && LOG="$LOG_DIR/$SESSION-$$.log"

# If the app partition is read-only or full, fall back to /tmp so pbAnki still
# starts. The subshell keeps a redirection failure on the special builtin `:`
# from aborting the whole script (POSIX makes that fatal).
PRIMARY_LOG="$LOG"
if ! ( : >>"$LOG" ) 2>/dev/null; then
    LOG="/tmp/$SESSION-$$.log"
    if ! ( : >>"$LOG" ) 2>/dev/null; then
        printf '%s error: cannot write %s or %s\n' \
            "$(date '+%Y-%m-%d %H:%M:%S')" "$PRIMARY_LOG" "$LOG" >&2
        exit 1
    fi
    printf '%s warning: cannot use %s; logging to %s\n' \
        "$(date '+%Y-%m-%d %H:%M:%S')" "$PRIMARY_LOG" "$LOG" >>"$LOG"
fi

# Deletes the oldest session logs beyond KEEP_LOGS. A function, so `set --`
# does not replace the launcher's arguments. The current log is never deleted,
# also if a wrong clock makes it sort first.
prune_logs() {
    set -- "$LOG_DIR"/pbanki-[0-9]*.log
    [ -e "$1" ] || return 0
    excess=$(($# - KEEP_LOGS))
    while [ "$excess" -gt 0 ]; do
        [ "$1" = "$LOG" ] || rm -f "$1"
        shift
        excess=$((excess - 1))
    done
}
prune_logs

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

if ! printf '%s\n%s\n' "$$" "$LOG" >"$PIDFILE" 2>/dev/null; then
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
