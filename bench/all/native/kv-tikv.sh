#!/usr/bin/env bash
# The kv engine's TiKV op: the loams-kv Store API on TiKV (keyspace bq1kv
# through PD at 127.0.0.1:39479, the TiKV engines.sh starts for sql). depth 1
# = one-shot get/put transactions; depth N = one transaction of N keys
# (batch_get, or N puts). Ops are reported as tikv.get and tikv.put.
# Read by common.sh:
# shellcheck disable=SC2034
ENGINE=kv DEFAULT_TARGET=127.0.0.1:39479 CONNS=${CONNS:-32}
build_args() { ARGS=(kv --backend tikv --pd "$1" --keyspace bq1kv --keys "${KEYS:-100000}"); }
rename_ops() { sed -e 's/"op":"get"/"op":"tikv.get"/' -e 's/"op":"put"/"op":"tikv.put"/'; }
# shellcheck source-path=SCRIPTDIR source=common.sh
source "$(dirname "$0")/common.sh"
native_main "$@"
