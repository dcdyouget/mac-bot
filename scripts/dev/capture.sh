#!/bin/bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
STAGE="${1:-S0}"
case "$STAGE" in S[0-5]) ;; *) echo 'Usage: capture.sh S0|S1|S2|S3|S4|S5' >&2; exit 2 ;; esac
SDK="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-$HOME/Library/Android/sdk}}"
ADB="$SDK/platform-tools/adb"
STAMP="$(date '+%Y%m%d-%H%M%S')"
OUT="$ROOT/docs/progress/$STAGE"
mkdir -p "$OUT"
FAILED=0
# Prefer the app window so unrelated applications are not captured.
WINDOW="$(/usr/bin/swift -e 'import CoreGraphics
let windows = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID) as? [[String: Any]] ?? []
for w in windows {
  let name = w[kCGWindowOwnerName as String] as? String ?? ""
  if ["MacBot", "macbot-desktop"].contains(name), (w[kCGWindowLayer as String] as? Int) == 0 {
    print(w[kCGWindowNumber as String] as? Int ?? 0); break
  }
}' 2>/dev/null || true)"
if [[ "$WINDOW" =~ ^[0-9]+$ ]]; then
  screencapture -x -l "$WINDOW" "$OUT/$STAMP-desktop.png" || FAILED=1
else
  echo '[capture] No visible desktop client window; desktop evidence missing.' >&2
  FAILED=1
fi
SERIAL=""
if [[ -x "$ADB" ]]; then
  while read -r device state rest; do
    [[ "$device" == emulator-* && "$state" == device ]] || continue
    AVD="$("$ADB" -s "$device" emu avd name 2>/dev/null | tr -d '\r' | head -1)"
    if [[ "$AVD" == macbot_api36 ]]; then SERIAL="$device"; break; fi
  done < <("$ADB" devices)
fi
if [[ -n "$SERIAL" ]]; then
  TEMP="$OUT/$STAMP-android.png.tmp"
  if "$ADB" -s "$SERIAL" exec-out screencap -p > "$TEMP" && [[ -s "$TEMP" ]]; then
    mv "$TEMP" "$OUT/$STAMP-android.png"
  else rm -f "$TEMP"; FAILED=1; fi
else
  echo '[capture] macbot_api36 is unavailable; Android screenshot missing.' >&2
  FAILED=1
fi
printf '[capture] Evidence directory: %s\n' "$OUT"
exit "$FAILED"
