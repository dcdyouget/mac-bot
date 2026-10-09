#!/bin/bash
set -uo pipefail

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
. "$SCRIPT_DIR/common.sh"

failures=0
macbot_acquire_lock || exit 1
trap macbot_release_lock EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
if [ -n "${MACBOT_DEPLOY_SHA-}" ]; then
  macbot_resolve_main_sha || exit 1
else
  macbot_sync_main_ref || true
  macbot_resolve_main_sha || exit 1
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
  sidecar_script="$MACBOT_SOURCE_DIR/server/macbotd/packaging/prepare-sidecar.sh"
  if [ -f "$sidecar_script" ]; then
    "$sidecar_script" "$app_path/Contents/MacOS/agent-browser" || return 1
    cp "$MACBOT_SOURCE_DIR/server/macbotd/packaging/agent-browser.LICENSE" \
      "$app_path/Contents/Resources/agent-browser.LICENSE" || return 1
  else
    macbot_warn "server：此版本没有浏览器 sidecar 打包入口，浏览器验收待补"
  fi
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
if [ "${MACBOT_SKIP_PRODUCTION:-0}" = 1 ]; then
  macbot_log "server：当前仅部署 mock，正式执行链路尚未 ready；跳过 7788"
  server_rc=2
else
  macbot_build_server || server_rc=$?
fi
case "$server_rc" in
  0)
    if ! install_server; then
      failures=1
    elif ! macbot_wait_health 7788 30 || ! macbot_verify_service_pid "$MACBOT_SERVER_LABEL" 7788 || ! macbot_verify_rpc_access 7788; then
      failures=1
    fi
    ;;
  2) ;;
  *) failures=1 ;;
esac

desktop_rc=0
if [ "${MACBOT_SKIP_DESKTOP:-0}" = 1 ]; then
  macbot_log "client-mac：保留当前安装与验证窗口，跳过构建/安装"
  desktop_rc=2
else
  macbot_build_desktop || desktop_rc=$?
fi
case "$desktop_rc" in
  0)
    desktop_destination="$HOME/Applications/MacBot.app"
    desktop_previous_pids=$(pgrep -f "$desktop_destination/Contents/MacOS/" 2>/dev/null || true)
    for desktop_pid in $desktop_previous_pids; do kill -TERM "$desktop_pid" 2>/dev/null || true; done
    desktop_stopped=0
    for ((desktop_attempt=0; desktop_attempt<40; desktop_attempt++)); do
      desktop_alive=0
      for desktop_pid in $desktop_previous_pids; do
        if kill -0 "$desktop_pid" 2>/dev/null; then desktop_alive=1; fi
      done
      if [ "$desktop_alive" -eq 0 ]; then desktop_stopped=1; break; fi
      sleep 0.25
    done
    if [ "$desktop_stopped" -ne 1 ]; then
      macbot_error "旧桌面进程未在 10 秒内退出；保留现有安装，下次重试"
      failures=1
    elif ! rm -rf "$desktop_destination" || \
       ! mkdir -p "$HOME/Applications" || \
       ! cp -R "$MACBOT_DESKTOP_APP" "$desktop_destination"; then
      macbot_error "桌面客户端安装失败"
      failures=1
    else
      printf '%s\n' "$MACBOT_MAIN_SHA" > "$desktop_destination/Contents/Resources/source-commit" || failures=1
      macbot_log "桌面客户端已安装：$desktop_destination"
      desktop_open_args=(-n -g)
      if [ -f "$MACBOT_CACHE_ROOT/watch/file-secrets" ]; then
        desktop_open_args+=(--env MACBOT_SECRET_BACKEND=file)
      fi
      if ! open "${desktop_open_args[@]}" "$desktop_destination"; then
        macbot_error "桌面客户端已安装但无法打开"
        failures=1
      fi
    fi
    ;;
  2) ;;
  *) failures=1 ;;
esac

android_rc=0
if [ "${MACBOT_SKIP_ANDROID:-0}" = 1 ]; then
  macbot_log "client-android：保留当前 UI 验证会话，跳过安装/重启"
  android_rc=2
else
  macbot_build_android || android_rc=$?
fi
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
