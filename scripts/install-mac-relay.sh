#!/usr/bin/env bash
set -euo pipefail
repo_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
label="com.openwhoop.relay"
plist="$HOME/Library/LaunchAgents/$label.plist"
log_dir="$HOME/Library/Logs/OpenWhoop"
mkdir -p "$(dirname "$plist")" "$log_dir"
test -f "$repo_dir/.env.cloud"
test -x "$repo_dir/target/debug/openwhoop"
cat > "$plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>Label</key><string>$label</string>
<key>ProgramArguments</key><array><string>$repo_dir/scripts/run-mac-relay.sh</string></array>
<key>WorkingDirectory</key><string>$repo_dir</string>
<key>RunAtLoad</key><true/>
<key>KeepAlive</key><true/>
<key>StandardOutPath</key><string>$log_dir/relay.log</string>
<key>StandardErrorPath</key><string>$log_dir/relay.log</string>
</dict></plist>
EOF
chmod 600 "$plist"
launchctl bootout "gui/$(id -u)/$label" 2>/dev/null || true
launchctl bootstrap "gui/$(id -u)" "$plist"
launchctl kickstart -k "gui/$(id -u)/$label"
echo "Installed $label; logs: $log_dir/relay.log"
