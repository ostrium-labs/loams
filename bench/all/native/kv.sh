#!/usr/bin/env bash
# The kv engine in-process: the loams-kv Store API on its embedded backend
# (an in-memory redb store, no fsync), 100,000 keys of 64 bytes. depth 1 =
# one-shot get/put transactions; depth N = one transaction of N keys
# (batch_get, or N puts). --target is unused (in-process).
# Read by common.sh:
# shellcheck disable=SC2034
ENGINE=kv DEFAULT_TARGET=in-process CONNS=${CONNS:-8}
build_args() { ARGS=(kv --backend embedded --keys "${KEYS:-100000}"); }
# shellcheck source-path=SCRIPTDIR source=common.sh
source "$(dirname "$0")/common.sh"
native_main "$@"
