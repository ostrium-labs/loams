#!/usr/bin/env bash
# Resonate through its native HTTP protocol: promise.create over HTTP/2 (h2c),
# depth N streams in flight per connection, server on SQLite (engines.sh:
# 127.0.0.1:38101).
# Read by common.sh:
# shellcheck disable=SC2034
ENGINE=durable DEFAULT_TARGET=127.0.0.1:38101 CONNS=${CONNS:-8}
build_args() { ARGS=(http --addr "$1" --path / --kind resonate-create); }
# shellcheck source-path=SCRIPTDIR source=common.sh
source "$(dirname "$0")/common.sh"
native_main "$@"
