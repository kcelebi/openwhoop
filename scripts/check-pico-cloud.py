#!/usr/bin/env python3
"""Check certificate-verified Pico HTTPS while its BLE controller is active."""

import argparse
from datetime import datetime, timezone
import glob
from pathlib import Path
from mpremote.transport_serial import SerialTransport


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--host", required=True)
    parser.add_argument(
        "--ca-file",
        required=True,
        type=Path,
        help="DER root from an independently verified TLS chain",
    )
    parser.add_argument("--port")
    args = parser.parse_args()
    if not args.host or any(
        c not in "abcdefghijklmnopqrstuvwxyz0123456789.-" for c in args.host
    ):
        parser.error("host must be a plain DNS hostname")
    ports = [args.port] if args.port else glob.glob("/dev/cu.usbmodem*")
    if len(ports) != 1:
        parser.error("Connect exactly one Pico or specify --port")
    now = datetime.now(timezone.utc)
    clock = (
        now.year,
        now.month,
        now.day,
        now.weekday(),
        now.hour,
        now.minute,
        now.second,
        0,
    )
    transport = SerialTransport(ports[0], baudrate=115200, timeout=2)
    try:
        transport.enter_raw_repl()
        code = (
            "import machine\nmachine.RTC().datetime("
            + repr(clock)
            + ")\n_ca="
            + repr(args.ca_file.read_bytes())
            + "\n_f=open('cloud-root.der','wb');_f.write(_ca);_f.close()\ndel _ca,_f"
        )
        _, error = transport.exec_raw(code, timeout=10)
        if error:
            raise SystemExit("Could not initialize Pico clock/root certificate")
        code = (
            "_host="
            + repr(args.host)
            + "\n"
            + """
import network,socket,ssl,bluetooth,gc
_w=network.WLAN(network.STA_IF)
if not _w.isconnected():raise RuntimeError('Provision/connect Wi-Fi first')
_b=bluetooth.BLE();_b.active(True)
print('BLE controller active during HTTPS:',_b.active())
gc.collect();print('Free heap before TLS:',gc.mem_free())
_ctx=ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
_ctx.verify_mode=ssl.CERT_REQUIRED
_ctx.load_verify_locations(cafile='cloud-root.der')
_sock=socket.socket();_sock.settimeout(15)
try:
 _sock.connect(socket.getaddrinfo(_host,443,socket.AF_INET,socket.SOCK_STREAM)[0][-1])
 _tls=_ctx.wrap_socket(_sock,server_hostname=_host)
 try:
  _tls.write(('GET /auth/v1/health HTTP/1.1\\r\\nHost: '+_host+'\\r\\nConnection: close\\r\\n\\r\\n').encode())
  _status=_tls.readline().decode().strip()
  print('Certificate-verified HTTPS:',_status)
  if not _status.startswith('HTTP/1.1 '):raise RuntimeError('Missing HTTP response')
 finally:_tls.close()
finally:
 _sock.close();_b.active(False)
print('PASS: Wi-Fi and certificate/hostname-verified cloud TLS with BLE enabled')
"""
        )
        output, error = transport.exec_raw(code, timeout=35)
        print(output.decode().strip())
        if error:
            raise SystemExit(
                "Pico verified-TLS test failed; certificate validation was not disabled"
            )
    finally:
        try:
            transport.exit_raw_repl()
        finally:
            transport.close()


if __name__ == "__main__":
    main()
