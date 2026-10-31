#!/usr/bin/env python3
"""Read-only proof of recorded coverage and native buffered readings for a UTC window."""

import argparse
from datetime import datetime, timezone
import json
import sqlite3


def stamp(text):
    value = datetime.fromisoformat(text.replace("Z", "+00:00"))
    if value.tzinfo is None:
        raise argparse.ArgumentTypeError("Use an explicit timezone offset or Z")
    return int(value.timestamp() * 1000)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--database", required=True)
    parser.add_argument("--start", required=True, type=stamp)
    parser.add_argument("--end", required=True, type=stamp)
    args = parser.parse_args()
    if args.end <= args.start:
        parser.error("end must be after start")
    connection = sqlite3.connect(f"file:{args.database}?mode=ro", uri=True, timeout=5)
    try:
        sources = {}
        for source in ["gatt_live", "device_history"]:
            rows = connection.execute(
                "select recorded_at_ms,synced from telemetry_outbox where kind='reading' and json_extract(payload,'$.source')=? and recorded_at_ms>=? and recorded_at_ms<?",
                (source, args.start, args.end),
            ).fetchall()
            sources[source] = {
                "records": len(rows),
                "distinct_seconds": len({ms // 1000 for ms, _ in rows}),
                "cloud_pending": sum(not synced for _, synced in rows),
            }
        latest = connection.execute(
            "select max(recorded_at_ms) from telemetry_outbox where kind='reading' and json_extract(payload,'$.source')='device_history'"
        ).fetchone()[0]
        receipt = connection.execute(
            "select max(received_at_ms) from telemetry_outbox where kind='reading' and json_extract(payload,'$.source')='device_history'"
        ).fetchone()[0]
        print(
            json.dumps(
                {
                    "window_start": datetime.fromtimestamp(
                        args.start / 1000, timezone.utc
                    ).isoformat(),
                    "window_end": datetime.fromtimestamp(
                        args.end / 1000, timezone.utc
                    ).isoformat(),
                    "window_seconds": (args.end - args.start) // 1000,
                    "sources": sources,
                    "latest_buffered_device_time": datetime.fromtimestamp(
                        latest / 1000, timezone.utc
                    ).isoformat()
                    if latest
                    else None,
                    "latest_buffered_receipt_time": datetime.fromtimestamp(
                        receipt / 1000, timezone.utc
                    ).isoformat()
                    if receipt
                    else None,
                    "note": "Native buffered records in the requested interval prove backfill; live reconnect and older buffered records alone do not.",
                },
                indent=2,
            )
        )
    finally:
        connection.close()


if __name__ == "__main__":
    main()
