#!/bin/bash
set -u

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
. "$SCRIPT_DIR/common.sh"

main_sha=$(git -C "$MACBOT_REPO_ROOT" rev-parse --verify main^{commit} 2>/dev/null || printf 'unknown')
printf 'MacBot status\n'
printf 'main sha: %s\n' "$main_sha"
printf 'host: %s\n' "$(scutil --get ComputerName 2>/dev/null || hostname)"
printf '\n[deployment watcher]\n'
if [ -f "$MACBOT_CACHE_ROOT/watch/ui-validation-hold" ]; then
  printf 'deployment hold: active (shared emulator/mock UI validation)\n'
fi
if [ -f "$MACBOT_CACHE_ROOT/watch/mock-only" ]; then
  printf 'automatic production deployment: held (mock-only flag; see service status below)\n'
fi
watch_pid=$(macbot_launchctl_pid bot.mac.integrator.watch)
if [ -n "$watch_pid" ]; then printf 'process: running (pid %s)\n' "$watch_pid"; else printf 'process: scheduled, currently idle\n'; fi
python3 - "$MACBOT_CACHE_ROOT/watch/latest.json" <<'PY'
import json
import pathlib
import sys
path = pathlib.Path(sys.argv[1])
if path.exists():
    data = json.loads(path.read_text())
    print('last completed sha:', data.get('sha', 'unknown'))
    print('last completed S0 API:', data.get('s0_api_status', 'not run'))
else:
    print('No completed deployment yet')
PY

macbot_pid_executable() {
  pid="$1"
  [ -n "$pid" ] || return 0
  if [ -x /usr/sbin/lsof ]; then
    /usr/sbin/lsof -n -p "$pid" -a -d txt -Fn 2>/dev/null |
      sed -n 's/^n//p' | sed -n '1p'
  fi
}

macbot_source_marker() {
  binary="$1"
  case "$binary" in
    */Contents/MacOS/*)
      app_root=${binary%/Contents/MacOS/*}
      ;;
    *)
      return 0
      ;;
  esac
  for marker in \
    "$app_root/Contents/Resources/source-commit" \
    "$app_root/Contents/Resources/source-commit.txt"; do
    if [ -f "$marker" ]; then
      tr -d '\r\n' < "$marker"
      return 0
    fi
  done
  if [ -f "$app_root/Contents/Info.plist" ]; then
    /usr/libexec/PlistBuddy -c 'Print :MacBotSourceCommit' \
      "$app_root/Contents/Info.plist" 2>/dev/null || true
  fi
}

show_service() {
  title="$1"; label="$2"; port="$3"; log_path="$4"; shift 4
  agent_pid=$(macbot_launchctl_pid "$label")
  listen_pid=$(macbot_port_pid "$port")
  if [ -n "$agent_pid" ]; then agent_state="running (pid $agent_pid)"; else agent_state="stopped"; fi
  if [ -n "$listen_pid" ]; then port_state="listening (pid $listen_pid)"; else port_state="closed"; fi
  agent_binary=$(macbot_pid_executable "$agent_pid")
  listen_binary=$(macbot_pid_executable "$listen_pid")
  actual_binary="$listen_binary"
  [ -n "$actual_binary" ] || actual_binary="$agent_binary"
  printf '\n[%s]\n' "$title"
  printf 'launchagent: %s\n' "$agent_state"
  printf 'port %s: %s\n' "$port" "$port_state"
  if [ -n "$agent_pid" ] && [ -n "$listen_pid" ]; then
    if [ "$agent_pid" = "$listen_pid" ]; then
      printf 'pid match: yes (%s)\n' "$agent_pid"
    else
      printf 'pid match: no (agent %s, listen %s)\n' "$agent_pid" "$listen_pid"
    fi
  else
    printf 'pid match: unknown (agent/listen PID incomplete)\n'
  fi
  if [ -n "$agent_binary" ]; then printf 'agent binary: %s\n' "$agent_binary"; fi
  if [ -n "$listen_binary" ]; then printf 'listen binary: %s\n' "$listen_binary"; fi
  if [ -n "$agent_binary" ] && [ -n "$listen_binary" ]; then
    if [ "$agent_binary" = "$listen_binary" ]; then
      printf 'binary match: yes\n'
    else
      printf 'binary match: no\n'
    fi
  fi
  if [ -n "$actual_binary" ]; then
    printf 'binary source: %s\n' "$actual_binary"
    source_sha=$(macbot_source_marker "$actual_binary")
    printf 'installed sha: %s\n' "${source_sha:-unknown}"
  else
    printf 'binary source: unavailable (no process executable resolved)\n'
    printf 'installed sha: unknown\n'
  fi
  printf 'known paths:\n'
  known_path_count=0
  for candidate in "$@"; do
    if [ -e "$candidate" ]; then
      known_path_count=$((known_path_count + 1))
      candidate_sha=$(macbot_source_marker "$candidate")
      printf '  %s (%s)\n' "$candidate" "${candidate_sha:-source unknown}"
    fi
  done
  if [ "$known_path_count" -eq 0 ]; then
    printf '  none\n'
  fi
  if macbot_have curl; then
    health=$(curl --fail --silent --show-error --max-time 2 "http://127.0.0.1:$port/api/v1/health" 2>/dev/null || true)
    if [ -n "$health" ]; then
      printf 'health: %s\n' "$health" | sed -E \
        's/(--password|--api-key)[=[:space:]]+[^[:space:]]+/\1=<redacted>/g; s/(Bearer[[:space:]]+)[^[:space:]]*/\1<redacted>/gI; s/("(password|api_key|authorization)"[[:space:]]*:[[:space:]]*)"[^"]*"/\1"<redacted>"/gI; s/(password|api[_-]?key|authorization)([=:])[[:space:]]*[^,}[:space:]]*/\1\2<redacted>/gI'
    else
      printf 'health: unavailable\n'
    fi
  fi
  printf 'recent logs (%s):\n' "$log_path"
  macbot_safe_log_tail "$log_path" 8
}

show_service "server" "$MACBOT_SERVER_LABEL" 7788 "$MACBOT_SERVER_LOG" \
  "$HOME/Applications/MacBotServer.app/Contents/MacOS/macbotd" \
  "$HOME/Applications/MacBot Server.app/Contents/MacOS/macbotd" \
  "/Applications/MacBot Server.app/Contents/MacOS/macbotd"
macbot_safe_log_tail "$MACBOT_SERVER_ERR_LOG" 8
show_service "mock" "$MACBOT_MOCK_LABEL" 7789 "$MACBOT_MOCK_LOG" \
  "$HOME/Applications/MacBotMock.app/Contents/MacOS/macbotd"
macbot_safe_log_tail "$MACBOT_MOCK_ERR_LOG" 8

printf '\n[desktop]\n'
desktop_app="$HOME/Applications/MacBot.app"
printf 'installed sha: %s\n' "$(cat "$desktop_app/Contents/Resources/source-commit" 2>/dev/null || printf 'unknown')"
if [ -d "$desktop_app" ]; then
  desktop_version=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$desktop_app/Contents/Info.plist" 2>/dev/null || true)
  printf 'app: installed (%s)\n' "$desktop_app"
  [ -n "$desktop_version" ] && printf 'version: %s\n' "$desktop_version"
else
  printf 'app: missing (%s)\n' "$desktop_app"
fi
desktop_pid=$(pgrep -f "$desktop_app/Contents/MacOS/" 2>/dev/null | sed -n '1p' || true)
if [ -n "$desktop_pid" ]; then printf 'process: running (pid %s)\n' "$desktop_pid"; else printf 'process: stopped\n'; fi

printf '\n[android]\n'
printf 'installed sha: %s\n' "$(cat "$MACBOT_CACHE_ROOT/android-installed-sha" 2>/dev/null || printf 'unknown')"
printf 'installed variant: %s\n' "$(cat "$MACBOT_CACHE_ROOT/android-installed-variant" 2>/dev/null || printf 'unknown')"
if macbot_find_android_sdk; then
  if macbot_find_android_serial; then
    printf 'avd: %s (%s)\n' "$MACBOT_AVD_NAME" "$MACBOT_ANDROID_SERIAL"
    package_line=$("$MACBOT_ANDROID_ADB" -s "$MACBOT_ANDROID_SERIAL" shell dumpsys package bot.mac.mobile 2>/dev/null | awk '/versionName=/{print; exit}')
    if [ -n "$package_line" ]; then printf 'package: %s\n' "$package_line"; else printf 'package: not installed\n'; fi
    android_pid=$("$MACBOT_ANDROID_ADB" -s "$MACBOT_ANDROID_SERIAL" shell pidof bot.mac.mobile 2>/dev/null | tr -d '\r' || true)
    printf 'process: %s\n' "${android_pid:-stopped}"
    python3 - "$MACBOT_ANDROID_ADB" "$MACBOT_ANDROID_SERIAL" <<'PY'
import subprocess
import sys
import time
import os
import signal
adb, serial = sys.argv[1:]

def run_adb_nc(*args, timeout=5):
    return subprocess.run(
        [adb, "-s", serial, "shell", "toybox", "nc", *args],
        capture_output=True,
        text=True,
        timeout=timeout,
    )

def kill_process_group(proc):
    try:
        os.killpg(proc.pid, signal.SIGKILL)
    except (OSError, ProcessLookupError):
        proc.kill()
    proc.wait()

try:
    route = subprocess.run([adb, "-s", serial, "shell", "ip", "route"],
                           capture_output=True, text=True, timeout=5)
    route_ok = route.returncode == 0 and any(
        line.strip() for line in route.stdout.splitlines()
    )
    print("network route:", "present" if route_ok else "missing")
    request = "GET /api/v1/health HTTP/1.1\r\nHost: 10.0.2.2\r\nConnection: close\r\n\r\n"
    for port, label in ((7788, "formal"), (7789, "mock")):
        if not route_ok:
            print(label + " from emulator: no route")
            continue
        try:
            tcp = run_adb_nc("-n", "-z", "-w", "3", "10.0.2.2", str(port))
        except subprocess.TimeoutExpired:
            print(label + " from emulator: TCP probe timeout")
            continue
        if tcp.returncode != 0:
            print(label + " from emulator: TCP unavailable")
            continue
        proc = subprocess.Popen(
            [adb, "-s", serial, "shell", "toybox", "nc", "-n", "-w", "3", "10.0.2.2", str(port)],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            text=True, start_new_session=True)
        try:
            proc.stdin.write(request)
            proc.stdin.flush()
            # Some toybox nc builds report EOF immediately if stdin is closed before
            # the server has had a chance to write the HTTP response.
            time.sleep(0.25)
            proc.stdin.close()
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            kill_process_group(proc)
            print(label + " from emulator: HTTP probe timeout")
            continue
        except (BrokenPipeError, OSError):
            kill_process_group(proc)
            print(label + " from emulator: HTTP probe transport error")
            continue
        stdout = proc.stdout.read()
        header = stdout.split("\r\n\r\n", 1)[0]
        if proc.returncode != 0:
            print(label + " from emulator: HTTP probe transport error")
        elif "200 OK" in header:
            print(label + " from emulator: HTTP 200")
        else:
            print(label + " from emulator: HTTP probe failed")
except (subprocess.TimeoutExpired, subprocess.SubprocessError, OSError):
    print("emulator network probe: unavailable")
PY
  else
    printf 'avd: %s (stopped)\n' "$MACBOT_AVD_NAME"
  fi
else
  printf 'sdk: unavailable\n'
fi
exit 0
