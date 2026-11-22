#!/bin/sh
# Checks a PocketBook release bundle (notes/build.md, "Toolchain" checks):
#   verify-bundle.sh <bundle dir> soft|hard
# - pbanki.app and pbanki.bin exist
# - the binary needs glibc 2.38 or lower (the firmware 6.10 floor)
# - the ELF float ABI matches the build (soft: B288/B300, hard: RK3566)
# - soft builds contain hardware float instructions (softfp, performance.md)
set -eu

DIR=${1:?usage: verify-bundle.sh <bundle dir> soft|hard}
ABI=${2:?usage: verify-bundle.sh <bundle dir> soft|hard}
BIN="$DIR/pbanki.bin"
MAX_GLIBC=2.38

fail() { echo "error: $*" >&2; exit 1; }

[ -x "$DIR/pbanki.app" ] || fail "$DIR/pbanki.app missing or not executable"
[ -x "$BIN" ] || fail "$BIN missing or not executable"

glibc=$(readelf -V "$BIN" | grep -o 'GLIBC_[0-9][0-9.]*' | sed 's/GLIBC_//' | sort -uV | tail -n 1)
[ -n "$glibc" ] || fail "no GLIBC version needs found in $BIN"
highest=$(printf '%s\n%s\n' "$glibc" "$MAX_GLIBC" | sort -V | tail -n 1)
[ "$highest" = "$MAX_GLIBC" ] || fail "binary needs GLIBC_$glibc, the limit is $MAX_GLIBC"
echo "glibc: needs $glibc (limit $MAX_GLIBC)"

flags=$(readelf -h "$BIN" | sed -n 's/^ *Flags: *//p')
case "$flags" in
    *"$ABI-float ABI"*) echo "float ABI: $flags" ;;
    *) fail "expected $ABI-float ABI, ELF flags are: $flags" ;;
esac

if [ "$ABI" = soft ]; then
    objdump=$(command -v llvm-objdump || ls /usr/bin/llvm-objdump-* 2>/dev/null | sort -V | tail -n 1 || true)
    [ -n "$objdump" ] || fail "llvm-objdump not found"
    count=$("$objdump" -d "$BIN" | grep -cE 'v(mul|add|mla)\.f(32|64)' || true)
    [ "$count" -gt 0 ] || fail "no hardware float instructions: the softfp RUSTFLAGS were not applied"
    echo "softfp: $count hardware float instructions"
fi

ls -lh "$DIR/pbanki.app" "$BIN"
