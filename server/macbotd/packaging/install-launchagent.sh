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
install -m 755 "$BIN" "$BIN_DEST"
ln -sf macbotd "$BIN_DIR/macbot"
cp "$ROOT/macbotd/packaging/MacBot Server.app/Contents/Info.plist" "$APP/Contents/Info.plist"
cat > "$APP_BIN" <<EOF
#!/bin/sh
exec "$BIN_DEST" "\$@"
EOF
chmod 755 "$APP_BIN"
sed -e "s#__MACBOT_HOME__#$HOME_DIR#g" -e "s#__MACBOT_BIN__#$APP_BIN#g" "$ROOT/macbotd/packaging/com.macbot.server.plist" > "$HOME/Library/LaunchAgents/com.macbot.server.plist"
launchctl unload "$HOME/Library/LaunchAgents/com.macbot.server.plist" 2>/dev/null || true
launchctl load "$HOME/Library/LaunchAgents/com.macbot.server.plist"
echo "Mac Bot installed at $BIN_DEST; open http://localhost:7788/admin to set the password"
