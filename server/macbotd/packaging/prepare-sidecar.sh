#!/bin/sh
set -eu
ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)"
DESTINATION="${1:?usage: prepare-sidecar.sh destination}"
VERSION=0.38.2
case "$(uname -m)" in
  arm64) ARCH=arm64; EXPECTED=8168b86ab5d94be8f670992dfe4fe1445016518a864b48bda105e64142e7cbf9 ;;
  x86_64) ARCH=x64; EXPECTED=787cb40e086a188d0bb13ff29a99a0b2380aff3aa5e8600b8f8131a0b98ca69c ;;
  *) echo "unsupported sidecar architecture" >&2; exit 1 ;;
esac
if [ -n "${MACBOT_BROWSER_BIN:-}" ]; then
  SOURCE="$MACBOT_BROWSER_BIN"
else
  SOURCE="$ROOT/target/sidecars/agent-browser-$VERSION-$ARCH"
  mkdir -p "$(dirname -- "$SOURCE")"
  if [ ! -f "$SOURCE" ]; then
    TEMPORARY="$(mktemp "$SOURCE.download.XXXXXX")"
    trap 'rm -f "$TEMPORARY"' EXIT INT TERM
    curl --fail --location --silent --show-error --connect-timeout 15 --max-time 180 \
      "https://github.com/vercel-labs/agent-browser/releases/download/v$VERSION/agent-browser-darwin-$ARCH" \
      -o "$TEMPORARY"
    ACTUAL="$(shasum -a 256 "$TEMPORARY" | awk '{print $1}')"
    [ "$ACTUAL" = "$EXPECTED" ] || { echo "sidecar checksum mismatch" >&2; exit 1; }
    mv "$TEMPORARY" "$SOURCE"
  fi
  ACTUAL="$(shasum -a 256 "$SOURCE" | awk '{print $1}')"
  [ "$ACTUAL" = "$EXPECTED" ] || { echo "sidecar cache checksum mismatch" >&2; exit 1; }
fi
mkdir -p "$(dirname -- "$DESTINATION")"
install -m 755 "$SOURCE" "$DESTINATION.new"
mv -f "$DESTINATION.new" "$DESTINATION"
