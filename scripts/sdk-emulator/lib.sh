# Shared settings for the PocketBook SDK PC emulator scripts.
#
# The emulator is the SDK's x86-64 desktop build of InkView (a GTK2 window),
# run in a container whose Debian release matches the SDK's libraries:
#   6.10 (B288)   -> bookworm (glibc 2.36, ICU 72, jsoncpp 25)
#   6.11 (RK3566) -> trixie   (glibc 2.41, ICU 76, jsoncpp 26)
# pbAnki is built for x86_64 and loads the SDK's libinkview.so at runtime.

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
HERE="$ROOT/scripts/sdk-emulator"

SDK="${SDK:-6.10}"
case "$SDK" in
    6.10)
        SDK_ARCHIVE="SDK-B288-6.10.7z"
        SDK_TAG="6.10"
        # Path of the SDK root inside the archive.
        SDK_PREFIX="SDK-B288-6.10/usr"
        SDK_FEATURE="sdk-6-10"
        # libinkview.so needs GLIBC_2.34; bookworm has 2.36.
        GLIBC="2.34"
        DEFAULT_PROFILE="700"
        # 1264x1680 at 300 dpi, the PB700/PB710 panel.
        DEFAULT_RESMODE="16"
        ;;
    6.11)
        SDK_ARCHIVE="SDK-RK3566-6.11.7z"
        SDK_TAG="6.11"
        SDK_PREFIX="arm-buildroot-linux-gnueabihf_sdk-buildroot"
        SDK_FEATURE="sdk-6-11"
        GLIBC="2.38"
        DEFAULT_PROFILE="1030"
        # 1404x1872, the 1030 profile's panel.
        DEFAULT_RESMODE="12"
        ;;
    *)
        echo "error: SDK must be 6.10 or 6.11 (got '$SDK')" >&2
        exit 1
        ;;
esac

# hwconfig profile copied to system/config/hwconfig: 700 is the PB700 Era
# (1264x1680, the PB710's panel); 1030 is the RK3566 device.
PROFILE="${PROFILE:-$DEFAULT_PROFILE}"
# Emulator Display Mode (emu_resmode), matched to the profile's panel by
# default. See run.sh for other values.
RESMODE="${RESMODE:-$DEFAULT_RESMODE}"

# Third-party re-upload of PocketBook's SDK; archives are pinned in SHA256SUMS.
SDK_RELEASES="https://github.com/Sean-on-Git/PocketBook-SDK/releases/download"
# The same repo's git tree holds the emulator's system/ data (theme, fonts,
# language). Pinned to the only commit.
SDK_REPO="https://github.com/Sean-on-Git/PocketBook-SDK"
SDK_REPO_COMMIT="715cda6"

SDK_DIR="$ROOT/target/sdk/$SDK"
# Archive download (untrusted data; nothing is executed from here on the host).
SDK_DL="$SDK_DIR/dl"
# The PC runtime extracted from the archive: lib/ (libinkview.so, ...) and
# share/config/ (hwconfig profiles).
SDK_RUNTIME="$SDK_DIR/runtime"
SDK_REPO_DIR="$ROOT/target/sdk/repo"

EMU_DIR="$ROOT/target/sdk-emulator/$SDK"
# Working directory of the emulated app; ./system is the device's data tree
# and system/mnt/ext1 is mounted as /mnt/ext1.
WORK_DIR="$EMU_DIR/work"
EXT1_DIR="$WORK_DIR/system/mnt/ext1"
# EMU_APP runs another x86-64 InkView binary instead of pbAnki (e.g. the
# inkview-rs waveform_probe example).
APP_BIN="${EMU_APP:-$EMU_DIR/pbanki}"

IMAGE="pbanki-sdk-emulator:$SDK"
RUST_TARGET="x86_64-unknown-linux-gnu"
