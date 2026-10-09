#!/bin/zsh
set -euo pipefail

SCRIPT_DIR="${0:A:h}"
MAC_ROOT="${SCRIPT_DIR:h}"
CONFIG="${1:-debug}"
HOST="${MACBOT_HOST:-}"
PASSWORD="${MACBOT_PASSWORD:-}"

[[ "$CONFIG" == "debug" || "$CONFIG" == "release" ]] || {
  print -u2 "Usage: $0 [debug|release]"
  exit 2
}

if [[ "$CONFIG" == "release" ]]; then
  PROFILE_DIR="release"
else
  PROFILE_DIR="debug"
fi

APP_BINARY="$MAC_ROOT/dist/MacBot.app/Contents/MacOS/macbot-desktop"
TARGET_BINARY="$MAC_ROOT/target/$PROFILE_DIR/macbot-desktop"

if [[ -x "$APP_BINARY" ]]; then
  BINARY="$APP_BINARY"
elif [[ -x "$TARGET_BINARY" ]]; then
  BINARY="$TARGET_BINARY"
else
  print -u2 "No $CONFIG desktop binary found. Run packaging/package.sh $CONFIG app first."
  exit 1
fi

if [[ -n "$HOST" ]]; then
  print "Starting Mac Bot with host $HOST"
  exec env MACBOT_HOST="$HOST" MACBOT_PASSWORD="$PASSWORD" "$BINARY" "${@:2}"
fi
print "Starting Mac Bot with connection screen"
exec "$BINARY" "${@:2}"
