#!/usr/bin/env python3
"""Complete the first-boot PocketBook setup wizard and land on the home screen.

Only intended for a PB700 emulator instance. Run the emulator with
--qmp-socket --serial-socket (see scripts/emulator/run.sh) first.
"""

from __future__ import annotations

import argparse
from pathlib import Path

from qmp import DEFAULT_EMULATOR_DIR, Emulator


def run_setup(emu: Emulator) -> None:
    # Language page: select English, then next.
    emu.tap(600, 1265, settle=2.0)
    emu.tap(1060, 1633, settle=3.0)
    # Wi-Fi page: skip, confirm. x=800 is safely inside the full-width SKIP bar.
    emu.tap(800, 1633, settle=3.0)
    emu.tap(890, 1188, settle=3.0)
    # Legal page: accept both documents, then next.
    emu.tap(110, 990, settle=1.5)
    emu.tap(110, 1096, settle=1.5)
    emu.tap(1060, 1633, settle=4.0)
    # Date/time page: accept.
    emu.tap(630, 1633, settle=8.0)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--emulator-dir", type=Path, default=DEFAULT_EMULATOR_DIR)
    parser.add_argument("--machine", default="pb700")
    parser.add_argument("--width", type=int, default=1264)
    parser.add_argument("--height", type=int, default=1680)
    parser.add_argument(
        "--screenshot",
        type=Path,
        default=Path("target/emulator/setup-complete.png"),
    )
    parser.add_argument(
        "--only-if-running",
        action="store_true",
        help="skip the wizard when setup_wizard.app is not running",
    )
    args = parser.parse_args()

    emu = Emulator(args.emulator_dir, args.machine, args.width, args.height)
    print("waiting for emulator boot...")
    emu.wait_for_boot()
    if args.only_if_running:
        emu.wait_for_shell()
        if "setup_wizard.app" not in emu.serial("ps"):
            print("setup wizard is not running; setup already complete")
            return 0
    print("running setup wizard...")
    run_setup(emu)
    emu.screendump(args.screenshot)
    print(f"setup complete; screenshot: {args.screenshot}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
