#!/usr/bin/env python3
"""Independent launchd check: restart the relay when saved HR stops advancing."""
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import sys
import time


def check(database, state_path):
    now = time.monotonic()
    sample = None
    try:
        with sqlite3.connect(f"file:{database}?mode=ro", uri=True, timeout=2) as connection:
            sample = connection.execute("select max(received_at_ms) from cloud_hr_outbox").fetchone()[0]
    except sqlite3.Error:
        pass
    path = Path(state_path)
    try:
        state = json.loads(path.read_text())
    except (OSError, ValueError):
        state = {}
    last_progress = state.get("last_progress", now)
    if sample != state.get("sample") or now < last_progress:
        last_progress = now
    if now - last_progress >= 90:
        print("Relay watchdog: saved HR stalled for 90 seconds; restarting relay", flush=True)
        subprocess.run(["/bin/launchctl", "kickstart", "-k", f"gui/{os.getuid()}/com.openwhoop.relay"], check=True, timeout=15)
        last_progress = now
    temporary = path.with_suffix(".tmp")
    temporary.write_text(json.dumps({"sample": sample, "last_progress": last_progress}))
    temporary.replace(path)


if __name__ == "__main__":
    check(*sys.argv[1:])
