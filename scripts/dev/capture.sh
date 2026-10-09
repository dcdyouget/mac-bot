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
INSTALL_APP="$HOME/Applications/MacBot.app"
INSTALL_BINARY="$INSTALL_APP/Contents/MacOS/macbot-desktop"
INSTALL_SOURCE_COMMIT="$(cat "$INSTALL_APP/Contents/Resources/source-commit" 2>/dev/null | tr -d '\r\n' || true)"

# Resolve the installed executable first.  CGWindowOwnerPID is only accepted
# when it belongs to that exact executable, so a development worktree window
# cannot be selected by owner name alone.
DESKTOP_PIDS=()
if [[ -x "$INSTALL_BINARY" ]]; then
  while read -r pid comm; do
    [[ "$pid" =~ ^[0-9]+$ ]] || continue
    [[ "$comm" == "macbot-desktop" || "$comm" == */macbot-desktop || "$comm" == "$INSTALL_BINARY" ]] || continue
    command_line="$(/bin/ps -p "$pid" -o command= 2>/dev/null | sed -e 's/^[[:space:]]*//' || true)"
    case "$command_line" in
      "$INSTALL_BINARY"|"$INSTALL_BINARY "*) DESKTOP_PIDS+=("$pid") ;;
    esac
  done < <(/bin/ps -axo pid=,comm= 2>/dev/null || true)
fi

printf '[capture] Installed desktop: %s\n' "$INSTALL_APP"
printf '[capture] Installed source-commit: %s\n' "${INSTALL_SOURCE_COMMIT:-unknown}"
if ((${#DESKTOP_PIDS[@]} == 0)); then
  echo '[capture] Installed desktop executable is not running; desktop capture failed.' >&2
  FAILED=1
else
  MACBOT_CAPTURE_PIDS="${DESKTOP_PIDS[*]}" /usr/bin/swift -e 'import CoreGraphics
import Foundation
let wanted = Set((ProcessInfo.processInfo.environment["MACBOT_CAPTURE_PIDS"] ?? "").split(separator: " ").compactMap { Int32($0) })
let deadline = Date().addingTimeInterval(10)
var found = false
repeat {
  let windows = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID) as? [[String: Any]] ?? []
  for window in windows {
    let ownerPID = window[kCGWindowOwnerPID as String] as? Int ?? 0
    let layer = window[kCGWindowLayer as String] as? Int ?? 1
    let alpha = window[kCGWindowAlpha as String] as? Double ?? 1
    if wanted.contains(Int32(ownerPID)), layer == 0, alpha > 0,
       let number = window[kCGWindowNumber as String] as? Int {
      print("\(number) \(ownerPID)")
      found = true
      break
    }
  }
  if !found { Thread.sleep(forTimeInterval: 0.25) }
} while !found && Date() < deadline' > "$OUT/$STAMP-desktop-window.txt" 2>/dev/null || true
  WINDOW_INFO="$(cat "$OUT/$STAMP-desktop-window.txt" 2>/dev/null || true)"
  if [[ "$WINDOW_INFO" =~ ^([0-9]+)[[:space:]]([0-9]+)$ ]]; then
    WINDOW="${BASH_REMATCH[1]}"
    WINDOW_PID="${BASH_REMATCH[2]}"
    printf '[capture] Desktop window PID: %s (window %s)\n' "$WINDOW_PID" "$WINDOW"
    if python3 - "$WINDOW" "$OUT/$STAMP-desktop.png" <<'PY'
import os
from pathlib import Path
import subprocess
import sys
output = Path(sys.argv[2])
try:
    output.unlink(missing_ok=True)
    result = subprocess.run(["/usr/sbin/screencapture", "-x", "-l", sys.argv[1], str(output)],
                            timeout=float(os.environ.get("MACBOT_SCREENSHOT_TIMEOUT", "15")))
    if result.returncode or not output.is_file() or not output.stat().st_size:
        raise RuntimeError("No desktop screenshot produced")
except (subprocess.TimeoutExpired, RuntimeError) as exc:
    output.unlink(missing_ok=True)
    print(f"[capture] Desktop capture failed: {exc}", file=sys.stderr)
    sys.exit(1)
PY
    then
      {
        printf 'app=%s\n' "$INSTALL_APP"
        printf 'executable=%s\n' "$INSTALL_BINARY"
        printf 'window_pid=%s\n' "$WINDOW_PID"
        printf 'window_id=%s\n' "$WINDOW"
        printf 'source_commit=%s\n' "${INSTALL_SOURCE_COMMIT:-unknown}"
      } > "$OUT/$STAMP-desktop.txt"
      printf '[capture] Desktop metadata: %s\n' "$OUT/$STAMP-desktop.txt"
    else
      FAILED=1
    fi
  else
    echo '[capture] Installed desktop process has no visible on-screen window; desktop capture failed.' >&2
    FAILED=1
  fi
fi
rm -f "$OUT/$STAMP-desktop-window.txt"
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
