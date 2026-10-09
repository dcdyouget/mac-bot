#!/bin/sh
set -eu
ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)"
HOME_DIR="${MACBOT_HOME:-$HOME/MacBot}"
BIN="${MACBOT_BIN:-$ROOT/target/release/macbotd}"
BIN_DIR="${MACBOT_BIN_DIR:-$HOME/.local/bin}"
BIN_DEST="$BIN_DIR/macbotd"
APP="$HOME/Applications/MacBot Server.app"
APP_BIN="$APP/Contents/MacOS/macbotd"
mkdir -p "$HOME_DIR/data" "$HOME/Library/LaunchAgents" "$BIN_DIR" "$APP/Contents/MacOS"
cargo build --release --manifest-path "$ROOT/Cargo.toml" -p macbotd
mkdir -p "$APP/Contents/Resources"
install -m 755 "$BIN" "$APP_BIN.new"
mv -f "$APP_BIN.new" "$APP_BIN"
"$ROOT/macbotd/packaging/prepare-sidecar.sh" "$APP/Contents/MacOS/agent-browser"
cp "$ROOT/macbotd/packaging/agent-browser.LICENSE" "$APP/Contents/Resources/agent-browser.LICENSE"
install -m 644 "$ROOT/macbotd/packaging/MacBot Server.app/Contents/Info.plist" \
  "$APP/Contents/Info.plist"
rm -f "$BIN_DEST" "$BIN_DIR/macbot"
ln -sfn "$APP_BIN" "$BIN_DEST"
ln -sfn macbotd "$BIN_DIR/macbot"
cp "$ROOT/macbotd/packaging/com.macbot.server.plist" "$APP/Contents/Resources/com.macbot.server.plist"
cp "$ROOT/macbotd/packaging/update-installed.sh" "$APP/Contents/Resources/update-installed.sh"
cp "$ROOT/macbotd/packaging/update-manifest.json" "$APP/Contents/Resources/update-manifest.json"
chmod 755 "$APP/Contents/Resources/update-installed.sh"
sed -e "s#__MACBOT_HOME__#$HOME_DIR#g" -e "s#__MACBOT_BIN__#$APP_BIN#g" "$ROOT/macbotd/packaging/com.macbot.server.plist" > "$HOME/Library/LaunchAgents/com.macbot.server.plist"
/usr/bin/plutil -insert EnvironmentVariables.MACBOT_UPDATE_SCRIPT -string \
  "$ROOT/macbotd/packaging/update.sh" "$HOME/Library/LaunchAgents/com.macbot.server.plist"
launchctl unload "$HOME/Library/LaunchAgents/com.macbot.server.plist" 2>/dev/null || true
launchctl load "$HOME/Library/LaunchAgents/com.macbot.server.plist"
echo "Mac Bot installed at $BIN_DEST; open http://localhost:7788/admin to set the password"
