#!/usr/bin/env bash
set -euo pipefail
repo_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
set -a
source "$repo_dir/.env.cloud"
set +a
data_dir="$HOME/.local/share/openwhoop"
exec /usr/bin/python3 "$repo_dir/scripts/analyze-telemetry.py" --database "$data_dir/relay.sqlite" --state "$data_dir/analysis-state.json" --legacy "$data_dir/relay.sqlite" --legacy "$repo_dir/w.db"
