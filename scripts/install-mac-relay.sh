#!/usr/bin/env bash
set -euo pipefail
repo_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
label="com.openwhoop.relay"
plist="$HOME/Library/LaunchAgents/$label.plist"
log_dir="$HOME/Library/Logs/OpenWhoop"
bootstrap_agent() {
  for attempt in 1 2 3 4 5; do
    if launchctl bootstrap "gui/$(id -u)" "$1"; then return 0; fi
    sleep 1
  done
  return 1
}
launchctl bootout "gui/$(id -u)/com.openwhoop.watchdog" 2>/dev/null || true
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
<key>ProcessType</key><string>Interactive</string>
<key>ExitTimeOut</key><integer>5</integer>
<key>StandardOutPath</key><string>$log_dir/relay.log</string>
<key>StandardErrorPath</key><string>$log_dir/relay.log</string>
</dict></plist>
EOF
chmod 600 "$plist"
launchctl bootout "gui/$(id -u)/$label" 2>/dev/null || true
bootstrap_agent "$plist"
launchctl kickstart -k "gui/$(id -u)/$label"
watchdog_label="com.openwhoop.watchdog"
watchdog_plist="$HOME/Library/LaunchAgents/$watchdog_label.plist"
cat > "$watchdog_plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>Label</key><string>$watchdog_label</string>
<key>ProgramArguments</key><array><string>/usr/bin/python3</string><string>$repo_dir/scripts/relay-watchdog.py</string><string>$HOME/.local/share/openwhoop/relay.sqlite</string><string>$HOME/.local/share/openwhoop/watchdog.json</string></array>
<key>RunAtLoad</key><true/>
<key>StartInterval</key><integer>30</integer>
<key>StandardOutPath</key><string>$log_dir/relay.log</string>
<key>StandardErrorPath</key><string>$log_dir/relay.log</string>
</dict></plist>
EOF
chmod 600 "$watchdog_plist"
launchctl bootout "gui/$(id -u)/$watchdog_label" 2>/dev/null || true
bootstrap_agent "$watchdog_plist"
analysis_label="com.openwhoop.analysis"
analysis_plist="$HOME/Library/LaunchAgents/$analysis_label.plist"
cat > "$analysis_plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>Label</key><string>$analysis_label</string>
<key>ProgramArguments</key><array><string>$repo_dir/scripts/run-analysis.sh</string></array>
<key>RunAtLoad</key><true/>
<key>StartInterval</key><integer>60</integer>
<key>StandardOutPath</key><string>$log_dir/analysis.log</string>
<key>StandardErrorPath</key><string>$log_dir/analysis.log</string>
</dict></plist>
EOF
chmod 600 "$analysis_plist"
launchctl bootout "gui/$(id -u)/$analysis_label" 2>/dev/null || true
bootstrap_agent "$analysis_plist"
echo "Installed $label; logs: $log_dir/relay.log"
