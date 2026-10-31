# Pico W — OpenWhoop BLE Relay (Phase 1, T1 spike)

Proposed Raspberry Pi Pico W (RP2040 + CYW43439) relay. Current code only initializes CYW43
and an LED; it has no BLE central, Whoop protocol, Wi-Fi connection, or HTTP forwarding.
The Mac relay is the active implementation until hardware, cable, and firmware acceptance.

## T1 spike plan

1. Install toolchain (macOS host):
   - `brew install cmake picotool libusb`
   - SDK: clone https://github.com/raspberrypi/pico-sdk into `~/pico-sdk` and export `PICO_SDK_PATH`.
   - **Toolchain note (working set, verified 2026-10-30):** use the xPack GNU Arm Embedded GCC
     `arm-none-eabi-gcc` 12.2.1 (e.g. extracted from
     `xpack-arm-none-eabi-gcc-12.2.1-1.2-darwin-arm64.tar.gz`) and point
     `PICO_TOOLCHAIN_PATH` at it. The Arm official GCC 15.3 toolchain currently ships a newlib
     build that the GNU ld in that package cannot process ("Unknown destination type (ARM/Thumb)").
2. Bring-up: build & flash the stock Wi-Fi/blink example; verify USB serial output.
3. BLE central: use `pico_cyw43_arch` BLE API to connect to Whoop service
   `61080001-8d6d-82b8-614a-1c8cb0f8dcc6` and subscribe to notifies
   (CMD_FROM_STRAP, EVENTS_FROM_STRAP, DATA_FROM_STRAP, MEMFAULT).
4. Forward: on `HistoryReading` data → JSON → POST to backend ingest webhook (shared secret).
5. Spike deliverable: Pico W receives HR data → backend writes to Supabase → read path returns BPM.

See `docs/phase1-scoping.md` for the full Phase 1 context and milestone breakdown.

## Build / flash

```sh
cmake -B build -S . -DCMAKE_BUILD_TYPE=Release \
  -DPICO_SDK_PATH=/Users/kayacelebi/pico-sdk \
  -DPICO_TOOLCHAIN_PATH=/Users/kayacelebi/xpack-arm/xpack-arm-none-eabi-gcc-12.2.1-1.2 \
  -DPICO_BOARD=pico_w
cmake --build build
picotool uf2 convert build/pico_w.elf /tmp/pico_w.uf2
picotool load -f /tmp/pico_w.uf2
```

## Status

- [x] T1.1 Toolchain instructions and initial build spike
- [ ] T1.2 Radio bring-up (flash + verify)
- [ ] T1.3 BLE central spike (connect to Whoop)
- [ ] T1.4 Forward to backend (webhook)
- [ ] T1.5 Spike deliverable

## T1.3/T1.4 implementation design (spike draft)

- BLE central: use C SDK `pico_cyw43_arch` BLE surface toward Whoop service
  `61080001-8d6d-82b8-614a-1c8cb0f8dcc6`; subscribe to CMD_FROM_STRAP / EVENTS_FROM_STRAP /
  DATA_FROM_STRAP / MEMFAULT notifies.
- On `HistoryReading` data → JSON payload → POST to backend ingest webhook (shared secret).

## Acceptance

- **A1:** toolchain + radio + BLE central + forward + ingest → BPM returns. Not accepted: the
  current firmware is a bring-up stub, and the correct cable is still needed.
