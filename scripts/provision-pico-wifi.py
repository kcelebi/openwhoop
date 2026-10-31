#!/usr/bin/env python3
"""Provision a Pico from a named local 1Password item without echoing credentials.
Run using: uv run --with mpremote python scripts/provision-pico-wifi.py --item 'title'
"""

import argparse
import glob
import json
import os
import re
import subprocess
from mpremote.transport_serial import SerialTransport


def key(text):
    return re.sub(r"[^a-z0-9]", "", text.lower())


def field(item, wanted):
    for f in item.get("fields", []):
        if wanted in (key(f.get("label", "")), key(f.get("id", ""))):
            return f.get("value", "")
    return ""


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--item", required=True)
    parser.add_argument("--port")
    parser.add_argument(
        "--vault", help="Narrow the named item to a specific local vault"
    )
    parser.add_argument(
        "--inspect",
        action="store_true",
        help="Show field labels/types/population only, never values",
    )
    args = parser.parse_args()
    if os.getenv("OP_SERVICE_ACCOUNT_TOKEN"):
        raise SystemExit(
            "Use the Mac's local 1Password desktop account for apartment Wi-Fi, not a service-account token."
        )
    command = ["op", "item", "get", args.item, "--format=json"]
    if args.vault:
        command.extend(["--vault", args.vault])
    result = subprocess.run(
        command,
        capture_output=True,
        text=True,
        timeout=120,
    )
    if result.returncode:
        raise SystemExit(
            "1Password could not read the named item. Unlock/authorize the local app and retry."
        )
    item = json.loads(result.stdout)
    if args.inspect:
        print(
            json.dumps(
                [
                    {
                        "label": f.get("label"),
                        "type": f.get("type"),
                        "populated": bool(f.get("value")),
                    }
                    for f in item.get("fields", [])
                ],
                indent=2,
            )
        )
        return
    ssid = field(item, "basestationname")
    password = field(item, "basestationpassword")
    if not ssid or not password:
        raise SystemExit(
            "The named item needs populated base station name and base station password fields. No credential values were printed."
        )
    ports = [args.port] if args.port else glob.glob("/dev/cu.usbmodem*")
    if len(ports) != 1:
        raise SystemExit("Connect exactly one Pico or specify --port.")
    transport = SerialTransport(ports[0], baudrate=115200, timeout=2)
    try:
        transport.enter_raw_repl()
        # The secret exists only in process memory and on the user's Pico. It is
        # not a subprocess argument, temporary Mac file, log line, or source file.
        payload = json.dumps({"ssid": ssid, "password": password})
        code = (
            "import json,os\n_c="
            + repr(payload)
            + "\n_f=open('wifi.json.tmp','w');_f.write(_c);_f.close()\nos.rename('wifi.json.tmp','wifi.json')\ndel _c,_f\nprint('Wi-Fi configuration saved on Pico')"
        )
        output, error = transport.exec_raw(code, timeout=10)
        if error:
            raise SystemExit(
                "Could not save Pico Wi-Fi configuration; no credential values were printed."
            )
        print(output.decode().strip())
        code = """import network,time,json
_cfg=json.load(open('wifi.json'))
_w=network.WLAN(network.STA_IF);_w.active(True)
_names=[row[0] for row in _w.scan()]
print('Configured network visible:',_cfg['ssid'].encode() in _names)
if _cfg['ssid'].encode() not in _names:raise RuntimeError('Configured SSID not visible in the 2.4 GHz scan')
_w.connect(_cfg['ssid'],_cfg['password'])
del _cfg
_deadline=time.ticks_add(time.ticks_ms(),30000)
while not _w.isconnected() and _w.status()>=0 and time.ticks_diff(_deadline,time.ticks_ms())>0:time.sleep_ms(200)
print('Wi-Fi connected:',_w.isconnected(),'status:',_w.status())
if not _w.isconnected():raise RuntimeError('Wi-Fi association failed')
print('Wi-Fi RSSI:',_w.status('rssi'))
"""
        output, error = transport.exec_raw(code, timeout=45)
        print(output.decode().strip())
        if error:
            # Fixed diagnostic code contains no secrets; never echo credential
            # injection code or a 1Password response on failure.
            raise SystemExit(
                "Pico Wi-Fi validation failed; verify the base station fields describe the 2.4 GHz Wi-Fi network."
            )
    finally:
        try:
            transport.exit_raw_repl()
        finally:
            transport.close()


if __name__ == "__main__":
    main()
