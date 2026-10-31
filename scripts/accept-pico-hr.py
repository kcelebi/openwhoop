#!/usr/bin/env python3
"""Bounded physical Pico HR acceptance with explicit Mac handoff and durable capture."""

import argparse
import json
import os
from pathlib import Path
import signal
import sqlite3
import subprocess
import threading
import time
import uuid


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--handoff",
        action="store_true",
        help="Temporarily stop the Mac relay/watchdog and restore both after the test",
    )
    parser.add_argument(
        "--database", default=str(Path.home() / ".local/share/openwhoop/relay.sqlite")
    )
    args = parser.parse_args()
    if not args.handoff:
        parser.error(
            "--handoff is required; this test briefly takes BLE ownership from the Mac"
        )
    root = Path(__file__).resolve().parent.parent
    domain = f"gui/{os.getuid()}"
    stopped = []
    process = timer = None
    accepted = 0
    try:
        for label in ["com.openwhoop.watchdog", "com.openwhoop.relay"]:
            subprocess.run(
                ["launchctl", "bootout", domain + "/" + label], check=True, timeout=10
            )
            stopped.append(label)
        time.sleep(2)
        process = subprocess.Popen(
            [
                "uv",
                "run",
                "--with",
                "mpremote",
                "mpremote",
                "connect",
                "auto",
                "run",
                str(root / "firmware/pico-w/diagnostics/hr_probe.py"),
            ],
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            bufsize=1,
            start_new_session=True,
        )

        def expire():
            if process.poll() is None:
                os.killpg(process.pid, signal.SIGTERM)

        timer = threading.Timer(50, expire)
        timer.start()
        connection = sqlite3.connect(args.database, timeout=10)
        try:
            for line in process.stdout:
                print(line.rstrip(), flush=True)
                try:
                    payload = json.loads(line)
                except ValueError:
                    continue
                if payload.get("source") != "pico_ble_diagnostic":
                    continue
                now = round(time.time() * 1000)
                identifier = "pico-diagnostic:" + str(uuid.uuid4())
                raw = {
                    "characteristic": "00002a37-0000-1000-8000-00805f9b34fb",
                    "bytes": payload.pop("raw_bytes"),
                    "decoder_version": 1,
                    "diagnostic": True,
                }
                payload.update(
                    imu_data=[],
                    sensor_data=None,
                    decoder_version=1,
                    timestamp_basis="host_receipt_utc",
                    diagnostic=True,
                )
                with connection:
                    connection.execute(
                        "insert into telemetry_outbox values(?,?,?,?,?,false)",
                        (identifier + ":raw", "raw_ble", None, now, json.dumps(raw)),
                    )
                    connection.execute(
                        "insert into telemetry_outbox values(?,?,?,?,?,false)",
                        (identifier, "reading", now, now, json.dumps(payload)),
                    )
                accepted += 1
        finally:
            connection.close()
        code = process.wait(timeout=5)
        if code or accepted < 5:
            raise RuntimeError(
                f"Pico acceptance incomplete: exit={code}, durable samples={accepted}"
            )
        print(
            f"PASS: {accepted} Pico HR/RR notifications durably preserved for cloud upload",
            flush=True,
        )
    finally:
        if timer:
            timer.cancel()
        if process and process.poll() is None:
            os.killpg(process.pid, signal.SIGTERM)
            process.wait(timeout=5)
        for label in reversed(stopped):
            subprocess.run(
                [
                    "launchctl",
                    "bootstrap",
                    domain,
                    str(Path.home() / "Library/LaunchAgents" / f"{label}.plist"),
                ],
                check=True,
                timeout=15,
            )
        if stopped:
            print("Mac relay and watchdog restored", flush=True)


if __name__ == "__main__":
    main()
