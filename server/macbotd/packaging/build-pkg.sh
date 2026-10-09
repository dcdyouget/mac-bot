#!/bin/sh
set -eu
ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)"
OUT="${1:-$ROOT/target/MacBot-Server.pkg}"
VERSION="${MACBOT_VERSION:-0.1.0}"
STAGE="$(mktemp -d "${TMPDIR:-/tmp}/macbot-pkg.XXXXXX")"
trap 'rm -rf "$STAGE"' EXIT INT TERM

cargo build --release --manifest-path "$ROOT/Cargo.toml" -p macbotd
APP="$STAGE/Applications/MacBot Server.app"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
install -m 755 "$ROOT/target/release/macbotd" "$APP/Contents/MacOS/macbotd"
cp "$ROOT/macbotd/packaging/MacBot Server.app/Contents/Info.plist" "$APP/Contents/Info.plist"
cp "$ROOT/macbotd/packaging/com.macbot.server.plist" "$APP/Contents/Resources/com.macbot.server.plist"
mkdir -p "$(dirname -- "$OUT")"
pkgbuild --root "$STAGE" \
  --scripts "$ROOT/macbotd/packaging/pkg-scripts" \
  --identifier com.macbot.server \
  --version "$VERSION" \
  --install-location / "$OUT"
echo "$OUT"
