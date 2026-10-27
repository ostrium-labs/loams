#!/bin/sh
# Restore Headscale's state from a backup object into an EMPTY directory.
#
#   restore.sh OBJECT_NAME AGE_IDENTITY_FILE TARGET_DIR
#
# OBJECT_NAME is a file under backup:$BACKUP_BUCKET/$BACKUP_PREFIX/ (list them
# with `rclone ls`). AGE_IDENTITY_FILE is the offline private key. Run with
# Headscale stopped; then copy db.sqlite, noise_private.key and
# derp_server_private.key into the `lib` volume and the policy into
# deploy/tailnet/policy/ (docs/platform/deploy/headscale.md, "Recovery").
set -eu
[ $# -eq 3 ] || { echo "usage: restore.sh OBJECT_NAME AGE_IDENTITY_FILE TARGET_DIR" >&2; exit 2; }
obj=$1; ident=$2; target=$3
[ -d "$target" ] && [ -z "$(ls -A "$target")" ] || { echo "target must be an empty directory" >&2; exit 2; }
DEST="backup:${BACKUP_BUCKET}/${BACKUP_PREFIX}"
rclone copyto "$DEST/$obj" "/scratch/$obj" --s3-no-check-bucket
age -d -i "$ident" "/scratch/$obj" | tar -C "$target" -xf -
rm -f "/scratch/$obj"
[ "$(sqlite3 "$target/db.sqlite" 'PRAGMA integrity_check;')" = "ok" ] || { echo "restored database fails integrity_check" >&2; exit 1; }
echo "restored into $target:"; ls -l "$target"
