#!/bin/sh
# The `kafka` profile's startup guard (D1798): refuses to start a service of
# the profile while Iggy's root password is unset or still a placeholder from
# .env.example, then `exec`s the service's own command.
#
#   kafka-guard.sh <NAME OF THE PASSWORD VARIABLE> <command> [args...]
#
# Iggy writes its root user to its data volume on the first start, so a
# placeholder that got in once stays there: the guard has to run before the
# server ever starts. The password itself is never printed.
set -eu

if [ "$#" -lt 2 ]; then
  echo "kafka-guard: usage: kafka-guard.sh <PASSWORD_VARIABLE> <command> [args...]" >&2
  exit 64
fi

var="$1"
shift
case "$var" in
  '' | *[!A-Za-z0-9_]* | [0-9]*)
    echo "kafka-guard: '$var' is not a variable name" >&2
    exit 64
    ;;
esac
password="$(printenv "$var" || true)"

# The values .env.example has shipped with (keep in step with it), and empty.
case "$password" in
  '' | change-me | loams-kafka-preview)
    if [ -z "$password" ]; then why="is empty"; else why="is still the .env.example placeholder"; fi
    cat >&2 <<MSG
kafka profile: KAFKA_IGGY_PASSWORD $why; refusing to start.
Set a password of your own in .env (for example: openssl rand -hex 24) before
the first start: Iggy keeps its root user in the iggy-data volume.
MSG
    exit 78
    ;;
esac
if [ "${#password}" -lt 12 ]; then
  echo "kafka profile: KAFKA_IGGY_PASSWORD is shorter than 12 characters; refusing to start." >&2
  exit 78
fi

exec "$@"
