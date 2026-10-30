#!/usr/bin/env bash
# Postgres through PG wire (tokio-postgres) to the compute engines.sh starts
# (127.0.0.1:55701). depth N = N queries in flight on each connection
# (tokio-postgres pipelines them). 100,000 rows (bq1_accounts).
# Read by common.sh:
# shellcheck disable=SC2034
ENGINE=pg DEFAULT_TARGET=127.0.0.1:55701 CONNS=${CONNS:-16}
build_args() {
  ARGS=(pg --url "host=${1%:*} port=${1##*:} user=postgres password=bq1 dbname=postgres" --rows "${ROWS:-100000}")
}
# shellcheck source-path=SCRIPTDIR source=common.sh
source "$(dirname "$0")/common.sh"
native_main "$@"
