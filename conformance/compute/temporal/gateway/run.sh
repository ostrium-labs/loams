#!/usr/bin/env bash
# The Temporal gateway's SDK legs (plan DX1 Task 10): the official Go and
# TypeScript SDKs, at the corpus's pins, run every DX1 scenario of the corpus
# against `loams-temporal`; each SDK's replayer must accept the histories the
# gateway wrote; and the histories must match the corpus after
# normalisation (compare.py).
#
#   run.sh                 # gateway on the in-process Resonate (dev mode)
#   LOAMS_RESONATE_BIN=… run.sh  # gateway on a real Resonate server (SQLite)
#
# Environment:
#   LOAMS_TEMPORAL_BIN  the gateway binary (default: cargo's target/debug/loams-temporal)
#   LOAMS_RESONATE_BIN        a Resonate server binary (the fork's `resonate`); unset = in-process
#   GW_PORT, RS_PORT    loopback ports (default 27233, 28001)
#   SCENARIOS           comma-separated (default: the corpus's DX1 scenarios)
# Everything listens on 127.0.0.1 only; the SDK containers use host networking.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=conformance/compute/lib.sh
. "$here/../../lib.sh"

corpus=$here/../corpus
out=$OC_OUT/temporal-gateway
rt=$(oc_container_cmd)
: "${GW_PORT:=27233}" "${RS_PORT:=28001}"
: "${SCENARIOS:=$(python3 -c 'import json,sys; print(",".join(s["name"] for s in json.load(open(sys.argv[1]))["scenarios"] if s["phase"]=="DX1"))' "$corpus/scenarios.json")}"
target=$(cargo metadata --format-version 1 --no-deps 2>/dev/null | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])' || echo "$OC_ROOT/target")
: "${LOAMS_TEMPORAL_BIN:=$target/debug/loams-temporal}"
junit=$out/temporal-gateway.junit.xml
mkdir -p "$out"
rm -rf "$out/histories" "$junit"
mkdir -p "$out/histories/go" "$out/histories/ts"
pids=()
cleanup() { for p in "${pids[@]}"; do kill "$p" 2>/dev/null || true; done; }
trap cleanup EXIT
case_() { python3 "$OC_ROOT/scripts/compute/junit.py" add "$junit" temporal-gateway "$1" "$2" "${3:-}"; }

# The gateway's own copy of the binary: parallel builds overwrite the target.
cp "$LOAMS_TEMPORAL_BIN" "$out/loams-temporal"
durability='kind = "in_process"'
if [ -n "${LOAMS_RESONATE_BIN:-}" ]; then
  cp "$LOAMS_RESONATE_BIN" "$out/resonate"
  rm -f "$out/resonate.db"
  RESONATE_SERVERS__ACTIVE=server_sqlite RESONATE_SERVERS__SERVER_SQLITE__PATH="$out/resonate.db" \
    RESONATE_GATEWAYS__GATEWAY_HTTP__BIND=127.0.0.1:$RS_PORT \
    RESONATE_GATEWAYS__GATEWAY_METRICS__BIND=127.0.0.1:$((RS_PORT + 1)) \
    env -u LOAMS_RESONATE_BIN "$out/resonate" serve >"$out/resonate.log" 2>&1 &
  pids+=($!)
  for _ in $(seq 50); do curl -fs "http://127.0.0.1:$RS_PORT/ready" >/dev/null && break; sleep 0.2; done
  durability="kind = \"resonate\"
url = \"http://127.0.0.1:$RS_PORT\""
fi
cat >"$out/gateway.toml" <<TOML
listen = "127.0.0.1:$GW_PORT"
insecure_dev = true
pid = "dx1-conformance"
dev_loams_namespace = "local"
[store]
kind = "memory"
[durability]
$durability
[[bootstrap_namespaces]]
loams_namespace_id = "local"
name = "default"
TOML
RUST_LOG=${GW_LOG:-info} "$out/loams-temporal" --config "$out/gateway.toml" >"$out/gateway.log" 2>&1 &
pids+=($!)
sleep 1

"$rt" volume create dx1-gocache >/dev/null 2>&1 || true
"$rt" volume create dx1-npmcache >/dev/null 2>&1 || true
go_run() {
  "$rt" run --rm --network host -v "$corpus:/c:ro" -v "$out:/out" -v dx1-gocache:/go/pkg \
    -e GOMAXPROCS=4 -e GOFLAGS=-p=4 -e CGO_ENABLED=0 -e GOCACHE=/go/pkg/build "$GO_IMAGE" \
    sh -ec "cp -r /c /tmp/c && cd /tmp/c/$1"
}
addr=127.0.0.1:$GW_PORT

if go_run "workers/go && go build -o /tmp/corpus . && /tmp/corpus generate -address $addr -namespace default -no-search-attribute -scenarios $SCENARIOS -out /out/histories/go" >"$out/go.log" 2>&1; then
  case_ gateway_dx1_scenarios_go pass
else
  case_ gateway_dx1_scenarios_go fail "$(tail -3 "$out/go.log")"
fi
if "$rt" run --rm --network host -v "$corpus:/c:ro" -v "$out:/out" -v dx1-npmcache:/root/.npm "$NODE_IMAGE" \
  sh -ec "cp -r /c/workers/ts /tmp/ts && cd /tmp/ts && npm ci --no-audit --no-fund >/dev/null && npx tsc -p . && node dist/generate.js $addr default /out/generate-ts.json $SCENARIOS" >"$out/ts.log" 2>&1 &&
  go_run "workers/go && go build -o /tmp/corpus . && /tmp/corpus export -address $addr -namespace default -no-search-attribute -prefix corpus-ts- -scenarios $SCENARIOS -out /out/histories/ts" >>"$out/ts.log" 2>&1; then
  case_ gateway_dx1_scenarios_ts pass
else
  case_ gateway_dx1_scenarios_ts fail "$(tail -3 "$out/ts.log")"
fi
for s in hello signal_then_complete; do
  for lang in go ts; do
    name=gateway_${s%%_*}_$lang
    if [ -s "$out/histories/$lang/$s.json" ] && grep -q EVENT_TYPE_WORKFLOW_EXECUTION_COMPLETED "$out/histories/$lang/$s.json"; then
      case_ "$name" pass
    else
      case_ "$name" fail "no completed $s history"
    fi
  done
done

# The SDKs' own replayers on the gateway's histories.
if go_run "replay/go && CORPUS_HISTORIES=/out/histories/go go test -count=1 ./..." >"$out/replay-go.log" 2>&1; then
  case_ gateway_histories_replay_go pass
else
  case_ gateway_histories_replay_go fail "$(tail -5 "$out/replay-go.log")"
fi
if "$rt" run --rm --network none -v "$corpus:/c:ro" -v "$out:/out:ro" -v dx1-npmcache:/root/.npm -e CORPUS_HISTORIES=/out/histories/ts "$NODE_IMAGE" \
  sh -ec "cp -r /c/workers/ts /tmp/ts && cd /tmp/ts && npm ci --offline --no-audit --no-fund >/dev/null && npx tsc -p . && node --test dist/replay.test.js" >"$out/replay-ts.log" 2>&1; then
  case_ gateway_histories_replay_ts pass
else
  case_ gateway_histories_replay_ts fail "$(tail -5 "$out/replay-ts.log")"
fi

# The differential against the corpus.
python3 -I "$here/compare.py" --corpus "$corpus/histories" --gateway "$out/histories" --scenarios "$SCENARIOS" --junit "$junit"

echo "temporal-gateway: results in $junit"
oc_xfail_check temporal/gateway "$junit"
