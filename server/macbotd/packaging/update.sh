#!/bin/sh
set -eu
ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)"
git -C "$ROOT" pull --ff-only
"$ROOT/macbotd/packaging/install-launchagent.sh"
