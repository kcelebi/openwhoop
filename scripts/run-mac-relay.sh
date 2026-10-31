#!/usr/bin/env bash
set -euo pipefail
repo_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
set -a
source "$repo_dir/.env.cloud"
set +a
data_dir="$HOME/.local/share/openwhoop"
mkdir -p "$data_dir"
export DATABASE_URL="sqlite://$data_dir/relay.sqlite?mode=rwc"
export OPENWHOOP_STUDIO_BIND=127.0.0.1
export OPENWHOOP_HISTORY_SYNC=false
export OPENWHOOP_RICH_CAPTURE=true
export RUST_LOG=warn,openwhoop=info,btleplug::corebluetooth::adapter=debug
exec /usr/bin/caffeinate -i "$repo_dir/target/debug/openwhoop" live-server --whoop 4C0639073 --port 3848
