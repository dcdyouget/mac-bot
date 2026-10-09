#!/bin/zsh
set -euo pipefail

SCRIPT_DIR="${0:A:h}"
MAC_ROOT="${SCRIPT_DIR:h}"
OUTPUT="${1:-$MAC_ROOT/progress/S5/macbot-window.png}"
OWNER="${2:-Mac Bot}"
WINDOW_PID="${3:-${MACBOT_WINDOW_PID:-}}"
TMP_ROOT="${TMPDIR:-/tmp}/macbot-window-id"
HELPER="$TMP_ROOT/window-id"
TMP_OUTPUT="${OUTPUT:h}/.development-mock.png"

mkdir -p "$TMP_ROOT" "${OUTPUT:h}"
rm -f "$TMP_OUTPUT"
cleanup() { rm -f "$TMP_OUTPUT"; }
trap cleanup EXIT INT TERM
if [[ ! -x "$HELPER" || "$SCRIPT_DIR/window-id.swift" -nt "$HELPER" ]]; then
  swiftc "$SCRIPT_DIR/window-id.swift" -o "$HELPER"
fi

if [[ -n "$WINDOW_PID" && ! "$WINDOW_PID" =~ '^[0-9]+$' ]]; then
  print -u2 "Window PID must be numeric: '$WINDOW_PID'"
  exit 2
fi

PERMISSION_STATUS="$($HELPER --permission 2>/dev/null || true)"
if [[ "$PERMISSION_STATUS" != "authorized" ]]; then
  print -u2 "Screen Recording permission is not granted; CGPreflightScreenCaptureAccess reports '$PERMISSION_STATUS'."
fi

WINDOW_ID="$($HELPER "$OWNER" "$WINDOW_PID" 2>/dev/null || true)"
if [[ ! "$WINDOW_ID" =~ '^[0-9]+$' ]]; then
  if [[ -n "$WINDOW_PID" ]]; then
    print -u2 "No visible window found for owner '$OWNER' with PID $WINDOW_PID. Start the matching MacBot.app first."
  else
    print -u2 "No visible window found for owner '$OWNER'. Start MacBot.app first or pass its PID as the third argument."
  fi
  exit 1
fi

CAPTURE_TIMEOUT="${MACBOT_SCREENSHOT_TIMEOUT:-15}"
validate_png() {
  [[ -s "$TMP_OUTPUT" ]] || return 1
  local signature trailer
  signature="$(od -An -tx1 -N8 "$TMP_OUTPUT" 2>/dev/null | tr -d '[:space:]')"
  trailer="$(tail -c 12 "$TMP_OUTPUT" 2>/dev/null | od -An -tx1 | tr -d '[:space:]')"
  [[ "$signature" == "89504e470d0a1a0a" ]] || return 1
  [[ "$trailer" == "0000000049454e44ae426082" ]] || return 1

  local sips_output="$TMP_ROOT/sips-${$}.txt"
  sips -g pixelWidth -g pixelHeight "$TMP_OUTPUT" >"$sips_output" 2>&1 &
  local sips_pid=$!
  for ((attempt = 0; attempt < 20; attempt++)); do
    kill -0 "$sips_pid" 2>/dev/null || break
    sleep 0.05
  done
  if kill -0 "$sips_pid" 2>/dev/null; then
    kill -TERM "$sips_pid" 2>/dev/null || true
    sleep 0.1
    kill -KILL "$sips_pid" 2>/dev/null || true
  fi
  wait "$sips_pid" 2>/dev/null || true
  local valid_size=1
  grep -Eq 'pixelWidth: [1-9][0-9]*' "$sips_output" || valid_size=0
  grep -Eq 'pixelHeight: [1-9][0-9]*' "$sips_output" || valid_size=0
  rm -f "$sips_output"
  ((valid_size))
}

screencapture -x -l "$WINDOW_ID" "$TMP_OUTPUT" &
CAPTURE_PID=$!
for ((attempt = 0; attempt < CAPTURE_TIMEOUT * 2; attempt++)); do
  kill -0 "$CAPTURE_PID" 2>/dev/null || break
  sleep 0.5
done
if kill -0 "$CAPTURE_PID" 2>/dev/null; then
  kill "$CAPTURE_PID" 2>/dev/null || true
  sleep 0.25
  kill -KILL "$CAPTURE_PID" 2>/dev/null || true
  wait "$CAPTURE_PID" 2>/dev/null || true
  if validate_png; then
    mv -f "$TMP_OUTPUT" "$OUTPUT"
    trap - EXIT INT TERM
    print "Captured window $WINDOW_ID to $OUTPUT (finalize_timeout_valid_png)"
    exit 0
  fi
  print -u2 "screencapture timed out without a complete PNG (permission or capture finalization issue)."
  exit 1
fi
if ! wait "$CAPTURE_PID"; then
  print -u2 "screencapture failed for window $WINDOW_ID."
  exit 1
fi
if ! validate_png; then
  print -u2 "screencapture exited without producing a valid PNG."
  exit 1
fi
mv -f "$TMP_OUTPUT" "$OUTPUT"
trap - EXIT INT TERM
print "Captured window $WINDOW_ID to $OUTPUT"
