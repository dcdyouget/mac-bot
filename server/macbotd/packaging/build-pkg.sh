#!/bin/sh
set -eu
ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)"
OUT="${1:-$ROOT/target/MacBot-Server.pkg}"
VERSION="${MACBOT_VERSION:-0.1.0}"
SOURCE_COMMIT="${MACBOT_SOURCE_COMMIT:-$(git -C "$ROOT" rev-parse HEAD 2>/dev/null || true)}"
case "$SOURCE_COMMIT" in
  *[!0-9a-fA-F]*|'') echo "set MACBOT_SOURCE_COMMIT to the full source SHA when building an archive" >&2; exit 1 ;;
esac
[ "${#SOURCE_COMMIT}" -eq 40 ] || { echo "source SHA must contain 40 hexadecimal characters" >&2; exit 1; }
SOURCE_DIRTY=false
if [ -n "$(git -C "$ROOT" status --porcelain --untracked-files=no 2>/dev/null || true)" ]; then
  SOURCE_DIRTY=true
fi
STAGE="$(mktemp -d "${TMPDIR:-/tmp}/macbot-pkg.XXXXXX")"
trap 'rm -rf "$STAGE"' EXIT INT TERM

cargo build --release --manifest-path "$ROOT/Cargo.toml" -p macbotd
APP="$STAGE/Applications/MacBot Server.app"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
install -m 755 "$ROOT/target/release/macbotd" "$APP/Contents/MacOS/macbotd"
"$ROOT/macbotd/packaging/prepare-sidecar.sh" "$APP/Contents/MacOS/agent-browser"
cp "$ROOT/macbotd/packaging/agent-browser.LICENSE" "$APP/Contents/Resources/agent-browser.LICENSE"
cp "$ROOT/macbotd/packaging/MacBot Server.app/Contents/Info.plist" "$APP/Contents/Info.plist"
cp "$ROOT/macbotd/packaging/com.macbot.server.plist" "$APP/Contents/Resources/com.macbot.server.plist"
cp "$ROOT/macbotd/packaging/update-installed.sh" "$APP/Contents/Resources/update-installed.sh"
chmod 755 "$APP/Contents/Resources/update-installed.sh"
printf '%s\n' "$SOURCE_COMMIT" > "$APP/Contents/Resources/source-commit.txt"
BUILD_INFO="$APP/Contents/Resources/build-info.json"
plutil -create xml1 "$BUILD_INFO"
plutil -insert source_commit -string "$SOURCE_COMMIT" "$BUILD_INFO"
plutil -insert source_dirty -bool "$SOURCE_DIRTY" "$BUILD_INFO"
plutil -insert version -string "$VERSION" "$BUILD_INFO"
plutil -insert macbotd_sha256 -string "$(shasum -a 256 "$APP/Contents/MacOS/macbotd" | awk '{print $1}')" "$BUILD_INFO"
plutil -insert agent_browser_sha256 -string "$(shasum -a 256 "$APP/Contents/MacOS/agent-browser" | awk '{print $1}')" "$BUILD_INFO"
plutil -convert json "$BUILD_INFO"
plutil -insert MacBotSourceCommit -string "$SOURCE_COMMIT" "$APP/Contents/Info.plist"
UPDATE_URL="${MACBOT_UPDATE_URL:-}"
UPDATE_SHA256="${MACBOT_UPDATE_SHA256:-}"
MANIFEST="$APP/Contents/Resources/update-manifest.json"
plutil -create xml1 "$MANIFEST"
plutil -insert version -string "$VERSION" "$MANIFEST"
plutil -insert url -string "$UPDATE_URL" "$MANIFEST"
plutil -insert sha256 -string "$UPDATE_SHA256" "$MANIFEST"
plutil -convert json "$MANIFEST"
plutil -replace CFBundleShortVersionString -string "$VERSION" "$APP/Contents/Info.plist"
plutil -replace CFBundleVersion -string "$VERSION" "$APP/Contents/Info.plist"
mkdir -p "$(dirname -- "$OUT")"
pkgbuild --root "$STAGE" \
  --scripts "$ROOT/macbotd/packaging/pkg-scripts" \
  --identifier com.macbot.server \
  --version "$VERSION" \
  --install-location / "$OUT"
echo "$OUT"
