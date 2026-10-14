#!/bin/sh
set -eu

if [ "$#" -lt 4 ] || [ "$#" -gt 5 ]; then
  echo "usage: render-launchagent.sh TEMPLATE DESTINATION APP_BINARY DEFAULT_HOME [MACBOT_HOME_OVERRIDE]" >&2
  exit 2
fi

TEMPLATE="$1"
DESTINATION="$2"
APP_BINARY="$3"
DEFAULT_HOME="$4"
MACBOT_HOME_OVERRIDE="${5:-}"

case "$APP_BINARY" in
  /*) ;;
  *) echo "daemon path must be absolute: $APP_BINARY" >&2; exit 1 ;;
esac

cp "$TEMPLATE" "$DESTINATION"
# Rebuild the array instead of replacing index 0: plutil treats a terminal
# numeric path component as an insertion point, leaving the template value in
# place as a second argument.
/usr/bin/plutil -replace ProgramArguments -array "$DESTINATION"
/usr/bin/plutil -insert ProgramArguments.0 -string "$APP_BINARY" "$DESTINATION"
/usr/bin/plutil -replace StandardOutPath -string "$DEFAULT_HOME/data/macbot.log" "$DESTINATION"
/usr/bin/plutil -replace StandardErrorPath -string "$DEFAULT_HOME/data/macbot.log" "$DESTINATION"

if [ -n "$MACBOT_HOME_OVERRIDE" ]; then
  /usr/bin/plutil -insert EnvironmentVariables.MACBOT_HOME \
    -string "$MACBOT_HOME_OVERRIDE" "$DESTINATION"
fi

/usr/bin/plutil -lint "$DESTINATION" >/dev/null
