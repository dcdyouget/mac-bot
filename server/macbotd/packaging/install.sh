#!/bin/sh
set -eu
ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)"
"$ROOT/macbotd/packaging/install-launchagent.sh"
