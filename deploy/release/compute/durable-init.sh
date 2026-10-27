#!/bin/sh
# The databases and roles of the compute profiles on Loams Postgres (DX1 DX0
# Task 2, OC1 OC0 Task 2). One-shot and idempotent: running it again changes
# nothing but the roles' passwords (so a password set in .env takes effect).
#
#   durable-init.sh roles            # in the psql image (compose: compute-init)
#   durable-init.sh temporal-schema  # in Temporal's admin-tools image (temporal-schema)
#
# `roles` creates, through the direct endpoint as the admin role:
#   - role temporal (CREATEDB: Temporal's own persistence suite creates and
#     drops test_* databases, DX1 R0.5) owning temporal and temporal_visibility;
#   - role airflow owning airflow;
#   - role loams_temporal owning loams_temporal: the Loams Temporal gateway's
#     history, mutable state and outbox (design 74 §5.5, profile
#     `temporal-gateway`; the gateway migrates its own schema at start);
#   - role postgres (CREATEDB: DBOS's suites log in as `postgres` and create
#     their own databases, R0.12) owning dbos_py, dbos_ts, dbos_go, dbos_java,
#     one system database per app (D1520).
# Each product has its own database, never a schema in a shared one (D1529),
# and PUBLIC may not connect to any of them.
#
# `temporal-schema` sets up and updates Temporal's schema in temporal and
# temporal_visibility with temporal-sql-tool from the pinned admin-tools image
# (plugin postgres12), to the newest version that image carries.
#
# Never Loams SQL: Postgres only (design 74 §5.5).
set -eu

PG_HOST=${LOAMS_PG_HOST:-compute1}
PG_PORT=${LOAMS_PG_PORT:-55433}
ADMIN_USER=${LOAMS_PG_ADMIN_USER:-cloud_admin}
ADMIN_PASSWORD=${LOAMS_PG_ADMIN_PASSWORD:-cloud_admin}
TEMPORAL_DB_USER=${TEMPORAL_DB_USER:-temporal}
TEMPORAL_DB_PASSWORD=${TEMPORAL_DB_PASSWORD:-temporal}
AIRFLOW_DB_USER=${AIRFLOW_DB_USER:-airflow}
AIRFLOW_DB_PASSWORD=${AIRFLOW_DB_PASSWORD:-airflow}
LOAMS_TEMPORAL_DB_USER=${LOAMS_TEMPORAL_DB_USER:-loams_temporal}
LOAMS_TEMPORAL_DB_PASSWORD=${LOAMS_TEMPORAL_DB_PASSWORD:-loams_temporal}
DBOS_DB_USER=${DBOS_DB_USER:-postgres}
DBOS_DB_PASSWORD=${DBOS_DB_PASSWORD:-dbos}
WAIT_SECONDS=${DURABLE_INIT_WAIT_SECONDS:-300}

log() { echo "durable-init: $*"; }

# A role or password must be a plain word: they are spliced into SQL below.
plain() {
  case $2 in
    '' | *[!A-Za-z0-9_.-]*) log "$1 must be letters, digits, _ . or - (got '$2')"; exit 2 ;;
  esac
}

roles() {
  for v in "$TEMPORAL_DB_USER" "$AIRFLOW_DB_USER" "$DBOS_DB_USER" "$LOAMS_TEMPORAL_DB_USER"; do plain role "$v"; done
  for v in "$TEMPORAL_DB_PASSWORD" "$AIRFLOW_DB_PASSWORD" "$DBOS_DB_PASSWORD" "$LOAMS_TEMPORAL_DB_PASSWORD"; do plain password "$v"; done
  export PGPASSWORD="$ADMIN_PASSWORD" PGCONNECT_TIMEOUT=5
  psql_admin() { psql -h "$PG_HOST" -p "$PG_PORT" -U "$ADMIN_USER" -d postgres -v ON_ERROR_STOP=1 -XAtq "$@"; }

  i=0
  until psql_admin -c 'SELECT 1' >/dev/null 2>&1; do
    i=$((i + 2))
    if [ "$i" -ge "$WAIT_SECONDS" ]; then
      log "Loams Postgres at $PG_HOST:$PG_PORT did not answer in ${WAIT_SECONDS}s"; exit 1
    fi
    sleep 2
  done
  log "Loams Postgres $(psql_admin -c 'SHOW server_version') at $PG_HOST:$PG_PORT"

  # role <name> <password> <attributes>
  role() {
    psql_admin <<SQL
DO \$\$
BEGIN
  IF NOT EXISTS (SELECT FROM pg_roles WHERE rolname = '$1') THEN
    CREATE ROLE "$1" LOGIN $3;
  END IF;
END
\$\$;
ALTER ROLE "$1" WITH LOGIN $3 PASSWORD '$2';
SQL
  }
  # database <name> <owner>
  database() {
    psql_admin <<SQL
SELECT format('CREATE DATABASE %I OWNER %I', '$1', '$2')
WHERE NOT EXISTS (SELECT FROM pg_database WHERE datname = '$1')
\gexec
ALTER DATABASE "$1" OWNER TO "$2";
REVOKE ALL ON DATABASE "$1" FROM PUBLIC;
GRANT ALL ON DATABASE "$1" TO "$2";
SQL
  }

  role "$TEMPORAL_DB_USER" "$TEMPORAL_DB_PASSWORD" CREATEDB
  role "$AIRFLOW_DB_USER" "$AIRFLOW_DB_PASSWORD" NOCREATEDB
  role "$DBOS_DB_USER" "$DBOS_DB_PASSWORD" CREATEDB
  role "$LOAMS_TEMPORAL_DB_USER" "$LOAMS_TEMPORAL_DB_PASSWORD" NOCREATEDB
  for db in temporal temporal_visibility; do database "$db" "$TEMPORAL_DB_USER"; done
  database airflow "$AIRFLOW_DB_USER"
  for db in dbos_py dbos_ts dbos_go dbos_java; do database "$db" "$DBOS_DB_USER"; done
  database loams_temporal "$LOAMS_TEMPORAL_DB_USER"
  log "roles and databases ready"
}

temporal_schema() {
  plain role "$TEMPORAL_DB_USER"
  schema=/etc/temporal/schema/postgresql/v12
  # sql <database> <command> [options]
  sql() {
    db=$1
    shift
    temporal-sql-tool --plugin postgres12 --ep "$PG_HOST" -p "$PG_PORT" \
      -u "$TEMPORAL_DB_USER" --pw "$TEMPORAL_DB_PASSWORD" --db "$db" "$@"
  }
  for pair in temporal:temporal temporal_visibility:visibility; do
    db=${pair%%:*}
    dir=${pair#*:}
    # The versioning tables. setup-schema fails on a database that already
    # has them; then the update below (which needs them) is the check that
    # the failure was only that, and any other failure still stops the run.
    sql "$db" setup-schema -v 0.0 >/dev/null 2>&1 || log "$db: versioning tables already there"
    sql "$db" update-schema -d "$schema/$dir/versioned"
    log "$db: schema up to date"
  done
}

case ${1:-} in
  roles) roles ;;
  temporal-schema) temporal_schema ;;
  *) echo "usage: durable-init.sh roles | temporal-schema" >&2; exit 2 ;;
esac
