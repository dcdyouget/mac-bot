#!/bin/bash
set -u

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
. "$SCRIPT_DIR/common.sh"

main_sha=$(git -C "$MACBOT_REPO_ROOT" rev-parse --verify main^{commit} 2>/dev/null || printf 'unknown')
printf 'MacBot status\n'
printf 'main sha: %s\n' "$main_sha"
printf 'host: %s\n' "$(scutil --get ComputerName 2>/dev/null || hostname)"

show_service() {
  title="$1"; label="$2"; port="$3"; binary="$4"; log_path="$5"
  agent_pid=$(macbot_launchctl_pid "$label")
  listen_pid=$(macbot_port_pid "$port")
  if [ -n "$agent_pid" ]; then agent_state="running (pid $agent_pid)"; else agent_state="stopped"; fi
  if [ -n "$listen_pid" ]; then port_state="listening (pid $listen_pid)"; else port_state="closed"; fi
  printf '\n[%s]\n' "$title"
  source_marker="$(dirname "$binary")/../Resources/source-commit"
  printf 'installed sha: %s\n' "$(cat "$source_marker" 2>/dev/null || printf 'unknown')"
  printf 'launchagent: %s\n' "$agent_state"
  printf 'port %s: %s\n' "$port" "$port_state"
  if [ -x "$binary" ]; then
    printf 'binary: installed\n'
  else
    printf 'binary: missing (%s)\n' "$binary"
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

show_service "server" "$MACBOT_SERVER_LABEL" 7788 \
  "$HOME/Applications/MacBotServer.app/Contents/MacOS/macbotd" "$MACBOT_SERVER_LOG"
macbot_safe_log_tail "$MACBOT_SERVER_ERR_LOG" 8
show_service "mock" "$MACBOT_MOCK_LABEL" 7789 \
  "$HOME/Applications/MacBotMock.app/Contents/MacOS/macbotd" "$MACBOT_MOCK_LOG"
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
if macbot_find_android_sdk; then
  if macbot_find_android_serial; then
    printf 'avd: %s (%s)\n' "$MACBOT_AVD_NAME" "$MACBOT_ANDROID_SERIAL"
    package_line=$("$MACBOT_ANDROID_ADB" -s "$MACBOT_ANDROID_SERIAL" shell dumpsys package bot.mac.mobile 2>/dev/null | awk '/versionName=/{print; exit}')
    if [ -n "$package_line" ]; then printf 'package: %s\n' "$package_line"; else printf 'package: not installed\n'; fi
    android_pid=$("$MACBOT_ANDROID_ADB" -s "$MACBOT_ANDROID_SERIAL" shell pidof bot.mac.mobile 2>/dev/null | tr -d '\r' || true)
    printf 'process: %s\n' "${android_pid:-stopped}"
  else
    printf 'avd: %s (stopped)\n' "$MACBOT_AVD_NAME"
  fi
else
  printf 'sdk: unavailable\n'
fi
exit 0
