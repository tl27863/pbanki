#!/usr/bin/env python3
"""Shared QMP/serial helpers for driving the PB700 emulator."""

from __future__ import annotations

import json
import os
import subprocess
import sys
import time
from pathlib import Path

ABS_MAX = 32767
DEFAULT_EMULATOR_DIR = Path(__file__).resolve().parents[2].parent / "eink-emulator"
SERIAL_SCRIPT = Path(__file__).resolve().parent / "serial.py"


class Emulator:
    def __init__(
        self,
        emulator_dir: Path | str = DEFAULT_EMULATOR_DIR,
        machine: str = "pb700",
        width: int = 1264,
        height: int = 1680,
    ):
        self.dir = Path(emulator_dir)
        self.machine = machine
        self.width = width
        self.height = height
        self.qmp_script = self.dir / "scripts" / "eink-qmp.py"
        self.qmp_sock = self.dir / "machines" / machine / f"{machine}.qmp.sock"
        self.serial_log = self.dir / "machines" / machine / f"{machine}.serial.log"

    def qmp(self, events: list[dict]) -> None:
        subprocess.run(
            [
                sys.executable,
                str(self.qmp_script),
                "--machine",
                self.machine,
                "qmp",
                "input-send-event",
                "--arguments",
                json.dumps({"events": events}),
            ],
            check=True,
            stdout=subprocess.DEVNULL,
        )

    def tap(self, x: int, y: int, hold: float = 0.25, settle: float = 1.2) -> None:
        self.qmp([
            {"type": "abs", "data": {"axis": "x", "value": round(x * ABS_MAX / self.width)}},
            {"type": "abs", "data": {"axis": "y", "value": round(y * ABS_MAX / self.height)}},
        ])
        time.sleep(0.4)
        self.qmp([{"type": "btn", "data": {"down": True, "button": "left"}}])
        time.sleep(hold)
        self.qmp([{"type": "btn", "data": {"down": False, "button": "left"}}])
        time.sleep(settle)

    def drag(
        self,
        x: int,
        y0: int,
        y1: int,
        steps: int = 10,
        step_delay: float = 0.05,
        settle: float = 1.5,
    ) -> None:
        self.qmp(self._abs(x, y0))
        time.sleep(0.3)
        self.qmp([{"type": "btn", "data": {"down": True, "button": "left"}}])
        time.sleep(0.2)
        for i in range(1, steps + 1):
            y = y0 + (y1 - y0) * i // steps
            self.qmp(self._abs(x, y))
            time.sleep(step_delay)
        time.sleep(0.2)
        self.qmp([{"type": "btn", "data": {"down": False, "button": "left"}}])
        time.sleep(settle)

    def _abs(self, x: int, y: int) -> list[dict]:
        return [
            {"type": "abs", "data": {"axis": "x", "value": round(x * ABS_MAX / self.width)}},
            {"type": "abs", "data": {"axis": "y", "value": round(y * ABS_MAX / self.height)}},
        ]

    def screendump(self, path: Path) -> None:
        path.parent.mkdir(parents=True, exist_ok=True)
        subprocess.run(
            [
                sys.executable,
                str(self.qmp_script),
                "--machine",
                self.machine,
                "screendump",
                str(path),
            ],
            check=True,
            stdout=subprocess.DEVNULL,
        )

    def serial(self, *commands: str, timeout: float = 10.0) -> str:
        environment = os.environ.copy()
        environment["PBANKI_SERIAL_TIMEOUT"] = str(timeout)
        proc = subprocess.run(
            [
                sys.executable,
                str(SERIAL_SCRIPT),
                "--emulator-dir",
                str(self.dir),
                "--machine",
                self.machine,
                *commands,
            ],
            capture_output=True,
            text=True,
            env=environment,
        )
        return proc.stdout + proc.stderr

    def wait_for_shell(self, timeout: float = 30.0) -> None:
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if "ready" in self.serial("echo ready"):
                return
            time.sleep(1)
        raise TimeoutError("emulator serial console did not become ready")

    def wait_for_boot(self, timeout: float = 240.0, settle: float = 20.0) -> None:
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if self.qmp_sock.exists() and self.serial_log.exists():
                text = self.serial_log.read_text(errors="replace")
                if "Network Manager checker work done" in text:
                    time.sleep(settle)
                    return
            time.sleep(1)
        raise TimeoutError("emulator did not finish booting")
