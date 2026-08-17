#!/bin/bash
# Starts the planter overlay at login, via a LaunchAgent.
#
#   ./login-item.sh install     write the plist and start it now
#   ./login-item.sh uninstall   stop it and remove the plist
#   ./login-item.sh status      what launchd thinks
set -euo pipefail

label="local.claude-planter"
plist="$HOME/Library/LaunchAgents/$label.plist"
binary="$HOME/.local/bin/planter"
log="$HOME/Library/Logs/claude-planter.log"
domain="gui/$(id -u)"

case "${1:-install}" in
install)
    if [[ ! -x "$binary" ]]; then
        echo "no binary at $binary — run ./install.sh first" >&2
        exit 1
    fi

    mkdir -p "$(dirname "$plist")" "$(dirname "$log")"
    cat >"$plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>$label</string>
    <key>ProgramArguments</key>
    <array>
        <string>$binary</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <!-- Bring it back if it crashes, but let Quit from the right-click menu
         stick: a clean exit is not restarted. -->
    <key>KeepAlive</key>
    <dict>
        <key>SuccessfulExit</key>
        <false/>
    </dict>
    <key>ThrottleInterval</key>
    <integer>10</integer>
    <key>StandardOutPath</key>
    <string>$log</string>
    <key>StandardErrorPath</key>
    <string>$log</string>
</dict>
</plist>
PLIST

    # Any overlay started by hand would otherwise sit behind the managed one.
    pkill -f "$binary" 2>/dev/null || true
    launchctl bootout "$domain/$label" 2>/dev/null || true
    launchctl bootstrap "$domain" "$plist"
    echo "installed $plist"
    echo "logs: $log"
    ;;

uninstall)
    launchctl bootout "$domain/$label" 2>/dev/null || true
    rm -f "$plist"
    pkill -f "$binary" 2>/dev/null || true
    echo "removed $plist"
    ;;

status)
    launchctl print "$domain/$label" 2>/dev/null | grep -E '^\s*(state|pid|program|last exit) ' ||
        echo "not loaded"
    ;;

*)
    echo "usage: $0 [install|uninstall|status]" >&2
    exit 1
    ;;
esac
