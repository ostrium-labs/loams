#!/usr/bin/env bash
# loams-gateway's HTTP listener over HTTP/2 (h2c), depth N streams in flight
# per connection: GET /healthz on the admin listener (engines.sh:
# 127.0.0.1:19387). The gateway does not front the Loams Connect API yet
# (bench/gateway/native-load.sh says the same), so this is its HTTP stack.
# Read by common.sh:
# shellcheck disable=SC2034
ENGINE=gateway DEFAULT_TARGET=127.0.0.1:19387 CONNS=${CONNS:-16}
build_args() { ARGS=(http --addr "$1" --path /healthz --kind get); }
rename_ops() { sed 's/"op":"get"/"op":"http2.get"/'; }
# shellcheck source-path=SCRIPTDIR source=common.sh
source "$(dirname "$0")/common.sh"
native_main "$@"
