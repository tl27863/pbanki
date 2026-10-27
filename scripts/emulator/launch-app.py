#!/usr/bin/env python3
"""Launch pbAnki from the PocketBook app list."""

from __future__ import annotations

import argparse
from pathlib import Path

from qmp import DEFAULT_EMULATOR_DIR, Emulator


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--emulator-dir", type=Path, default=DEFAULT_EMULATOR_DIR)
    parser.add_argument("--machine", default="pb700")
    parser.add_argument("--width", type=int, default=1264)
    parser.add_argument("--height", type=int, default=1680)
    parser.add_argument(
        "--screenshot",
        type=Path,
        default=Path("target/emulator/pbanki-running.png"),
    )
    args = parser.parse_args()

    emu = Emulator(args.emulator_dir, args.machine, args.width, args.height)
    emu.wait_for_shell()

    # Library -> Apps -> USER'S -> @pbanki.
    emu.tap(1185, 1560, settle=3.0)
    emu.drag(600, 1400, 500, settle=2.0)
    emu.tap(120, 1510, settle=8.0)
    emu.screendump(args.screenshot)
    print(f"pbAnki launched; screenshot: {args.screenshot}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
