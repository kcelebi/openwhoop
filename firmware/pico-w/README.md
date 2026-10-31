# OpenWhoop Pico W bring-up

This folder contains **diagnostic firmware, not the deployed relay**. The Mac
continues to own the WHOOP connection, alarms, and cloud synchronization.

## Hardware evidence (2026-10-31)

- The replacement cable carries USB data: the board enumerates as `RP2 Boot`,
  mounts `RPI-RP2`, and responds to picotool.
- RP2040 B2, 2 MiB flash; Pico SDK 2.3.1 and xPack Arm GCC 12.2.1 build both targets.
- The USB-only C probe prints repeated heartbeats. Cable/USB runtime acceptance passes.
- The custom C radio image flashes/verifies, but its startup hangs; C radio runtime
  is **not** accepted.
- Official MicroPython 1.29.0 for Pico W boots and runs USB REPL normally. It
  activated Wi-Fi and found 20 networks, activated BLE, and completed a passive
  scan with 1,036 reports including 53 WHOOP advertisements. No Wi-Fi credentials
  or cloud secrets are installed on the board.
- Official firmware URL: https://micropython.org/resources/firmware/RPI_PICO_W-20260824-v1.29.0.uf2
  SHA-256: `f918c0a082c6daf53e27998b7f990ba5f065db2e298b21a34024532a20786794`.
- A controlled Mac/Pico handoff received six live BPM/RR notifications, verified
  the CCCD write response, durably saved all six raw and decoded readings to the
  local outbox, and restored the Mac. This is USB-hosted BLE acceptance, **not**
  Pico-to-cloud-over-Wi-Fi acceptance.
- A full flash backup was saved outside the repository before the first flash.

## Build

```sh
cmake -B firmware/pico-w/build -S firmware/pico-w \
  -DPICO_SDK_PATH=/Users/kayacelebi/pico-sdk \
  -DPICO_TOOLCHAIN_PATH=/Users/kayacelebi/xpack-arm/xpack-arm-none-eabi-gcc-12.2.1-1.2 \
  -DPICO_BOARD=pico_w
cmake --build firmware/pico-w/build -j 6
```

The SDK generates UF2s directly. USB stdio is enabled, UART stdio disabled, and
semihosting is not linked. `pico_usb_probe` prints a heartbeat once per second.
`pico_w` initializes CYW43, scans for Wi-Fi networks without joining, and scans
Bluetooth advertisements without connecting to or sending commands to the strap.
It reports aggregate scan counts and WHOOP names, without printing nearby SSIDs.

## Flash and inspect

Start with the USB-only image and verify serial output before testing radios:

```sh
picotool load -v firmware/pico-w/build/pico_usb_probe.uf2
picotool reboot
# Read /dev/cu.usbmodem* at 115200; expect repeated USB probe heartbeats.
# Re-enter BOOTSEL, then:
picotool load -v firmware/pico-w/build/pico_w.uf2
picotool reboot
```

`-f` can request software reboot from a responsive SDK USB-stdio image. If it
fails, use physical BOOTSEL. Do not run concurrent picotool/serial sessions while
recovering an unresponsive image. A successful flash is not runtime acceptance.

## MicroPython hardware probes

The board is currently on the official MicroPython firmware, with no auto-start
relay installed. These probes are run from the host, not at power-on:

```sh
uv run --with mpremote mpremote connect auto run firmware/pico-w/diagnostics/radio_probe.py
python3 scripts/accept-pico-hr.py --handoff
```

The second command requires explicit handoff, stops the Mac relay/watchdog for a
bounded test and restores both in `finally`. Received notifications are queued in
MicroPython before parsing and durably saved to the Mac telemetry outbox for
upload. It subscribes to standard HR/RR only and sends no buzzer or alarm commands.
The tested WHOOP HR service has exactly one trailing CCCD at `HR value + 1`; the
probe checks that layout before writing. Descriptor discovery on this firmware
reported the wrong completion event for the singleton range, so this is a
**device-specific diagnostic**, not a general production GATT client.

## Always-on relay work still required

1. USB, Wi-Fi scanning, BLE scanning and a controlled WHOOP connection are proven
   with MicroPython. Choose and harden the runtime (fix the C startup or use the
   MicroPython client), then validate Wi-Fi/BLE coexistence while actually joining
   the apartment network and uploading. Keep explicit ownership handoff/fallback.
2. Port framing/CRC, characteristic reassembly, raw telemetry preservation,
   buffered-history ACKs, reconnect/backoff and clock verification. Forward
   batches over verified TLS to a device-authenticated ingest endpoint. Do not
   put the Supabase service-role key on the Pico. Advance the strap trim cursor
   only after the corresponding readings are durably stored by the ingest path.
3. Provision apartment Wi-Fi, persist credentials/bond keys and cached alarms,
   and implement bounded buffering, flash-wear management, watchdog/power-cycle
   recovery, and device status/lease ownership. The Pico's 2 MiB flash is not an
   unlimited telemetry archive; use the strap's unacknowledged history as the
   principal retry source when the network is unavailable.
4. Match the Mac's alarm functionality: several schedules, dashboard toggles,
   immediate edits/resync, native alarm readback, timezone/DST-correct future UTC
   occurrences and advancing to the next alarm without a Mac. Move analysis
   execution from the Mac-only SQLite worker to a hosted worker so insights also
   continue with the Mac powered off.

Final acceptance must use wall power with the Mac relay stopped: live/rich upload,
leave-and-return backfill with actual interval coverage, two physical sequential
alarms, edits/toggles, Wi-Fi outage recovery and a cold power cycle. Do not label
the Pico the active relay until this entire path passes. The mobile layout stays
unchanged.
