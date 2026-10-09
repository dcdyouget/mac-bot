#!/bin/sh
set -eu
PLIST="$HOME/Library/LaunchAgents/com.macbot.server.plist"
launchctl unload "$PLIST" 2>/dev/null || true
rm -f "$PLIST"
rm -rf "$HOME/Applications/MacBot Server.app"
rm -f "${MACBOT_BIN_DIR:-$HOME/.local/bin}/macbotd" "${MACBOT_BIN_DIR:-$HOME/.local/bin}/macbot"
echo "Mac Bot LaunchAgent removed (data is preserved)"
