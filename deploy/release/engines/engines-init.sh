#!/bin/sh
# One-shot setup for the managed engines on the compose stack (design 81,
# ENG81 R6, R7). Idempotent: a rerun only resets the roles' passwords, which
# is how a password set in .env takes effect.
#
#   engines-init.sh buckets      # in an image with curl (compose: engines-buckets)
#   engines-init.sh databases    # in the psql image (compose: engines-init)
#   engines-init.sh warehouse    # in an image with curl, once Lakekeeper is up
#                                # (compose: lakekeeper-init)
#
# `buckets` creates `loams-engines` (engine state under engines/<ns>/<id>/)
# and `loams-lake` (the Lakekeeper warehouse `loams`) on RustFS. That needs
# the `rustfs` profile, which every engine profile requires.
# `databases` creates, as the admin role on the direct endpoint (compute1):
#   - role and database `lakekeeper`, Lakekeeper's catalog;
#   - role and database `rw_<key>` for the compose RisingWave instance's
#     meta store (D1951; RISINGWAVE_META_DB, default rw_default).
# `warehouse` bootstraps Lakekeeper and creates the warehouse `loams` on
# `loams-lake` (ENG81 R9).
#
# Each product gets its own database and role. PUBLIC may not connect to any of them.
set -eu

PG_HOST=${LOAMS_PG_HOST:-compute1}
PG_PORT=${LOAMS_PG_PORT:-55433}
ADMIN_USER=${LOAMS_PG_ADMIN_USER:-cloud_admin}
ADMIN_PASSWORD=${LOAMS_PG_ADMIN_PASSWORD:-cloud_admin}
LAKEKEEPER_DB_PASSWORD=${LAKEKEEPER_DB_PASSWORD:-lakekeeper}
RW_META_DB=${RISINGWAVE_META_DB:-rw_default}
RW_META_PASSWORD=${RISINGWAVE_META_PASSWORD:-risingwave}
S3_ENDPOINT=${ENGINES_S3_ENDPOINT:-http://rustfs:9000}
S3_REGION=${AWS_REGION:-us-east-1}
LAKEKEEPER_URL=${LAKEKEEPER_URL:-http://lakekeeper:8181}
WAIT_SECONDS=${ENGINES_INIT_WAIT_SECONDS:-300}

log() { echo "engines-init: $*"; }

# Names and passwords are spliced into SQL and URLs: plain words only.
# RisingWave splices its meta password into a URL unencoded, so it must be
# [A-Za-z0-9] (D1951).
plain() {
  case $2 in
    '' | *[!A-Za-z0-9_]*) log "$1 must be letters, digits or _ (got '$2')"; exit 2 ;;
  esac
}
alnum() {
  case $2 in
    '' | *[!A-Za-z0-9]*) log "$1 must be letters and digits only"; exit 2 ;;
  esac
}

wait_for() {
  what=$1; shift
  i=0
  until "$@" >/dev/null 2>&1; do
    i=$((i + 2))
    if [ "$i" -ge "$WAIT_SECONDS" ]; then
      log "$what did not answer in ${WAIT_SECONDS}s"; exit 1
    fi
    sleep 2
  done
}

# RustFS requires x-amz-content-sha256; curl's --aws-sigv4 does not add
# it, so it is set to the empty-body hash (as deploy/loams-postgres-dev does).
EMPTY_SHA=e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
s3() {
  curl -sf -o /dev/null -H "x-amz-content-sha256: $EMPTY_SHA" \
    --aws-sigv4 "aws:amz:$S3_REGION:s3" --user "$AWS_ACCESS_KEY_ID:$AWS_SECRET_ACCESS_KEY" "$@"
}

bucket() {
  name=$1
  if s3 -I "$S3_ENDPOINT/$name"; then
    log "bucket $name exists"
  else
    s3 -X PUT "$S3_ENDPOINT/$name"
    log "bucket $name created"
  fi
}

buckets() {
  wait_for "RustFS at $S3_ENDPOINT" curl -s -o /dev/null "$S3_ENDPOINT/"
  bucket loams-engines
  bucket loams-lake
}

databases() {
  plain "RISINGWAVE_META_DB" "$RW_META_DB"
  alnum "RISINGWAVE_META_PASSWORD" "$RW_META_PASSWORD"
  alnum "LAKEKEEPER_DB_PASSWORD" "$LAKEKEEPER_DB_PASSWORD"
  export PGPASSWORD="$ADMIN_PASSWORD" PGCONNECT_TIMEOUT=5
  psql_admin() { psql -h "$PG_HOST" -p "$PG_PORT" -U "$ADMIN_USER" -d postgres -v ON_ERROR_STOP=1 -XAtq "$@"; }
  wait_for "Loams Postgres at $PG_HOST:$PG_PORT" psql_admin -c 'SELECT 1'

  for pair in "lakekeeper:$LAKEKEEPER_DB_PASSWORD" "$RW_META_DB:$RW_META_PASSWORD"; do
    name=${pair%%:*}
    password=${pair#*:}
    psql_admin <<SQL
DO \$\$ BEGIN
  IF NOT EXISTS (SELECT 1 FROM pg_catalog.pg_roles WHERE rolname = '$name') THEN
    CREATE ROLE "$name" LOGIN PASSWORD '$password';
  ELSE
    ALTER ROLE "$name" LOGIN PASSWORD '$password';
  END IF;
END \$\$;
SQL
    if [ -z "$(psql_admin -c "SELECT 1 FROM pg_catalog.pg_database WHERE datname = '$name'")" ]; then
      psql_admin -c "CREATE DATABASE \"$name\" OWNER \"$name\""
      log "database $name created"
    fi
    psql_admin -c "REVOKE CONNECT ON DATABASE \"$name\" FROM PUBLIC"
  done
  log "databases ready"
}

warehouse() {
  wait_for "Lakekeeper at $LAKEKEEPER_URL" curl -sf "$LAKEKEEPER_URL/health"
  # Bootstrap once; a second call answers 400 or 409 ("already bootstrapped").
  code=$(curl -s -o /dev/null -w '%{http_code}' -X POST "$LAKEKEEPER_URL/management/v1/bootstrap" \
    -H 'Content-Type: application/json' --data '{"accept-terms-of-use": true}')
  log "bootstrap: HTTP $code"
  if curl -sf "$LAKEKEEPER_URL/management/v1/warehouse" | grep -q '"name":"loams"'; then
    log "warehouse loams exists"
    return
  fi
  curl -sf -o /dev/null -X POST "$LAKEKEEPER_URL/management/v1/warehouse" \
    -H 'Content-Type: application/json' --data @- <<JSON
{
  "warehouse-name": "loams",
  "storage-profile": {
    "type": "s3",
    "bucket": "loams-lake",
    "key-prefix": "warehouse",
    "endpoint": "$S3_ENDPOINT",
    "region": "$S3_REGION",
    "path-style-access": true,
    "flavor": "s3-compat",
    "sts-enabled": false
  },
  "storage-credential": {
    "type": "s3",
    "credential-type": "access-key",
    "access-key-id": "$AWS_ACCESS_KEY_ID",
    "secret-access-key": "$AWS_SECRET_ACCESS_KEY"
  }
}
JSON
  log "warehouse loams created"
}

case ${1:-} in
  buckets) buckets ;;
  databases) databases ;;
  warehouse) warehouse ;;
  *) echo "usage: $0 buckets|databases|warehouse" >&2; exit 2 ;;
esac
