# OpenWhoop Pico W bring-up

This folder contains **diagnostic firmware, not the deployed relay**. The Mac
continues to own the WHOOP connection, alarms, and cloud synchronization.

## Hardware evidence (2026-10-31)

- The replacement cable carries USB data: the board enumerates as `RP2 Boot`,
  mounts `RPI-RP2`, and responds to picotool.
- RP2040 B2, 2 MiB flash; Pico SDK 2.3.1 and xPack Arm GCC 12.2.1 build both targets.
- The radio image flashed and its flash contents verified. USB CDC enumerated,
  but runtime logging/radio initialization has **not** passed acceptance.
- A separate USB-only probe is built to isolate the USB runtime from CYW43/BTstack.
  If the running image does not respond to software reboot, hold BOOTSEL while
  reconnecting the board before flashing this probe.
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
picotool load -v -x firmware/pico-w/build/pico_usb_probe.uf2
# Read /dev/cu.usbmodem* at 115200; expect repeated USB probe heartbeats.
# Re-enter BOOTSEL, then:
picotool load -v -x firmware/pico-w/build/pico_w.uf2
```

`-f` can request software reboot from a responsive SDK USB-stdio image. If it
fails, use physical BOOTSEL. Do not run concurrent picotool/serial sessions while
recovering an unresponsive image. A successful flash is not runtime acceptance.

## Always-on relay work still required

1. Prove USB, Wi-Fi and BLE radio runtime, then a controlled WHOOP connection and
   standard HR/RR subscription. Use a deliberate ownership handoff from the Mac;
   restore the Mac if any acceptance check fails.
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
