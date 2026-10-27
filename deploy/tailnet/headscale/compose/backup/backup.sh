#!/bin/sh
# Nightly backup of Headscale's state to an S3-compatible bucket.
#
# One object per run: a tar of a consistent SQLite copy (.backup), the Noise
# and DERP private keys, and the policy, encrypted to the age recipient in
# BACKUP_AGE_RECIPIENT. The matching private key is kept offline, so a stolen
# bucket yields nothing. Pruned by age.
#
# Environment: BACKUP_AT (HH:MM UTC), BACKUP_RETENTION_DAYS, BACKUP_BUCKET,
# BACKUP_PREFIX, BACKUP_AGE_RECIPIENT, BACKUP_HEARTBEAT_URL and the
# RCLONE_CONFIG_BACKUP_* remote (see docker-compose.yml).
# Pass "now" as the first argument to run one backup immediately and exit.
set -eu

LIB=/var/lib/headscale
SCRATCH=/scratch
DEST="backup:${BACKUP_BUCKET}/${BACKUP_PREFIX}"

log() { printf '%s backup: %s\n' "$(date -u +%FT%TZ)" "$*"; }

run_backup() {
  # One run at a time: a scheduled run and `backup.sh now` must not share state.
  lock="$SCRATCH/lock"
  mkdir "$lock" 2>/dev/null || { log "another backup is running; skipping"; return 1; }
  ts=$(date -u +%Y%m%dT%H%M%SZ)
  stage="$SCRATCH/stage-$ts-$$"
  file="$SCRATCH/headscale-$ts.tar.age"
  cleanup() { rm -rf "$stage" "$file" "$lock"; }
  mkdir -p "$stage" || { cleanup; return 1; }
  log "copying $LIB/db.sqlite"
  sqlite3 "$LIB/db.sqlite" ".backup '$stage/db.sqlite'" || { log "sqlite .backup failed"; cleanup; return 1; }
  [ "$(sqlite3 "$stage/db.sqlite" 'PRAGMA integrity_check;')" = "ok" ] \
    || { log "integrity_check failed on the copy"; cleanup; return 1; }
  sqlite3 "$stage/db.sqlite" 'SELECT count(*) FROM users;' >/dev/null || { log "copy is not a Headscale database"; cleanup; return 1; }
  cp "$LIB"/noise_private.key "$LIB"/derp_server_private.key "$stage/" || { log "key copy failed"; cleanup; return 1; }
  cp /policy/policy.hujson "$stage/" || { log "policy copy failed"; cleanup; return 1; }
  # tar and age separately, so a tar failure cannot hide behind age's exit status.
  tar -C "$stage" -cf "$stage.tar" . || { log "tar failed"; rm -f "$stage.tar"; cleanup; return 1; }
  age -r "$BACKUP_AGE_RECIPIENT" -o "$file" "$stage.tar" || { log "age failed"; rm -f "$stage.tar"; cleanup; return 1; }
  rm -f "$stage.tar"
  size=$(wc -c <"$file")
  [ "$size" -gt 2000 ] || { log "archive suspiciously small ($size bytes)"; cleanup; return 1; }
  log "uploading ${size} bytes to $DEST/"
  rclone copyto "$file" "$DEST/headscale-$ts.tar.age" --s3-no-check-bucket \
    || { log "upload failed; keeping older backups, no heartbeat"; cleanup; return 1; }
  rm -f "$file"
  log "pruning objects older than ${BACKUP_RETENTION_DAYS}d"
  rclone delete "$DEST" --min-age "${BACKUP_RETENTION_DAYS}d" --s3-no-check-bucket || log "prune failed (ignored; the new backup is uploaded)"
  if [ -n "${BACKUP_HEARTBEAT_URL:-}" ]; then
    wget -q -O /dev/null "$BACKUP_HEARTBEAT_URL" || log "heartbeat ping failed (ignored)"
  fi
  cleanup
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

rm -rf "$SCRATCH/lock"   # a lock left by a killed run
log "scheduled daily at ${BACKUP_AT} UTC, retention ${BACKUP_RETENTION_DAYS}d, destination $DEST"
while :; do
  wait_s=$(seconds_until "$BACKUP_AT")
  while [ "$wait_s" -gt 0 ]; do
    touch "$SCRATCH/alive"
    step=60; [ "$wait_s" -lt 60 ] && step=$wait_s
    sleep "$step"
    wait_s=$((wait_s - step))
  done
  run_backup || log "BACKUP FAILED; will retry at the next slot"
  sleep 61
done
