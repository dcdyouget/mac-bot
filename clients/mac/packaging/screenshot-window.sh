#!/bin/zsh
set -euo pipefail

SCRIPT_DIR="${0:A:h}"
MAC_ROOT="${SCRIPT_DIR:h}"
OUTPUT="${1:-$MAC_ROOT/progress/S5/macbot-window.png}"
OWNER="${2:-Mac Bot}"
TMP_ROOT="${TMPDIR:-/tmp}/macbot-window-id"
HELPER="$TMP_ROOT/window-id"

mkdir -p "$TMP_ROOT" "${OUTPUT:h}"
if [[ ! -x "$HELPER" || "$SCRIPT_DIR/window-id.swift" -nt "$HELPER" ]]; then
  swiftc "$SCRIPT_DIR/window-id.swift" -o "$HELPER"
fi

WINDOW_ID="$($HELPER "$OWNER" 2>/dev/null || true)"
if [[ ! "$WINDOW_ID" =~ '^[0-9]+$' ]]; then
  print -u2 "No visible window found for owner '$OWNER'. Start MacBot.app first."
  exit 1
fi

CAPTURE_TIMEOUT="${MACBOT_SCREENSHOT_TIMEOUT:-15}"
screencapture -x -l "$WINDOW_ID" "$OUTPUT" &
CAPTURE_PID=$!
for ((attempt = 0; attempt < CAPTURE_TIMEOUT * 2; attempt++)); do
  kill -0 "$CAPTURE_PID" 2>/dev/null || break
  sleep 0.5
done
if kill -0 "$CAPTURE_PID" 2>/dev/null; then
  kill "$CAPTURE_PID" 2>/dev/null || true
  wait "$CAPTURE_PID" 2>/dev/null || true
  print -u2 "screencapture did not finish within ${CAPTURE_TIMEOUT}s (check Screen Recording permission)."
  exit 1
fi
wait "$CAPTURE_PID"
print "Captured window $WINDOW_ID to $OUTPUT"
