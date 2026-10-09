#!/bin/bash
set -uo pipefail

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
. "$SCRIPT_DIR/common.sh"

failures=0
macbot_acquire_lock || exit 1
trap macbot_release_lock EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
fetch_log="$MACBOT_LOG_DIR/fetch.log"
mkdir -p "$MACBOT_LOG_DIR" || exit 1
fetch_timed_out=0
GIT_TERMINAL_PROMPT=0 git -C "$MACBOT_REPO_ROOT" fetch origin main >> "$fetch_log" 2>&1 &
fetch_pid=$!
fetch_elapsed=0
while kill -0 "$fetch_pid" >/dev/null 2>&1; do
  if [ "$fetch_elapsed" -ge 15 ]; then
    kill "$fetch_pid" >/dev/null 2>&1 || true
    fetch_timed_out=1
    macbot_warn "git fetch origin main 超时；继续使用本地 main"
    break
  fi
  sleep 1
  fetch_elapsed=$((fetch_elapsed + 1))
done
if [ "$fetch_timed_out" -eq 0 ] && ! wait "$fetch_pid"; then
  macbot_warn "git fetch origin main 失败；继续使用本地 main"
fi
origin_main_sha=$(git -C "$MACBOT_REPO_ROOT" rev-parse --verify origin/main^{commit} 2>/dev/null || true)
local_main_sha=$(git -C "$MACBOT_REPO_ROOT" rev-parse --verify main^{commit} 2>/dev/null || true)
if [ -n "$origin_main_sha" ] && [ -n "$local_main_sha" ]; then
  merge_base=$(git -C "$MACBOT_REPO_ROOT" merge-base main origin/main 2>/dev/null || true)
  if [ "$merge_base" = "$local_main_sha" ] && [ "$merge_base" != "$origin_main_sha" ]; then
    macbot_warn "本地 main 落后 origin/main；按约定继续部署本地 main"
  elif [ "$merge_base" != "$local_main_sha" ] && [ "$merge_base" != "$origin_main_sha" ]; then
    macbot_warn "本地 main 与 origin/main 已分叉；按约定继续部署本地 main"
  fi
fi
macbot_prepare_main_source || exit 1
macbot_ensure_password || exit 1
macbot_ensure_log_dir

install_server() {
  app_path="$HOME/Applications/MacBotServer.app"
  binary_path="$app_path/Contents/MacOS/macbotd"
  rm -rf "$app_path" || return 1
  mkdir -p "$app_path/Contents/MacOS" "$app_path/Contents/Resources" || return 1
  cp "$MACBOT_SERVER_BINARY" "$binary_path" || return 1
  chmod 755 "$binary_path" || return 1
  printf '%s\n' "$MACBOT_MAIN_SHA" > "$app_path/Contents/Resources/source-commit" || return 1
  printf '%s\n' \
    '<?xml version="1.0" encoding="UTF-8"?>' \
    '<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">' \
    '<plist version="1.0"><dict>' \
    '<key>CFBundleExecutable</key><string>macbotd</string>' \
    '<key>CFBundleIdentifier</key><string>com.macbot.server</string>' \
    '<key>CFBundleName</key><string>MacBot Server</string>' \
    '<key>CFBundlePackageType</key><string>APPL</string>' \
    '<key>CFBundleShortVersionString</key><string>0.1.0</string>' \
    '<key>CFBundleVersion</key><string>0.1.0</string>' \
    '<key>LSUIElement</key><true/>' \
    '</dict></plist>' > "$app_path/Contents/Info.plist" || return 1
  macbot_install_launch_agent \
    "$MACBOT_SERVER_LABEL" "$binary_path" "$HOME/MacBot" \
    "$MACBOT_SERVER_LOG" "$MACBOT_SERVER_ERR_LOG" \
    --port 7788 --password "$MACBOT_PASSWORD"
}

server_rc=0
macbot_build_server || server_rc=$?
case "$server_rc" in
  0)
    if ! install_server; then
      failures=1
    elif ! macbot_wait_health 7788 30 || ! macbot_verify_service_pid "$MACBOT_SERVER_LABEL" 7788; then
      failures=1
    fi
    ;;
  2) ;;
  *) failures=1 ;;
esac

desktop_rc=0
macbot_build_desktop || desktop_rc=$?
case "$desktop_rc" in
  0)
    desktop_destination="$HOME/Applications/MacBot.app"
    if ! rm -rf "$desktop_destination" || \
       ! mkdir -p "$HOME/Applications" || \
       ! cp -R "$MACBOT_DESKTOP_APP" "$desktop_destination"; then
      macbot_error "桌面客户端安装失败"
      failures=1
    else
      printf '%s\n' "$MACBOT_MAIN_SHA" > "$desktop_destination/Contents/Resources/source-commit" || failures=1
      macbot_log "桌面客户端已安装：$desktop_destination"
    fi
    ;;
  2) ;;
  *) failures=1 ;;
esac

android_rc=0
macbot_build_android || android_rc=$?
case "$android_rc" in
  0)
    macbot_install_android || failures=1
    ;;
  2) ;;
  *) failures=1 ;;
esac

if [ "$failures" -ne 0 ]; then
  macbot_error "部署存在失败组件；已继续处理独立组件"
  exit 1
fi
macbot_log "部署完成（main ${MACBOT_MAIN_SHA}）"
