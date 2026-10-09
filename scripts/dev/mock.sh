#!/bin/bash
set -uo pipefail

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
. "$SCRIPT_DIR/common.sh"

macbot_acquire_lock || exit 1
trap macbot_release_lock EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
macbot_prepare_main_source || exit 1
macbot_ensure_log_dir

build_rc=0
macbot_build_server || build_rc=$?
case "$build_rc" in
  0) ;;
  2)
    macbot_log "mock：main 中没有 server 代码，跳过"
    exit 2
    ;;
  *)
    exit 1
    ;;
esac

mock_app="$HOME/Applications/MacBotMock.app"
mock_binary="$mock_app/Contents/MacOS/macbotd"
rm -rf "$mock_app" || exit 1
mkdir -p "$mock_app/Contents/MacOS" "$mock_app/Contents/Resources" || exit 1
cp "$MACBOT_SERVER_BINARY" "$mock_binary" || exit 1
chmod 755 "$mock_binary" || exit 1
printf '%s\n' "$MACBOT_MAIN_SHA" > "$mock_app/Contents/Resources/source-commit" || exit 1
printf '%s\n' \
  '<?xml version="1.0" encoding="UTF-8"?>' \
  '<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">' \
  '<plist version="1.0"><dict>' \
  '<key>CFBundleExecutable</key><string>macbotd</string>' \
  '<key>CFBundleIdentifier</key><string>com.macbot.mock</string>' \
  '<key>CFBundleName</key><string>MacBot Mock</string>' \
  '<key>CFBundlePackageType</key><string>APPL</string>' \
  '<key>CFBundleShortVersionString</key><string>0.1.0</string>' \
  '<key>CFBundleVersion</key><string>0.1.0</string>' \
  '<key>LSUIElement</key><true/>' \
  '</dict></plist>' > "$mock_app/Contents/Info.plist" || exit 1

macbot_install_launch_agent \
  "$MACBOT_MOCK_LABEL" "$mock_binary" "$HOME/MacBot-mock" \
  "$MACBOT_MOCK_LOG" "$MACBOT_MOCK_ERR_LOG" \
  --mock --port 7789 --password dev || exit 1

macbot_wait_health 7789 30 || exit 1
macbot_verify_service_pid "$MACBOT_MOCK_LABEL" 7789
