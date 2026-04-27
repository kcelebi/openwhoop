# OpenWhoop

OpenWhoop is a project that allows you to download and analyze health data directly from your Whoop 4.0 device without a Whoop subscription or Whoop's servers, making the data your own.

Features include sleep detection, exercise detection, stress calculation, HRV analysis, SpO2, skin temperature, and strain scoring — all computed locally from raw sensor data.

For threat model, firmware extraction limits, and operational practices, see [SECURITY.md](SECURITY.md).

## Installation

### From npm (recommended)

```sh
npm install -g openwhoop
```

The npm package downloads the correct binary for your platform automatically.

### From source

```sh
cargo build --release --package openwhoop
# Binary at target/release/openwhoop
```

## Getting Started

**Discover your device (no database file is created):**

```sh
cargo run -r -- scan
```

Bluetooth permission is required on macOS the first time the binary runs.

When you run commands that sync or analyze data, copy `.env.example` to `.env` and set `DATABASE_URL` (and `WHOOP` after you know your device id):

```sh
cp .env.example .env
```

After you find your device:

- On Linux, copy its address to `.env` under `WHOOP`
- On macOS, copy its name to `.env` under `WHOOP`

Then download data from your Whoop:
```sh
cargo run -r -- download-history
```

### Live dashboard (local only)

While the strap is connected over BLE, OpenWhoop can push parsed samples (heart rate and sync metadata) as JSON over a WebSocket bound to **127.0.0.1** only.

1. Terminal A — from the repo root (same `DATABASE_URL` / `WHOOP` as `download-history`):

   ```sh
   cargo run -r -- live-server
   ```

   Optional: `--port 3848` (default). The **Studio** server listens on `http://127.0.0.1:<port>/` with **WebSocket** `ws://127.0.0.1:<port>/ws` and **REST** under `/api/*` (insights from your DB, compute passes, and strap commands that share the same BLE session). Example: `GET /api/insights/sleep`, `POST /api/compute/stress`, `POST /api/device/battery`, `POST /api/device/alarm` with body `{"unix": <seconds>}`.

2. Terminal B — React UI (proxies `/ws` to that port):

   ```sh
   cd web/live-dashboard && npm install && npm run dev
   ```

   Open the URL Vite prints (usually `http://127.0.0.1:5173/`). If the browser log shows a quick WebSocket error then reconnects, restart `npm run dev` after pulling the latest dashboard (Vite proxy + no React StrictMode double-mount). After history sync finishes, `live-server` **keeps the WebSocket running** until you press **Ctrl+C** in that terminal.

   Dashboard e2e (mock WebSocket, no Whoop): from `web/live-dashboard`, run `npx playwright install chromium` once, then `npm run test:e2e` (uses port **5199** so it does not clash with a manual dev server on 5173).

**What “live” means here:** samples are emitted from the same **history / high-frequency sync** path OpenWhoop already uses when downloading data, not from a separate phone-style realtime mode. You will see BPM and related fields when the device is streaming those packets during the session.

**Using the Whoop without a phone:** the strap typically **buffers** readings on board when it is not connected to an app or tool; exact retention is firmware-dependent and not specified in this project. You do **not** need to stay connected 24/7 for the device to record. To **copy** data into your database, run `download-history` (or the official app) **periodically**; running `live-server` is only for watching a stream **while** a BLE session is active.

### BLE troubleshooting: “Peer removed pairing information”

If OpenWhoop fails to connect after you used the **official Whoop app** (or another computer), macOS may report **Peer removed pairing information**. The strap and your Mac then disagree about BLE bonding keys.

**What works in practice:** **Forget** the Whoop under **System Settings → Bluetooth** on the Mac, and **remove / unpair** it from the **official app** as well, then connect again with OpenWhoop. You can re-pair the official app afterward; switching back and forth may require repeating this.

### Deploying the Studio UI (e.g. GitHub Pages + separate backend)

**Reality check:** `live-server` talks to the strap over **Bluetooth** from the machine where it runs. A random cloud VM cannot replace that unless you run the BLE stack on something **local** (your laptop, a home PC, Raspberry Pi) and only expose HTTP(S) and **WSS** to the internet. GitHub Pages is **static files only**—it can host the React build, not the WebSocket/API server.

**Rough shape:**

1. **Backend (your “other service”):** Run `cargo run -r -- live-server --whoop …` on a host that has the strap in range and paired. Terminate TLS at a reverse proxy (Caddy, nginx, Traefik) or a tunnel (Cloudflare Tunnel, ngrok) so browsers get `https://` and `wss://` (required if the UI is served over HTTPS, e.g. `*.github.io`).
2. **CORS:** Set `OPENWHOOP_CORS_ORIGIN` to your Pages origin (comma-separated for several), e.g. `https://yourname.github.io`, so `fetch` to `/api/*` from the static site is allowed. See `.env.example`.
3. **Bind address:** Default Studio bind is **127.0.0.1**. To listen on all interfaces (e.g. behind a proxy on the same machine), set `OPENWHOOP_STUDIO_BIND=0.0.0.0`. **Do not** expose Studio to the public internet without authentication and TLS; it controls the strap and serves health data.
4. **Frontend build:** From `web/live-dashboard`, point the UI at your public Studio URL when building:

   ```sh
   VITE_STUDIO_ORIGIN=https://studio.example.com npm run build
   ```

   Deploy the `dist/` output to GitHub Pages (branch/folder or Actions). For a **project** site at `https://user.github.io/repo/`, set Vite `base: '/repo/'` in `vite.config.ts` before building.

Local dev is unchanged: omit `VITE_STUDIO_ORIGIN` and use `npm run dev` so Vite proxies `/api` and `/ws` to `127.0.0.1:3848`.

## Commands

| Command | Description |
|---------|-------------|
| `scan` | Scan for available Whoop devices |
| `download-history` | Download historical data from the device |
| `live-server` | BLE sync + local **Studio** server: WebSocket live stream + `/api/*` REST for insights, compute, and strap control (see [Live dashboard](#live-dashboard-local-only)) |
| `detect-events` | Detect sleep and exercise events from raw data |
| `sleep-stats` | Print sleep statistics (all-time and last 7 days) |
| `exercise-stats` | Print exercise statistics (all-time and last 7 days) |
| `calculate-stress` | Calculate stress scores (Baevsky stress index) |
| `calculate-spo2` | Calculate blood oxygen from raw sensor data |
| `calculate-skin-temp` | Calculate skin temperature from raw sensor data |
| `set-alarm <time>` | Set device alarm (see [Alarm Formats](#alarm-formats)) |
| `sync` | Sync data between local and remote databases |
| `merge <database_url>` | Copy packets from another database into the current one |
| `rerun` | Reprocess stored packets (useful after adding new packet handlers) |
| `enable-imu` | Enable IMU (accelerometer/gyroscope) data collection |
| `download-firmware` | Download firmware from WHOOP API |
| `version` | Get device firmware version |
| `restart` | Restart device |
| `erase` | Erase all history data from device |
| `completions <shell>` | Generate shell completions (bash, zsh, fish) |
| **Remote Commands** | |
| `agent buzzer` | Trigger buzzer via HTTP API |
| `agent battery` | Get battery level via HTTP API |
| `agent set-alarm` | Set alarm via HTTP API |
| `agent get-alarm` | Get current alarm setting |
| `agent create-alarm` | Create scheduled alarm (cron or one-time) |
| `agent list-alarms` | List all scheduled alarms |
| `scheduler` | Run scheduler daemon (checks cron, queues commands) |
| `queue list` | List pending commands in queue |
| `queue push` | Push command to queue |
| `queue process` | Process and send pending commands to live-server |

### Alarm Formats

The `set-alarm` command accepts several time formats:

- **Datetime**: `2025-01-15 07:00:00` or `2025-01-15T07:00:00`
- **Time of day**: `07:00:00`
- **Relative offsets**: `1min`, `5min`, `10min`, `15min`, `30min`, `hour`

## Configuration

Configuration is done through environment variables or a `.env` file.

| Variable | Description | Required |
|----------|-------------|----------|
| `DATABASE_URL` | Database connection string (SQLite or PostgreSQL) | No for `scan` / `completions`; yes for commands that read or write the DB |
| `WHOOP` | Device identifier (MAC address on Linux, name on macOS) | For device commands |
| `REMOTE` | Remote database URL for `sync` command | For sync |
| `BLE_INTERFACE` | BLE adapter to use, e.g. `"hci1 (usb:Something)"` (Linux only) | No |
| `DEBUG_PACKETS` | Set to `true` to store raw packets in database | No |
| `RUST_LOG` | Logging level (default: `info`) | No |
| `OPENWHOOP_STUDIO_BIND` | IP for Studio HTTP bind (default `127.0.0.1`; e.g. `0.0.0.0` behind a proxy) | No |
| `OPENWHOOP_CORS_ORIGIN` | Comma-separated origins for Studio CORS (e.g. GitHub Pages URL) | No |
| `WHOOP_EMAIL` | WHOOP account email for `download-firmware` | For firmware |
| `WHOOP_PASSWORD` | WHOOP account password for `download-firmware` | For firmware |

### Database URLs

SQLite:
```
DATABASE_URL=sqlite://db.sqlite?mode=rwc
```

PostgreSQL:
```
DATABASE_URL=postgresql://user:password@localhost:5432/openwhoop
```

**Local Postgres (optional):**

- **Docker** (start Docker Desktop first): `docker compose up -d` in the repo root, then use `postgresql://openwhoop:openwhoop@127.0.0.1:5432/openwhoop`.
- **Homebrew** (example): `initdb -D .pgdata -U openwhoop --auth-local=trust --auth-host=trust`, then `pg_ctl -D .pgdata -o "-p 5432" -l .pgdata/logfile start`, `createdb -U openwhoop openwhoop`, and `DATABASE_URL=postgresql://openwhoop@127.0.0.1:5432/openwhoop`. Add `.pgdata/` to your own ignore rules if needed (it is gitignored here).

## Python environment (notebooks)

The CLI and database pipeline are **Rust** (`cargo`). The `notebooks/` analysis uses **Python**; dependencies are listed in `requirements.txt`.

```sh
python3 -m venv .venv
source .venv/bin/activate   # Windows: .venv\Scripts\activate
pip install --upgrade pip
pip install -r requirements.txt
```

Run Jupyter from the repo root (after `source .venv/bin/activate`):

```sh
jupyter lab notebooks/
```

Optional: register this venv as a Jupyter kernel for the IDE:

```sh
python -m ipykernel install --user --name=openwhoop --display-name="Python (openwhoop)"
```

## Importing Data to Python

```py
import pandas as pd
import os

# Heart rate data
QUERY = "SELECT time, bpm FROM heart_rate"

# Other available tables:
# "SELECT * FROM sleep_cycles"
# "SELECT * FROM activities"

PREFIX = "sqlite:///"  # Use "sqlite:///../" if working from notebooks/
DATABASE_URL = os.getenv("DATABASE_URL").replace("sqlite://", PREFIX)
df = pd.read_sql(QUERY, DATABASE_URL)
```

## Protocol

For the full reverse engineering writeup, see [Reverse Engineering Whoop 4.0 for fun and FREEDOM](https://github.com/bWanShiTong/reverse-engineering-whoop-post).

### BLE Service

The device communicates over a custom BLE service (`61080001-8d6d-82b8-614a-1c8cb0f8dcc6`) with the following characteristics:

| UUID       | Name              | Direction    | Description |
|------------|-------------------|--------------|-------------|
| 0x61080002 | CMD_TO_STRAP      | Write        | Send commands to the device |
| 0x61080003 | CMD_FROM_STRAP    | Notify       | Device command responses |
| 0x61080004 | EVENTS_FROM_STRAP | Notify       | Event notifications |
| 0x61080005 | DATA_FROM_STRAP   | Notify       | Sensor and history data |
| 0x61080007 | MEMFAULT          | Notify       | Memory fault logs |

### Packet Structure

All packets follow the same general structure:

| Field | Size | Description |
|-------|------|-------------|
| SOF | 1 byte | Start of frame (`0xAA`) |
| Length | 2 bytes | Payload length (little-endian) |
| Header | 2 bytes | Packet type identifier |
| Payload | variable | Command or data payload |
| CRC-32 | 4 bytes | Checksum |

### CRC

Packets use a CRC-32 with custom parameters:
- Polynomial: `0x4C11DB7`
- Reflect input/output: `true`
- Initial value: `0x0`
- XOR output: `0xF43F44AC`

### Command Categories

Commands sent to `CMD_TO_STRAP` use a category byte:

| Category | Purpose |
|----------|---------|
| `0x03` | Start/end activity and recording |
| `0x0e` | Enable/disable broadcast heart rate |
| `0x16` | Trigger data retrieval |
| `0x19` | Erase device |
| `0x1d` | Reboot device |
| `0x23` | Sync/history requests |
| `0x42` | Set alarm time |
| `0x4c` | Get device name |

### History Data

Each historical reading (96 bytes) contains:

| Field | Description |
|-------|-------------|
| Heart rate | BPM (beats per minute) |
| RR intervals | Beat-to-beat timing in milliseconds |
| Activity | Classification (active, sleep, inactive, awake) |
| PPG green/red/IR | Photoplethysmography sensor values |
| SpO2 red/IR | Blood oxygen sensor values |
| Skin temperature | Thermistor ADC reading |
| Accelerometer | 3-axis gravity vector |
| Respiratory rate | Derived respiratory rate |

The remaining sensor fields in each packet (which the original blog post marked as unknown) have since been fully decoded and are used to compute SpO2, skin temperature, and stress metrics.

## TODO

- [x] Sleep detection and activity detection
- [x] SpO2 readings
- [x] Temperature readings
- [x] Stress calculation (Baevsky stress index)
- [x] HRV analysis (RMSSD)
- [x] Strain scoring (Edwards TRIMP)
- [x] Database sync between SQLite and PostgreSQL
- [ ] Mobile/Desktop app
- [ ] Testout Whoop 5.0

## Remote Usage with Tailscale

OpenWhoop can be used remotely over a Tailscale network, enabling you to control your Whoop from a different machine (e.g., an AWS instance) while the device stays connected to your laptop.

### Architecture

```
┌─────────────────────────────────────────────────────────────────────┐
│                         AWS (Lightsail)                             │
│  ┌─────────────┐    ┌──────────────┐    ┌───────────────────────┐  │
│  │  Database   │◄───│   Scheduler  │───►│  Command Queue        │  │
│  │  (Postgres) │    │  (cron logic)│    │  (offline resilience) │  │
│  └─────────────┘    └──────────────┘    └───────────────────────┘  │
│         │                   │                      │                │
│         │    openwhoop scheduler/queue/agent     │                │
└─────────│─────────────────────────────────────────│────────────────┘
          │                   │                      │
          │    Tailscale Network                     │
          │                   │                      │
          ▼                   ▼                      ▼
┌─────────────────────────────────────────────────────────────────────┐
│                         Laptop                                       │
│  ┌────────────────────────────────────────────────────────────────┐ │
│  │  openwhoop live-server --whoop DEVICE_ID                      │ │
│  │  OPENWHOOP_STUDIO_BIND=0.0.0.0                                │ │
│  └────────────────────────────────────────────────────────────────┘ │
│         │                                                           │
│    Bluetooth                                                        │
│         │                                                           │
│    👑 Whoop                                                         │
└─────────────────────────────────────────────────────────────────────┘
```

### Setup

**1. On your laptop (with Whoop connected):**

```sh
# Install openwhoop (or use cargo)
OPENWHOOP_STUDIO_BIND=0.0.0.0 openwhoop live-server --whoop YOUR_DEVICE_ID
```

The `OPENWHOOP_STUDIO_BIND=0.0.0.0` is required for the server to accept connections from Tailscale IPs.

**2. On AWS (or any machine on your Tailscale network):**

```sh
# Set environment
export DATABASE_URL=postgres://user:pass@your-aws-host:5432/openwhoop
export OPENWHOOP_STUDIO_URL=http://100.x.y.z:3848  # Your laptop's Tailscale IP

# Trigger buzzer
openwhoop agent buzzer

# Set alarm
openwhoop agent set-alarm --alarm-time "7:00"

# Create cron alarm (runs daily at 7am)
openwhoop agent create-alarm --label "wake" --kind "cron" --schedule "0 7 * * *"

# Run scheduler (keeps cron alarms in sync)
openwhoop scheduler --device-id YOUR_DEVICE_ID --studio-url http://100.x.y.z:3848
```

### Queue Commands (Offline Resilience)

When the laptop is offline, commands are queued and automatically sent when reconnection happens:

```sh
# Queue a command manually
openwhoop queue push --device-id XYZ --command buzzer

# List pending commands
openwhoop queue list --device-id XYZ

# Process queue (sends pending commands to live-server)
openwhoop queue process --device-id XYZ --studio-url http://100.x.y.z:3848
```

### Finding Your Tailscale IP

```sh
tailscale ip -4  # Or check Tailscale admin console
```
