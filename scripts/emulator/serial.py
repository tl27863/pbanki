#!/usr/bin/env python3
"""Run shell commands on the PB700 emulator's serial console."""

from __future__ import annotations

import argparse
import os
import socket
import sys
import time
import uuid
from pathlib import Path

DEFAULT_EMULATOR_DIR = Path(__file__).resolve().parents[2].parent / "eink-emulator"


def socket_path(emulator_dir: Path, machine: str) -> Path:
    return emulator_dir / "machines" / machine / f"{machine}.serial.sock"


def run_commands(sock_path: Path, commands: list[str], timeout: float) -> int:
    sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    sock.settimeout(timeout)
    sock.connect(str(sock_path))
    # Discard anything the console emitted while no client was attached.
    sock.settimeout(0.3)
    try:
        while sock.recv(65536):
            pass
    except socket.timeout:
        pass
    sock.settimeout(timeout)

    for command in commands:
        token = f"__PBANKI_{uuid.uuid4().hex}__"
        payload = f"\r\n{command}\r\necho {token}\r\n"
        sock.sendall(payload.encode())
        output = b""
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            try:
                chunk = sock.recv(65536)
            except socket.timeout:
                break
            if not chunk:
                break
            output += chunk
            # Once by the terminal echo, once by the echo command itself.
            if output.count(token.encode()) >= 2:
                break
        text = output.decode(errors="replace")
        # Strip the trailing marker and prompt so callers see the real output.
        marker = text.rfind(token)
        if marker >= 0:
            text = text[:marker]
        print(f"$ {command}")
        print(text.rstrip())
        if token.encode() not in output:
            print(f"error: no response marker for: {command}", file=sys.stderr)
            return 1
    sock.close()
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("commands", nargs="+", help="shell command(s) to execute")
    parser.add_argument(
        "--emulator-dir",
        type=Path,
        default=Path(os.environ.get("EINK_EMULATOR_DIR", DEFAULT_EMULATOR_DIR)),
    )
    parser.add_argument(
        "--machine",
        default=os.environ.get("PBANKI_EMULATOR_MACHINE", "pb700"),
    )
    parser.add_argument(
        "--timeout",
        type=float,
        default=float(os.environ.get("PBANKI_SERIAL_TIMEOUT", "10")),
    )
    args = parser.parse_args()

    path = socket_path(args.emulator_dir, args.machine)
    if not path.exists():
        print(f"error: serial socket not found: {path}", file=sys.stderr)
        print("start the emulator first: make emulator-run", file=sys.stderr)
        return 2
    return run_commands(path, args.commands, args.timeout)


if __name__ == "__main__":
    raise SystemExit(main())
