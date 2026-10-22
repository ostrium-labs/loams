#!/bin/sh
# Restore drill and real restore for the Authentik database.
#
#   restore.sh list                         list backups in the bucket
#   restore.sh verify [latest|<object>]     restore into a scratch database,
#                                           check it, drop it (safe drill)
#   restore.sh apply  [latest|<object>]     restore over $PGDATABASE (DESTRUCTIVE;
#                                           stop server and worker first)
set -eu

DEST="backup:${BACKUP_BUCKET}/${BACKUP_PREFIX}"
cmd=${1:-}
obj=${2:-latest}

pick() {
  if [ "$obj" = latest ]; then
    obj=$(rclone lsf "$DEST" --s3-no-check-bucket | sort | tail -n 1)
    [ -n "$obj" ] || { echo "no backups found in $DEST" >&2; exit 1; }
  fi
  echo "using $obj" >&2
  rclone copyto "$DEST/$obj" "/scratch/$obj" --s3-no-check-bucket
  pg_restore --list "/scratch/$obj" >/dev/null
}

case "$cmd" in
  list)
    rclone lsl "$DEST" --s3-no-check-bucket
    ;;
  verify)
    pick
    scratch_db=authentik_restore_drill
    psql -d postgres -v ON_ERROR_STOP=1 -c "DROP DATABASE IF EXISTS $scratch_db" -c "CREATE DATABASE $scratch_db"
    pg_restore --no-owner --no-privileges --dbname="$scratch_db" "/scratch/$obj"
    users=$(psql -d "$scratch_db" -Atc "SELECT count(*) FROM authentik_core_user")
    apps=$(psql -d "$scratch_db" -Atc "SELECT count(*) FROM authentik_core_application")
    echo "restored $obj: $users users, $apps applications"
    psql -d postgres -c "DROP DATABASE $scratch_db"
    rm -f "/scratch/$obj"
    [ "$users" -ge 1 ] || { echo "DRILL FAILED: no users in restored database" >&2; exit 1; }
    echo "DRILL OK"
    ;;
  apply)
    pick
    echo "restoring $obj over $PGDATABASE (server and worker must be stopped)" >&2
    pg_restore --clean --if-exists --no-owner --no-privileges --dbname="$PGDATABASE" "/scratch/$obj"
    rm -f "/scratch/$obj"
    echo "restore complete; start server and worker"
    ;;
  *)
    sed -n '2,10p' "$0" >&2
    exit 2
    ;;
esac
