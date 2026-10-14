#!/bin/sh
set -eu

if [ "$#" -ne 2 ]; then
  echo "usage: wait-for-admin.sh <admin-url> <timeout-seconds>" >&2
  exit 2
fi

ADMIN_URL="$1"
TIMEOUT_SECONDS="$2"
case "$TIMEOUT_SECONDS" in
  ''|*[!0-9]*)
    echo "invalid readiness timeout: $TIMEOUT_SECONDS" >&2
    exit 2
    ;;
esac
if [ "$TIMEOUT_SECONDS" -lt 1 ]; then
  echo "readiness timeout must be at least one second" >&2
  exit 2
fi

log_message() {
  MESSAGE="Mac Bot installer: $1"
  echo "$MESSAGE" >&2
  /usr/bin/logger -t MacBotInstaller "$1" 2>/dev/null || true
}

STARTED_AT=$(/bin/date +%s)
DELAY=0.25
LAST_STATUS=000

while :; do
  LAST_STATUS=$(/usr/bin/curl --silent --output /dev/null --write-out '%{http_code}' \
    --connect-timeout 1 --max-time 2 "$ADMIN_URL" 2>/dev/null) || LAST_STATUS=000
  case "$LAST_STATUS" in
    200|401)
      exit 0
      ;;
  esac

  NOW=$(/bin/date +%s)
  ELAPSED=$((NOW - STARTED_AT))
  if [ "$ELAPSED" -ge "$TIMEOUT_SECONDS" ]; then
    log_message "timed out after ${TIMEOUT_SECONDS}s waiting for $ADMIN_URL (last HTTP status: $LAST_STATUS); browser was not opened"
    exit 1
  fi

  /bin/sleep "$DELAY"
  case "$DELAY" in
    0.25) DELAY=0.5 ;;
    0.5) DELAY=1 ;;
    1) DELAY=2 ;;
    *) DELAY=2 ;;
  esac
done
