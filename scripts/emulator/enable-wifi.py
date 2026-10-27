#!/usr/bin/env python3
"""Ensure the PB700 emulator has Wi-Fi access to the PocketBook-QEMU network."""

from __future__ import annotations

import argparse
import re
import sys
import time
from pathlib import Path

from qmp import DEFAULT_EMULATOR_DIR, Emulator

SSID = "PocketBook-QEMU"


def has_ip(emu: Emulator) -> bool:
    return "inet addr:" in emu.serial("ifconfig eth0")


def interface_exists(emu: Emulator) -> bool:
    for _ in range(5):
        if "eth0" in emu.serial("ls /sys/class/net"):
            return True
        time.sleep(1)
    return False


def network_id(emu: Emulator) -> int | None:
    output = emu.serial("wpa_cli -i eth0 list_networks")
    for line in output.splitlines():
        if SSID in line:
            match = re.match(r"\s*(\d+)\s", line)
            if match:
                return int(match.group(1))
    return None


def connect(emu: Emulator) -> None:
    current = network_id(emu)
    if current is None:
        output = emu.serial("wpa_cli -i eth0 add_network")
        ids = [int(line.strip()) for line in output.splitlines() if line.strip().isdigit()]
        current = ids[-1] if ids else 0
    emu.serial(
        f"wpa_cli -i eth0 set_network {current} ssid '\"{SSID}\"'",
        f"wpa_cli -i eth0 set_network {current} key_mgmt NONE",
        f"wpa_cli -i eth0 select_network {current}",
    )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--emulator-dir", type=Path, default=DEFAULT_EMULATOR_DIR)
    parser.add_argument("--machine", default="pb700")
    parser.add_argument("--width", type=int, default=1264)
    parser.add_argument("--height", type=int, default=1680)
    args = parser.parse_args()

    emu = Emulator(args.emulator_dir, args.machine, args.width, args.height)
    emu.wait_for_shell()

    if has_ip(emu):
        print("Wi-Fi already connected")
        return 0

    if not interface_exists(emu):
        print("turning on Wi-Fi...")
        emu.tap(1055, 55, settle=2.0)   # quick settings
        emu.tap(170, 235, settle=5.0)   # Wi-Fi toggle
        emu.tap(630, 1560, settle=2.0)  # close quick settings
        for _ in range(15):
            if interface_exists(emu):
                break
            time.sleep(2)
        else:
            print("error: Wi-Fi interface did not appear", file=sys.stderr)
            return 1

    if has_ip(emu):
        print("Wi-Fi connected")
        return 0

    # Configure the open AP through wpa_cli; this avoids relying on UI row
    # positions, which differ between firmware versions and screens.
    for _ in range(3):
        connect(emu)
        for _ in range(5):
            if has_ip(emu):
                print("Wi-Fi connected")
                return 0
            time.sleep(2)
        emu.serial("udhcpc -i eth0 -q -n", timeout=30)
        if has_ip(emu):
            print("Wi-Fi connected")
            return 0

    print(
        "error: Wi-Fi did not connect; enable it in Settings > Wi-Fi and rerun "
        "'make emulator-sync'",
        file=sys.stderr,
    )
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
