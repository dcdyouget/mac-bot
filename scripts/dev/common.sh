#!/bin/bash

# Shared macOS Bash 3.2 compatible helpers.

MACBOT_SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
MACBOT_REPO_ROOT=$(CDPATH= cd -- "$MACBOT_SCRIPT_DIR/../.." && pwd)
MACBOT_CACHE_ROOT=${MACBOT_CACHE_ROOT:-"$HOME/Library/Caches/MacBot/integrator"}
MACBOT_SOURCE_ROOT="$MACBOT_CACHE_ROOT/source"
MACBOT_TARGET_ROOT="$MACBOT_CACHE_ROOT/target"
MACBOT_LOCK_DIR="$MACBOT_CACHE_ROOT/deploy.lock"
MACBOT_MAIN_SHA=
MACBOT_FETCH_OK=
MACBOT_SOURCE_DIR=
MACBOT_SERVER_BINARY=
MACBOT_DESKTOP_APP=
MACBOT_ANDROID_APK=
MACBOT_ANDROID_SERIAL=
MACBOT_ANDROID_SDK=
MACBOT_ANDROID_ADB=
MACBOT_ANDROID_EMULATOR=
MACBOT_AVD_NAME=${MACBOT_AVD_NAME:-macbot_api36}
MACBOT_SERVER_LABEL=${MACBOT_SERVER_LABEL:-com.macbot.server}
MACBOT_MOCK_LABEL=${MACBOT_MOCK_LABEL:-com.macbot.mock}
MACBOT_LOG_DIR="$HOME/Library/Logs/MacBot"
MACBOT_SERVER_LOG="$MACBOT_LOG_DIR/server.out.log"
MACBOT_SERVER_ERR_LOG="$MACBOT_LOG_DIR/server.err.log"
MACBOT_MOCK_LOG="$MACBOT_LOG_DIR/mock.out.log"
MACBOT_MOCK_ERR_LOG="$MACBOT_LOG_DIR/mock.err.log"

macbot_log() { printf '[MacBot] %s\n' "$*"; }
macbot_warn() { printf '[MacBot] warning: %s\n' "$*" >&2; }
macbot_error() { printf '[MacBot] error: %s\n' "$*" >&2; }
macbot_have() { command -v "$1" >/dev/null 2>&1; }

macbot_sync_main_ref() {
  if [ -n "${MACBOT_DEPLOY_SHA-}" ]; then
    MACBOT_FETCH_OK=skipped
    return 0
  fi
  if ! macbot_have python3; then
    MACBOT_FETCH_OK=failed
    macbot_warn "找不到 python3，无法 fetch origin/main；继续按现有本地 refs 解析"
    return 1
  fi
  fetch_stderr=$(mktemp "${TMPDIR:-/tmp}/macbot-fetch.XXXXXX") || {
    MACBOT_FETCH_OK=failed
    macbot_warn "无法创建 fetch 临时文件；继续按现有本地 refs 解析"
    return 1
  }
  python3 - "$MACBOT_REPO_ROOT" > /dev/null 2> "$fetch_stderr" <<'PY'
import os
import subprocess
import sys

try:
    result = subprocess.run(
        ["git", "fetch", "origin", "main"],
        cwd=sys.argv[1],
        env=dict(os.environ, GIT_TERMINAL_PROMPT="0"),
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        timeout=15,
    )
except subprocess.TimeoutExpired:
    print("timeout after 15 seconds", file=sys.stderr)
    raise SystemExit(124)
if result.returncode and result.stderr:
    print(result.stderr[-1000:], file=sys.stderr)
raise SystemExit(result.returncode)
PY
  fetch_rc=$?
  if [ "$fetch_rc" -eq 0 ]; then
    MACBOT_FETCH_OK=ok
    rm -f "$fetch_stderr"
    return 0
  fi
  fetch_detail=$(tr '\n' ' ' < "$fetch_stderr" | sed -E \
    's#(https?://)[^/@[:space:]]+@#\1<redacted>@#g; s/(password|token|key)[=:][^[:space:]]*/\1=<redacted>/gI' | cut -c 1-240)
  rm -f "$fetch_stderr"
  MACBOT_FETCH_OK=failed
  if [ -n "$fetch_detail" ]; then
    macbot_warn "git fetch origin/main 失败（继续按现有本地 refs 解析）：$fetch_detail"
  else
    macbot_warn "git fetch origin/main 失败（继续按现有本地 refs 解析）"
  fi
  return 1
}

macbot_resolve_main_sha() {
  requested_sha=${MACBOT_DEPLOY_SHA-}
  if [ -n "$requested_sha" ]; then
    case "$requested_sha" in
      *[!0-9a-fA-F]*)
        macbot_error "MACBOT_DEPLOY_SHA 不是十六进制 commit SHA"
        return 1
        ;;
    esac
    resolved_sha=$(git -C "$MACBOT_REPO_ROOT" rev-parse --verify "$requested_sha^{commit}" 2>/dev/null || true)
    if [ -z "$resolved_sha" ]; then
      macbot_error "MACBOT_DEPLOY_SHA 不存在：$requested_sha"
      return 1
    fi
    MACBOT_MAIN_SHA="$resolved_sha"
    macbot_log "使用固定部署 SHA $MACBOT_MAIN_SHA"
    return 0
  fi

  local_sha=$(git -C "$MACBOT_REPO_ROOT" rev-parse --verify main^{commit} 2>/dev/null || true)
  if [ -z "$local_sha" ]; then
    macbot_error "无法解析本地 main"
    return 1
  fi
  remote_sha=$(git -C "$MACBOT_REPO_ROOT" rev-parse --verify origin/main^{commit} 2>/dev/null || true)
  if [ -z "$remote_sha" ]; then
    MACBOT_MAIN_SHA="$local_sha"
    macbot_warn "没有 origin/main；使用本地 main $MACBOT_MAIN_SHA"
    return 0
  fi
  if [ "$local_sha" = "$remote_sha" ]; then
    MACBOT_MAIN_SHA="$local_sha"
    return 0
  fi
  if git -C "$MACBOT_REPO_ROOT" merge-base --is-ancestor "$local_sha" "$remote_sha"; then
    MACBOT_MAIN_SHA="$remote_sha"
    macbot_log "origin/main 是本地 main 的后代；使用远端 SHA $MACBOT_MAIN_SHA"
  elif git -C "$MACBOT_REPO_ROOT" merge-base --is-ancestor "$remote_sha" "$local_sha"; then
    MACBOT_MAIN_SHA="$local_sha"
    macbot_log "本地 main 领先 origin/main；使用本地 SHA $MACBOT_MAIN_SHA"
  else
    macbot_error "本地 main 与 origin/main 已分叉，拒绝部署"
    return 1
  fi
}

macbot_acquire_lock() {
  mkdir -p "$MACBOT_CACHE_ROOT" || return 1
  elapsed=0
  while ! mkdir "$MACBOT_LOCK_DIR" 2>/dev/null; do
    if [ -f "$MACBOT_LOCK_DIR/pid" ]; then
      lock_pid=$(cat "$MACBOT_LOCK_DIR/pid")
      if [[ "$lock_pid" =~ ^[0-9]+$ ]] && ! kill -0 "$lock_pid" 2>/dev/null; then
        rm -f "$MACBOT_LOCK_DIR/pid"
        rmdir "$MACBOT_LOCK_DIR" 2>/dev/null || true
        continue
      fi
    else
      # An interrupted owner can remove pid before leaving an empty lock directory.
      # Keep a grace period for a new owner between mkdir and writing its pid.
      lock_mtime=$(stat -f %m "$MACBOT_LOCK_DIR" 2>/dev/null || true)
      if [[ "$lock_mtime" =~ ^[0-9]+$ ]] && [ "$(( $(date +%s) - lock_mtime ))" -ge 30 ]; then
        if rmdir "$MACBOT_LOCK_DIR" 2>/dev/null; then continue; fi
      fi
    fi
    if [ "$elapsed" -ge 180 ]; then
      macbot_error "已有另一个集成部署运行超过 180 秒"; return 1
    fi
    sleep 1
    elapsed=$((elapsed + 1))
  done
  printf '%s\n' "$$" > "$MACBOT_LOCK_DIR/pid"
}

macbot_release_lock() {
  rm -f "$MACBOT_LOCK_DIR/pid"
  rmdir "$MACBOT_LOCK_DIR" 2>/dev/null || true
}

macbot_prepare_main_source() {
  [ -n "$MACBOT_MAIN_SHA" ] || macbot_resolve_main_sha || return 1
  MACBOT_SOURCE_DIR="$MACBOT_SOURCE_ROOT/$MACBOT_MAIN_SHA"
  mkdir -p "$MACBOT_SOURCE_ROOT" || return 1
  if [ -f "$MACBOT_SOURCE_DIR/.macbot-source-sha" ] && \
     [ "$(sed -n '1p' "$MACBOT_SOURCE_DIR/.macbot-source-sha")" = "$MACBOT_MAIN_SHA" ]; then
    macbot_log "使用 main 干净快照 $MACBOT_MAIN_SHA"
    return 0
  fi
  tmp_dir="$MACBOT_SOURCE_ROOT/.$MACBOT_MAIN_SHA.$$"
  rm -rf "$tmp_dir"
  mkdir -p "$tmp_dir" || return 1
  macbot_log "创建 main 干净快照 $MACBOT_MAIN_SHA"
  if ! git -C "$MACBOT_REPO_ROOT" archive --format=tar "$MACBOT_MAIN_SHA" | tar -x -C "$tmp_dir"; then
    rm -rf "$tmp_dir"; macbot_error "无法从 main 创建源码快照"; return 1
  fi
  printf '%s\n' "$MACBOT_MAIN_SHA" > "$tmp_dir/.macbot-source-sha"
  rm -rf "$MACBOT_SOURCE_DIR"
  mv "$tmp_dir" "$MACBOT_SOURCE_DIR"
}

macbot_target_path_exists() {
  [ -e "$1" ] || [ -L "$1" ]
}

macbot_prepare_target_dir() {
  component="$1"
  source_target="$2"
  shared_target="$MACBOT_TARGET_ROOT/$component"
  mkdir -p "$MACBOT_TARGET_ROOT" "$(dirname "$source_target")" || return 1

  if [ -L "$source_target" ]; then
    linked_target=$(readlink "$source_target" 2>/dev/null || true)
    if [ "$linked_target" != "$shared_target" ]; then
      macbot_error "$source_target 已指向非集成缓存 target：$linked_target"
      return 1
    fi
    mkdir -p "$shared_target" || return 1
    return 0
  fi

  if [ -e "$source_target" ]; then
    [ -d "$source_target" ] || {
      macbot_error "$source_target 不是目录，无法接入集成 target 缓存"; return 1;
    }
    if macbot_target_path_exists "$shared_target"; then
      unused_root="$MACBOT_TARGET_ROOT/unused"
      unused_target="$unused_root/${MACBOT_MAIN_SHA:-snapshot}-$component-$$"
      suffix=0
      while macbot_target_path_exists "$unused_target"; do
        suffix=$((suffix + 1))
        unused_target="$unused_root/${MACBOT_MAIN_SHA:-snapshot}-$component-$$-$suffix"
      done
      mkdir -p "$unused_root" || return 1
      mv "$source_target" "$unused_target" || {
        macbot_error "无法保留已有 $source_target（共享 target 已存在）"; return 1;
      }
      macbot_warn "共享 $component target 已存在；已有快照 target 已移到 $unused_target"
    else
      mv "$source_target" "$shared_target" || {
        macbot_error "无法接管已有 $source_target 为共享 $component target"; return 1;
      }
      macbot_log "接管已有 $component target：$shared_target"
    fi
  else
    mkdir -p "$shared_target" || return 1
  fi

  if ! macbot_target_path_exists "$shared_target" || [ ! -d "$shared_target" ]; then
    macbot_error "共享 $component target 不可用：$shared_target"
    return 1
  fi
  ln -s "$shared_target" "$source_target" || {
    macbot_error "无法将 $source_target 链接到共享 $component target"; return 1;
  }
}

macbot_ensure_log_dir() {
  mkdir -p "$MACBOT_LOG_DIR" || return 1
  chmod 700 "$MACBOT_LOG_DIR" 2>/dev/null || true
}

macbot_password_file() { printf '%s\n' "$HOME/.macbot-dev-password"; }

macbot_ensure_password() {
  password_path=$(macbot_password_file)
  if [ ! -f "$password_path" ]; then
    umask 077
    if macbot_have openssl; then
      openssl rand -hex 24 > "$password_path" 2>/dev/null || return 1
    else
      od -An -N24 -tx1 /dev/urandom | tr -d ' \n' > "$password_path" || return 1
    fi
  fi
  chmod 600 "$password_path" || return 1
  MACBOT_PASSWORD=$(sed -n '1p' "$password_path")
  [ -n "$MACBOT_PASSWORD" ] || { macbot_error "密码文件为空"; return 1; }
}

macbot_server_manifest() {
  if [ -f "$MACBOT_SOURCE_DIR/server/Cargo.toml" ]; then
    printf '%s\n' "$MACBOT_SOURCE_DIR/server/Cargo.toml"
  elif [ -f "$MACBOT_SOURCE_DIR/server/macbotd/Cargo.toml" ]; then
    printf '%s\n' "$MACBOT_SOURCE_DIR/server/macbotd/Cargo.toml"
  fi
}

macbot_build_server() {
  manifest=$(macbot_server_manifest)
  if [ -z "$manifest" ]; then
    macbot_log "server：main 中没有 Cargo workspace，跳过"; return 2
  fi
  macbot_have cargo || { macbot_error "server 有代码但找不到 cargo"; return 1; }
  server_target="$MACBOT_SOURCE_DIR/server/target"
  macbot_prepare_target_dir server "$server_target" || return 1
  macbot_log "编译 server：$manifest"
  (cd "$MACBOT_SOURCE_DIR" && CARGO_TARGET_DIR="$server_target" cargo build --release --manifest-path "$manifest") || {
    macbot_error "server 编译失败"; return 1;
  }
  MACBOT_SERVER_BINARY=
  for candidate in \
    "$MACBOT_SOURCE_DIR/server/target/release/macbotd" \
    "$MACBOT_SOURCE_DIR/server/macbotd/target/release/macbotd" \
    "$MACBOT_SOURCE_DIR/target/release/macbotd"; do
    if [ -x "$candidate" ]; then MACBOT_SERVER_BINARY="$candidate"; break; fi
  done
  if [ -z "$MACBOT_SERVER_BINARY" ]; then
    candidate=$(find "$MACBOT_SOURCE_DIR/server" -type f -path '*/target/release/macbotd' -perm -111 -print 2>/dev/null | sed -n '1p')
    [ -n "$candidate" ] && MACBOT_SERVER_BINARY="$candidate"
  fi
  [ -n "$MACBOT_SERVER_BINARY" ] || {
    macbot_error "server 编译完成但没有找到 macbotd"; return 1;
  }
}

macbot_desktop_manifest() {
  [ -f "$MACBOT_SOURCE_DIR/clients/mac/Cargo.toml" ] && \
    printf '%s\n' "$MACBOT_SOURCE_DIR/clients/mac/Cargo.toml"
}

macbot_make_app_bundle() {
  binary="$1"; app_path="$2"
  rm -rf "$app_path"
  mkdir -p "$app_path/Contents/MacOS" "$app_path/Contents/Resources"
  cp "$binary" "$app_path/Contents/MacOS/MacBot"
  chmod 755 "$app_path/Contents/MacOS/MacBot"
  printf '%s\n' \
    '<?xml version="1.0" encoding="UTF-8"?>' \
    '<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">' \
    '<plist version="1.0"><dict>' \
    '<key>CFBundleExecutable</key><string>MacBot</string>' \
    '<key>CFBundleIdentifier</key><string>com.macbot.desktop</string>' \
    '<key>CFBundleName</key><string>MacBot</string>' \
    '<key>CFBundlePackageType</key><string>APPL</string>' \
    '<key>CFBundleShortVersionString</key><string>0.1.0</string>' \
    '<key>CFBundleVersion</key><string>0.1.0</string>' \
    '<key>LSUIElement</key><true/>' \
    '</dict></plist>' > "$app_path/Contents/Info.plist"
}

macbot_build_desktop() {
  manifest=$(macbot_desktop_manifest)
  package_script=
  package_cwd="$MACBOT_SOURCE_DIR"
  for candidate in \
    "$MACBOT_SOURCE_DIR/clients/mac/packaging/package.sh" \
    "$MACBOT_SOURCE_DIR/scripts/package.sh" \
    "$MACBOT_SOURCE_DIR/clients/mac/scripts/package.sh" \
    "$MACBOT_SOURCE_DIR/clients/mac/package.sh"; do
    if [ -f "$candidate" ]; then
      package_script="$candidate"
      case "$candidate" in "$MACBOT_SOURCE_DIR/clients/mac/"*) package_cwd="$MACBOT_SOURCE_DIR/clients/mac" ;; esac
      break
    fi
  done
  if [ -z "$manifest" ] && [ -z "$package_script" ]; then
    macbot_log "client-mac：main 中没有工程或打包脚本，跳过"; return 2
  fi
  desktop_target="$MACBOT_SOURCE_DIR/clients/mac/target"
  macbot_prepare_target_dir desktop "$desktop_target" || return 1
  if [ -n "$package_script" ]; then
    macbot_log "执行桌面打包脚本：$package_script"
    (cd "$package_cwd" && CARGO_TARGET_DIR="$desktop_target" /bin/zsh "$package_script" debug app) || {
      macbot_error "桌面打包失败"; return 1;
    }
    MACBOT_DESKTOP_APP="$MACBOT_SOURCE_DIR/clients/mac/dist/MacBot.app"
    [ -d "$MACBOT_DESKTOP_APP" ] || {
      macbot_error "打包脚本成功但没有生成 clients/mac/dist/MacBot.app"; return 1;
    }
    return 0
  fi
  macbot_have cargo || { macbot_error "client-mac 有代码但找不到 cargo"; return 1; }
  macbot_log "编译 client-mac：$manifest"
  (cd "$MACBOT_SOURCE_DIR" && CARGO_TARGET_DIR="$desktop_target" cargo build --release --manifest-path "$manifest") || {
    macbot_error "client-mac 编译失败"; return 1;
  }
  binary=
  for candidate in \
    "$MACBOT_SOURCE_DIR/clients/mac/target/release/macbot-desktop" \
    "$MACBOT_SOURCE_DIR/target/release/macbot-desktop"; do
    if [ -x "$candidate" ]; then binary="$candidate"; break; fi
  done
  [ -n "$binary" ] || {
    macbot_error "client-mac 编译完成但没有找到 macbot-desktop"; return 1;
  }
  MACBOT_DESKTOP_APP="$MACBOT_SOURCE_DIR/clients/mac/dist/MacBot.app"
  macbot_make_app_bundle "$binary" "$MACBOT_DESKTOP_APP"
}

macbot_find_android_sdk() {
  if [ -n "${ANDROID_HOME:-}" ] && [ -d "$ANDROID_HOME" ]; then
    MACBOT_ANDROID_SDK="$ANDROID_HOME"
  elif [ -n "${ANDROID_SDK_ROOT:-}" ] && [ -d "$ANDROID_SDK_ROOT" ]; then
    MACBOT_ANDROID_SDK="$ANDROID_SDK_ROOT"
  elif [ -d "$HOME/Library/Android/sdk" ]; then
    MACBOT_ANDROID_SDK="$HOME/Library/Android/sdk"
  else return 1
  fi
  MACBOT_ANDROID_ADB="$MACBOT_ANDROID_SDK/platform-tools/adb"
  MACBOT_ANDROID_EMULATOR="$MACBOT_ANDROID_SDK/emulator/emulator"
  [ -x "$MACBOT_ANDROID_ADB" ] && [ -x "$MACBOT_ANDROID_EMULATOR" ]
}

macbot_android_avd_for_serial() {
  "$MACBOT_ANDROID_ADB" -s "$1" emu avd name 2>/dev/null | tr -d '\r' | sed -n '1p'
}

macbot_find_android_serial() {
  MACBOT_ANDROID_SERIAL=
  [ -x "$MACBOT_ANDROID_ADB" ] || return 1
  serials=$("$MACBOT_ANDROID_ADB" devices 2>/dev/null | awk '$1 ~ /^emulator-[0-9]+$/ && $2 == "device" { print $1 }')
  for serial in $serials; do
    if [ "$(macbot_android_avd_for_serial "$serial")" = "$MACBOT_AVD_NAME" ]; then
      MACBOT_ANDROID_SERIAL="$serial"; return 0
    fi
  done
  return 1
}

macbot_wait_for_android() {
  timeout_seconds=${1:-180}; elapsed=0
  while [ "$elapsed" -lt "$timeout_seconds" ]; do
    if macbot_find_android_serial; then
      boot_state=$("$MACBOT_ANDROID_ADB" -s "$MACBOT_ANDROID_SERIAL" shell getprop sys.boot_completed 2>/dev/null | tr -d '\r' | tail -n 1)
      [ "$boot_state" = "1" ] && return 0
    fi
    sleep 2; elapsed=$((elapsed + 2))
  done
  macbot_error "Android 模拟器 $MACBOT_AVD_NAME 启动或引导超时"; return 1
}

macbot_start_android() {
  macbot_find_android_sdk || { macbot_error "找不到 Android SDK、adb 或 emulator"; return 1; }
  "$MACBOT_ANDROID_ADB" start-server >/dev/null 2>&1 || return 1
  if macbot_find_android_serial; then
    macbot_log "复用 Android 模拟器 ${MACBOT_AVD_NAME}（${MACBOT_ANDROID_SERIAL}）"
    macbot_wait_for_android 180; return $?
  fi
  emulator_log="$MACBOT_LOG_DIR/emulator.log"; macbot_ensure_log_dir
  macbot_log "启动 Android 模拟器 $MACBOT_AVD_NAME"
  nohup "$MACBOT_ANDROID_EMULATOR" -avd "$MACBOT_AVD_NAME" </dev/null >> "$emulator_log" 2>&1 &
  macbot_wait_for_android 240
}

macbot_android_project_dir() {
  if [ -x "$MACBOT_SOURCE_DIR/gradlew" ] || [ -f "$MACBOT_SOURCE_DIR/settings.gradle" ] || [ -f "$MACBOT_SOURCE_DIR/settings.gradle.kts" ]; then
    printf '%s\n' "$MACBOT_SOURCE_DIR"
  elif [ -x "$MACBOT_SOURCE_DIR/clients/mobile/gradlew" ] || [ -f "$MACBOT_SOURCE_DIR/clients/mobile/settings.gradle" ] || [ -f "$MACBOT_SOURCE_DIR/clients/mobile/settings.gradle.kts" ]; then
    printf '%s\n' "$MACBOT_SOURCE_DIR/clients/mobile"
  fi
}

macbot_build_android() {
  project_dir=$(macbot_android_project_dir)
  [ -n "$project_dir" ] || { macbot_log "client-android：main 中没有 Gradle 工程，跳过"; return 2; }
  gradle="$project_dir/gradlew"
  [ -x "$gradle" ] || { macbot_error "Android 工程存在但缺少可执行 gradlew"; return 1; }
  macbot_find_android_sdk || { macbot_error "找不到 Android SDK"; return 1; }
  macbot_log "编译 Android APK：$gradle :androidApp:assembleDebug"
  (cd "$project_dir" && export ANDROID_HOME="$MACBOT_ANDROID_SDK" && \
    export JAVA_HOME="${JAVA_HOME:-$(/usr/libexec/java_home -v 21)}" && \
    "$gradle" --no-daemon :androidApp:assembleDebug) || {
    macbot_error "Android 编译失败"; return 1;
  }
  MACBOT_ANDROID_APK="$project_dir/androidApp/build/outputs/apk/debug/androidApp-debug.apk"
  [ -f "$MACBOT_ANDROID_APK" ] || {
    macbot_error "Android 编译完成但没有找到 $MACBOT_ANDROID_APK"; return 1;
  }
}

macbot_install_android() {
  macbot_start_android || return 1
  macbot_log "安装 Android APK 到 $MACBOT_ANDROID_SERIAL"
  "$MACBOT_ANDROID_ADB" -s "$MACBOT_ANDROID_SERIAL" install -r "$MACBOT_ANDROID_APK" >/dev/null || {
    macbot_error "Android APK 安装失败"; return 1;
  }
  printf '%s\n' "$MACBOT_MAIN_SHA" > "$MACBOT_CACHE_ROOT/android-installed-sha" || return 1
  "$MACBOT_ANDROID_ADB" -s "$MACBOT_ANDROID_SERIAL" shell monkey -p bot.mac.mobile -c android.intent.category.LAUNCHER 1 >/dev/null 2>&1 || \
    macbot_warn "无法自动打开 bot.mac.mobile；请在模拟器中手动启动 App"
}

macbot_plist_escape() {
  printf '%s' "$1" | sed 's/&/\&amp;/g; s/</\&lt;/g; s/>/\&gt;/g; s/"/\&quot;/g'
}

macbot_install_launch_agent() {
  label="$1"; binary="$2"; data_dir="$3"; out_log="$4"; err_log="$5"; shift 5
  plist="$HOME/Library/LaunchAgents/$label.plist"; uid=$(id -u)
  mkdir -p "$HOME/Library/LaunchAgents" "$data_dir" || return 1
  macbot_ensure_log_dir || return 1
  args_xml="<string>$(macbot_plist_escape "$binary")</string>"
  while [ "$#" -gt 0 ]; do
    args_xml="$args_xml
    <string>$(macbot_plist_escape "$1")</string>"; shift
  done
  data_xml=$(macbot_plist_escape "$data_dir")
  out_xml=$(macbot_plist_escape "$out_log")
  err_xml=$(macbot_plist_escape "$err_log")
  home_xml=$(macbot_plist_escape "$HOME")
  secret_backend_xml=
  if [ -f "$MACBOT_CACHE_ROOT/watch/file-secrets" ]; then
    secret_backend_xml='<key>MACBOT_SECRET_BACKEND</key><string>file</string>'
  fi
  printf '%s\n' \
    '<?xml version="1.0" encoding="UTF-8"?>' \
    '<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">' \
    '<plist version="1.0"><dict>' \
    "<key>Label</key><string>$label</string>" \
    '<key>ProgramArguments</key><array>' "$args_xml" '</array>' \
    '<key>EnvironmentVariables</key><dict>' \
    "<key>MACBOT_HOME</key><string>$data_xml</string>" \
    "$secret_backend_xml" \
    "<key>HOME</key><string>$home_xml</string>" '</dict>' \
    '<key>RunAtLoad</key><true/><key>KeepAlive</key><true/>' \
    "<key>StandardOutPath</key><string>$out_xml</string>" \
    "<key>StandardErrorPath</key><string>$err_xml</string>" \
    '</dict></plist>' > "$plist" || return 1
  chmod 600 "$plist" || return 1
  if launchctl print "gui/$uid/$label" >/dev/null 2>&1; then
    launchctl bootout "gui/$uid/$label" >/dev/null 2>&1 || true
  fi
  if ! launchctl bootstrap "gui/$uid" "$plist" >/dev/null 2>&1; then
    launchctl load -w "$plist" >/dev/null 2>&1 || {
      macbot_error "无法加载 LaunchAgent $label"; return 1;
    }
  fi
  launchctl kickstart -k "gui/$uid/$label" >/dev/null 2>&1 || {
    macbot_error "无法 kickstart LaunchAgent $label"; return 1;
  }
}

macbot_wait_health() {
  port="$1"; timeout_seconds=${2:-30}; elapsed=0
  if ! macbot_have curl; then macbot_warn "找不到 curl，跳过端口 $port 健康检查"; return 0; fi
  while [ "$elapsed" -lt "$timeout_seconds" ]; do
    if curl --fail --silent --show-error --max-time 2 "http://127.0.0.1:$port/api/v1/health" >/dev/null 2>&1; then
      macbot_log "端口 $port 健康检查通过"; return 0
    fi
    sleep 1; elapsed=$((elapsed + 1))
  done
  macbot_error "端口 $port 健康检查失败"; return 1
}

macbot_verify_service_pid() {
  label="$1"; port="$2"
  agent_pid=$(macbot_launchctl_pid "$label")
  listen_pid=$(macbot_port_pid "$port")
  if [ -z "$agent_pid" ] || [ -z "$listen_pid" ] || [ "$agent_pid" != "$listen_pid" ]; then
    macbot_error "LaunchAgent $label 与端口 $port 的进程不一致"
    return 1
  fi
}

macbot_launchctl_pid() {
  uid=$(id -u)
  launchctl print "gui/$uid/$1" 2>/dev/null | sed -n 's/^[[:space:]]*pid = \([0-9][0-9]*\)$/\1/p' | sed -n '1p'
}

macbot_port_pid() {
  [ -x /usr/sbin/lsof ] && /usr/sbin/lsof -nP -iTCP:"$1" -sTCP:LISTEN -t 2>/dev/null | sed -n '1p'
}

macbot_safe_log_tail() {
  path="$1"; lines=${2:-12}
  [ -f "$path" ] || { printf '(无日志)\n'; return 0; }
  tail -n "$lines" "$path" 2>/dev/null | sed -E \
    's/(--password|--api-key)[=[:space:]]+[^[:space:]]*/\1=<redacted>/g; s/(Bearer[[:space:]]+)[^[:space:]]*/\1<redacted>/gI; s/("(password|api_key|authorization)"[[:space:]]*:[[:space:]]*)"[^"]*"/\1"<redacted>"/gI; s/(password|api[_-]?key|authorization)([=:])[[:space:]]*[^,}[:space:]]*/\1\2<redacted>/gI'
}
