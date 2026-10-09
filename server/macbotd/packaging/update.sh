#!/bin/sh
set -eu
ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)"
git -C "$ROOT" pull --ff-only
MACBOT_BIN_DIR="${MACBOT_BIN_DIR:-$HOME/.local/bin}" \
  cargo build --release --manifest-path "$ROOT/Cargo.toml" -p macbotd
"$ROOT/macbotd/packaging/install-launchagent.sh"
