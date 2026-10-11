#!/bin/zsh
set -euo pipefail

SCRIPT_DIR="${0:A:h}"
MAC_ROOT="${SCRIPT_DIR:h}"
APP_ROOT="$MAC_ROOT/app"
BUNDLE_APP="$APP_ROOT/src-tauri/target/release/bundle/macos/Mac Bot.app"
DIST_DIR="$MAC_ROOT/dist"
APP_OUTPUT="$DIST_DIR/MacBot.app"
DMG_OUTPUT="$DIST_DIR/MacBot.dmg"
STAGE_DIR=""

cleanup() {
  if [[ -n "$STAGE_DIR" && -d "$STAGE_DIR" ]]; then
    rm -rf "$STAGE_DIR"
  fi
}
trap cleanup EXIT INT TERM

mkdir -p "$DIST_DIR"
cd "$APP_ROOT"
npm ci
if [[ -n "${CODESIGN_IDENTITY:-}" ]]; then
  export APPLE_SIGNING_IDENTITY="$CODESIGN_IDENTITY"
fi
npm run tauri -- build --bundles app

[[ -d "$BUNDLE_APP" ]] || {
  print -u2 "Tauri did not create the expected app bundle: $BUNDLE_APP"
  exit 1
}

if ! codesign --verify --deep --strict "$BUNDLE_APP" >/dev/null 2>&1; then
  [[ -z "${CODESIGN_IDENTITY:-}" ]] || {
    print -u2 "Tauri bundle signature verification failed for the requested identity."
    exit 1
  }
  codesign --force --deep --sign - --timestamp=none "$BUNDLE_APP"
fi
codesign --verify --deep --strict "$BUNDLE_APP"

STAGE_DIR="$(mktemp -d "$DIST_DIR/.MacBot-tauri-package.XXXXXX")"
ditto "$BUNDLE_APP" "$STAGE_DIR/MacBot.app"
ln -s /Applications "$STAGE_DIR/Applications"
rm -rf "$APP_OUTPUT"
ditto "$STAGE_DIR/MacBot.app" "$APP_OUTPUT"
codesign --verify --deep --strict "$APP_OUTPUT"
rm -f "$DMG_OUTPUT"
hdiutil create -quiet -volname "Mac Bot" -srcfolder "$STAGE_DIR" -ov -format UDZO "$DMG_OUTPUT"

print "Created $APP_OUTPUT"
print "Created $DMG_OUTPUT"
