#!/bin/zsh
set -euo pipefail

# Read-only update helper for the About/Settings UI. It never downloads or
# installs an update. The UI can parse the JSON response and decide whether to
# show an update action.

CURRENT_VERSION="${MACBOT_CURRENT_VERSION:-0.1.0}"
UPDATE_URL="${MACBOT_UPDATE_URL:-}"

if [[ -z "$UPDATE_URL" ]]; then
  print -r -- "{\"status\":\"disabled\",\"current_version\":\"$CURRENT_VERSION\"}"
  exit 0
fi

if [[ "$UPDATE_URL" != https://* \
   && "$UPDATE_URL" != http://localhost:* \
   && "$UPDATE_URL" != http://127.0.0.1:* \
   && "$UPDATE_URL" != http://\[::1\]:* ]]; then
  print -r -- "{\"status\":\"error\",\"current_version\":\"$CURRENT_VERSION\",\"message\":\"update URL must use https or local http\"}"
  exit 1
fi

response="$(/usr/bin/curl --fail --silent --show-error --location --max-time "${MACBOT_UPDATE_TIMEOUT:-10}" "$UPDATE_URL")" || {
  print -r -- "{\"status\":\"error\",\"current_version\":\"$CURRENT_VERSION\",\"message\":\"update check failed\"}"
  exit 1
}

# Validate JSON when jq is available (it is present on the development Mac).
# The helper still works on a clean Mac by passing the response through; the
# settings UI remains the final schema validator.
if command -v jq >/dev/null 2>&1 && ! print -r -- "$response" | jq -e . >/dev/null 2>&1; then
  print -r -- "{\"status\":\"error\",\"current_version\":\"$CURRENT_VERSION\",\"message\":\"update endpoint returned invalid JSON\"}"
  exit 1
fi

print -r -- "$response"
