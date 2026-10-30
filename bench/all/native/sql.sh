#!/usr/bin/env bash
# Loams SQL through MySQL wire to keyspace-mode TiDB (engines.sh: 127.0.0.1:44100).
# depth N = N ids per point lookup (WHERE id IN (...)): the classic MySQL
# protocol has no pipelining. 100,000 rows (bq1.sbtest1), loaded on first use.
# Read by common.sh:
# shellcheck disable=SC2034
ENGINE=sql DEFAULT_TARGET=127.0.0.1:44100 CONNS=${CONNS:-32}
build_args() { ARGS=(mysql --url "mysql://root@$1/" --rows "${ROWS:-100000}"); }
# shellcheck source-path=SCRIPTDIR source=common.sh
source "$(dirname "$0")/common.sh"
native_main "$@"
