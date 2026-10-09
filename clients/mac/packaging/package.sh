#!/bin/zsh
set -euo pipefail

# Build and package the macOS desktop client. This script deliberately keeps all
# generated files under clients/mac/dist, which is ignored by clients/mac/.gitignore.

SCRIPT_DIR="${0:A:h}"
MAC_ROOT="${SCRIPT_DIR:h}"
DIST_DIR="$MAC_ROOT/dist"
APP_NAME="MacBot.app"
APP_DIR="$DIST_DIR/$APP_NAME"
BUNDLE_ID="bot.mac.desktop"
BINARY_NAME="macbot-desktop"
DMG_ROOT=""
DMG_TEMP=""

usage() {
  print "Usage: $0 [debug|release] [app|dmg|all]"
  print ""
  print "Environment:"
  print "  SKIP_BUILD=1             Reuse target output instead of running cargo build"
  print "  CODESIGN_IDENTITY=...    Sign the app with this codesign identity"
  print "  MACBOT_VERSION=...       Override CFBundle version (default: 0.1.0)"
  exit 2
}

CONFIG="${1:-release}"
ACTION="${2:-all}"
[[ "$CONFIG" == "debug" || "$CONFIG" == "release" ]] || usage
[[ "$ACTION" == "app" || "$ACTION" == "dmg" || "$ACTION" == "all" ]] || usage

if [[ "$CONFIG" == "release" ]]; then
  PROFILE_ARGS=(--release)
  PROFILE_DIR="release"
else
  PROFILE_ARGS=()
  PROFILE_DIR="debug"
fi

if [[ "${SKIP_BUILD:-0}" != "1" ]]; then
  if [[ ! -f "$MAC_ROOT/Cargo.toml" ]]; then
    print -u2 "clients/mac/Cargo.toml is missing; the desktop workspace has not been created yet."
    exit 1
  fi
  cargo build --manifest-path "$MAC_ROOT/Cargo.toml" -p "$BINARY_NAME" "${PROFILE_ARGS[@]}"
fi

BINARY="$MAC_ROOT/target/$PROFILE_DIR/$BINARY_NAME"
[[ -x "$BINARY" ]] || {
  print -u2 "Desktop binary not found or not executable: $BINARY"
  print -u2 "Build it first, or set SKIP_BUILD=0."
  exit 1
}

make_app() {
  local version="${MACBOT_VERSION:-0.1.0}"
  mkdir -p "$DIST_DIR"
  rm -rf "$APP_DIR"
  mkdir -p "$APP_DIR/Contents/MacOS" "$APP_DIR/Contents/Resources"
  cp "$BINARY" "$APP_DIR/Contents/MacOS/$BINARY_NAME"
  if [[ -d "$MAC_ROOT/../../protocol/fixtures" ]]; then
    cp -R "$MAC_ROOT/../../protocol/fixtures" "$APP_DIR/Contents/Resources/fixtures"
  fi
  chmod 755 "$APP_DIR/Contents/MacOS/$BINARY_NAME"
  git -C "$MAC_ROOT" rev-parse HEAD > "$APP_DIR/Contents/Resources/source-commit"
  if [[ -n "$(git -C "$MAC_ROOT" status --porcelain -- crates packaging Cargo.toml Cargo.lock)" ]]; then
    print "uncommitted-client-source" > "$APP_DIR/Contents/Resources/source-dirty"
  fi

  # Keep the source plist reviewable while allowing release automation to stamp
  # the version without changing files in the source tree.
  cp "$SCRIPT_DIR/Info.plist" "$APP_DIR/Contents/Info.plist"
  /usr/libexec/PlistBuddy -c "Set :CFBundleShortVersionString $version" "$APP_DIR/Contents/Info.plist"
  /usr/libexec/PlistBuddy -c "Set :CFBundleVersion $version" "$APP_DIR/Contents/Info.plist"

  if [[ -n "${CODESIGN_IDENTITY:-}" ]]; then
    codesign --force --sign "$CODESIGN_IDENTITY" --timestamp=none "$APP_DIR"
  elif [[ "${MACBOT_ADHOC_SIGN:-0}" == "1" ]]; then
    codesign --force --sign - --timestamp=none "$APP_DIR"
  fi

  print "Created $APP_DIR (bundle id: $BUNDLE_ID, configuration: $CONFIG)"
}

make_dmg() {
  [[ -d "$APP_DIR" ]] || {
    print -u2 "App bundle is missing: $APP_DIR"
    exit 1
  }
  local dmg="$DIST_DIR/MacBot.dmg"
  DMG_ROOT="$(mktemp -d "$DIST_DIR/.MacBot.dmg-root.XXXXXX")"
  cleanup_dmg_root() {
    if [[ -n "$DMG_ROOT" ]]; then
      rm -rf "$DMG_ROOT"
      DMG_ROOT=""
    fi
    if [[ -n "$DMG_TEMP" ]]; then
      rm -f "$DMG_TEMP"
      DMG_TEMP=""
    fi
  }
  trap cleanup_dmg_root EXIT
  cp -R "$APP_DIR" "$DMG_ROOT/MacBot.app"
  ln -s /Applications "$DMG_ROOT/Applications"
  DMG_TEMP="$DIST_DIR/.MacBot.$$.tmp.dmg"
  rm -f "$DMG_TEMP"
  hdiutil create -volname "Mac Bot" -srcfolder "$DMG_ROOT" -ov -format UDZO "$DMG_TEMP" >/dev/null
  mv -f "$DMG_TEMP" "$dmg"
  DMG_TEMP=""
  cleanup_dmg_root
  trap - EXIT
  print "Created $dmg"
}

if [[ "$ACTION" == "app" || "$ACTION" == "all" ]]; then
  make_app
fi
if [[ "$ACTION" == "dmg" ]]; then
  # A dmg request also packages the current binary, so it is self-contained.
  make_app
  make_dmg
elif [[ "$ACTION" == "all" ]]; then
  make_dmg
fi
