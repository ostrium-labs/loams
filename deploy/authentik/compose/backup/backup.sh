#!/bin/sh
# Nightly pg_dump of the Authentik database to an S3-compatible bucket.
# Custom-format dumps (pg_restore), one object per run, pruned by age.
#
# Environment: PGHOST PGUSER PGDATABASE PGPASSWORD, BACKUP_AT (HH:MM UTC),
# BACKUP_RETENTION_DAYS, BACKUP_BUCKET, BACKUP_PREFIX, BACKUP_HEARTBEAT_URL,
# and the RCLONE_CONFIG_BACKUP_* remote (see docker-compose.yml).
# Pass "now" as the first argument to run one backup immediately and exit.
set -eu

SCRATCH=/scratch
DEST="backup:${BACKUP_BUCKET}/${BACKUP_PREFIX}"

log() { printf '%s backup: %s\n' "$(date -u +%FT%TZ)" "$*"; }

run_backup() {
  ts=$(date -u +%Y%m%dT%H%M%SZ)
  file="$SCRATCH/authentik-$ts.dump"
  log "dumping $PGDATABASE@$PGHOST to $file"
  pg_dump --format=custom --no-owner --no-privileges --file="$file"
  # A dump that pg_restore cannot list is not a backup.
  pg_restore --list "$file" >/dev/null
  size=$(wc -c <"$file")
  [ "$size" -gt 10000 ] || { log "dump suspiciously small ($size bytes)"; rm -f "$file"; return 1; }
  log "uploading ${size} bytes to $DEST/"
  rclone copyto "$file" "$DEST/authentik-$ts.dump" --s3-no-check-bucket
  rm -f "$file"
  log "pruning objects older than ${BACKUP_RETENTION_DAYS}d"
  rclone delete "$DEST" --min-age "${BACKUP_RETENTION_DAYS}d" --s3-no-check-bucket
  if [ -n "${BACKUP_HEARTBEAT_URL:-}" ]; then
    wget -q -O /dev/null "$BACKUP_HEARTBEAT_URL" || log "heartbeat ping failed (ignored)"
  fi
  log "done"
}

if [ "${1:-}" = "now" ]; then
  run_backup
  exit $?
fi

seconds_until() { # HH:MM UTC -> seconds until its next occurrence
  now=$(date -u +%s)
  today=$(date -u +%F)
  target=$(date -u -d "$today $1:00" +%s)
  [ "$target" -gt "$now" ] || target=$((target + 86400))
  echo $((target - now))
}

log "scheduled daily at ${BACKUP_AT} UTC, retention ${BACKUP_RETENTION_DAYS}d, destination $DEST"
while :; do
  wait_s=$(seconds_until "$BACKUP_AT")
  # Sleep in short slices so the health marker stays fresh.
  while [ "$wait_s" -gt 0 ]; do
    touch "$SCRATCH/alive"
    step=60; [ "$wait_s" -lt 60 ] && step=$wait_s
    sleep "$step"
    wait_s=$((wait_s - step))
  done
  run_backup || log "BACKUP FAILED; will retry at the next slot"
  sleep 61   # never run twice inside the same minute
done
