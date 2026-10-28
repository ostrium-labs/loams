#!/usr/bin/env bash
# The Temporal gateway's SDK legs (plan DX1 Task 10): the official Go and
# TypeScript SDKs, at the corpus's pins, run every DX1 scenario of the corpus
# against `loams-temporal`; each SDK's replayer must accept the histories the
# gateway wrote; and the histories must match the corpus after
# normalisation (compare.py).
#
#   run.sh                 # loopback: gateway on the in-process Resonate (dev mode)
#   LOAMS_RESONATE_BIN=… run.sh  # loopback: gateway on a real Resonate server (SQLite)
#   run.sh --profile       # the compose profile `temporal-gateway` (D2031)
#
# Loopback mode runs a built binary on 127.0.0.1 and the SDK containers use
# host networking. Profile mode brings up the profile of
# conformance/compute/temporal-gateway.yaml (oc_up: Loams Postgres, the
# engine, the `loams_temporal` database, and the gateway's image, built
# locally when missing) and runs the SDK containers on the stack's network
# against `loams:7233` with the profile's test API key; the gateway runs as
# deployed (Postgres store, the engine's Resonate, API keys). It leaves the
# stack up; `oc_compose --gateway down -v` removes it.
#
# Environment:
#   LOAMS_TEMPORAL_BIN  loopback: the gateway binary (default: cargo's target/debug/loams-temporal)
#   LOAMS_RESONATE_BIN  loopback: a Resonate server binary (the fork's `resonate`); unset = in-process
#   GW_PORT, RS_PORT    loopback: ports (default 27233, 28001)
#   GW_BUILD=1          profile: rebuild the gateway's image even when it exists
#   OC_PROJECT          profile: a compose project other than `loams` (lib.sh)
#   SCENARIOS           comma-separated (default: the corpus's DX1 scenarios)
#   TS_SCENARIO_TIMEOUT seconds one TypeScript scenario may run (default 240)
#   LEG_TIMEOUT         seconds one SDK container may run (default 900); a
#                       hung leg is killed and fails its case instead of the job
#   SDK_LEGS            comma-separated SDKs (default go,ts). Go and TypeScript
#                       are inline below; every other SDK is legs/<sdk>.sh,
#                       which defines leg_<sdk>_generate and leg_<sdk>_replay
#                       (D2138; the other SDK lanes add theirs the same way)
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=conformance/compute/lib.sh
. "$here/../../lib.sh"

corpus=$here/../corpus
rt=$(oc_container_cmd)
: "${GW_PORT:=27233}" "${RS_PORT:=28001}"
: "${SCENARIOS:=$(python3 -c 'import json,sys; print(",".join(s["name"] for s in json.load(open(sys.argv[1]))["scenarios"] if s["phase"]=="DX1"))' "$corpus/scenarios.json")}"
mode=loopback
case ${1:-} in
  --profile) mode=profile ;;
  '') ;;
  *) echo "usage: run.sh [--profile]" >&2; exit 2 ;;
esac
suffix=
[ "$mode" = profile ] && suffix=-profile
out=$OC_OUT/temporal-gateway$suffix
junit=$out/temporal-gateway.junit.xml
mkdir -p "$out"
rm -rf "$out/histories" "$junit"
: "${SDK_LEGS:=go,ts}"
sdks=${SDK_LEGS//,/ }
# want <sdk>: whether SDK_LEGS names it.
want() { [[ ",$SDK_LEGS," == *",$1,"* ]]; }
for l in $sdks; do
  mkdir -p "$out/histories/$l"
  case $l in
    go | ts) ;;
    *)
      [ -f "$here/legs/$l.sh" ] || { echo "run.sh: no legs/$l.sh for SDK_LEGS=$l" >&2; exit 2; }
      # shellcheck source=/dev/null
      . "$here/legs/$l.sh"
      ;;
  esac
done
pids=()
names=()
: "${LEG_TIMEOUT:=900}"
cleanup() {
  for p in "${pids[@]}"; do kill "$p" 2>/dev/null || true; done
  for n in "${names[@]}"; do "$rt" rm -f "$n" >/dev/null 2>&1 || true; done
  # Profile mode: the gateway's and the engine's logs as they are at the end,
  # whatever happened (the gateway's debug lines show a stuck run's inputs).
  if [ "$mode" = profile ]; then
    gw logs --no-color temporal-gateway >"$out/gateway.log" 2>&1 || true
    gw logs --no-color --tail 2000 loams >"$out/engine.log" 2>&1 || true
  fi
}
trap cleanup EXIT

# leg <name> <run args...>: one SDK container, killed after LEG_TIMEOUT
# seconds and removed either way, so a hung scenario fails its case (the
# log says which) instead of holding the job until its timeout.
leg() {
  local name=dx1-gw-$1-$$ rc=0
  shift
  names+=("$name")
  timeout --kill-after=30 "$LEG_TIMEOUT" "$rt" run --rm --name "$name" "$@" || rc=$?
  "$rt" rm -f "$name" >/dev/null 2>&1 || true
  [ "$rc" -ne 124 ] || echo "run.sh: $name killed after ${LEG_TIMEOUT}s"
  return "$rc"
}
case_() { python3 "$OC_ROOT/scripts/compute/junit.py" add "$junit" temporal-gateway "$1" "$2" "${3:-}"; }

if [ "$mode" = loopback ]; then
  target=$(cargo metadata --format-version 1 --no-deps 2>/dev/null | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])' || echo "$OC_ROOT/target")
  : "${LOAMS_TEMPORAL_BIN:=$target/debug/loams-temporal}"
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
  addr=127.0.0.1:$GW_PORT
  net=(--network host)
else
  gw() { oc_compose --gateway --profile temporal-gateway "$@"; }
  image=$(gw config --format json | python3 -c 'import json,sys; print(json.load(sys.stdin)["services"]["temporal-gateway"]["image"])')
  if [ -n "${GW_BUILD:-}" ] || ! "$rt" image inspect "$image" >/dev/null 2>&1; then
    CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-2} gw build temporal-gateway
  fi
  oc_up temporal-gateway
  serving=
  for _ in $(seq 100); do
    if gw logs temporal-gateway 2>&1 | grep -q 'serving the Temporal API'; then serving=1; break; fi
    sleep 3
  done
  gw logs temporal-gateway >"$out/gateway.log" 2>&1 || true
  if [ -n "$serving" ]; then
    case_ gateway_profile_serving pass
  else
    case_ gateway_profile_serving fail "$(tail -3 "$out/gateway.log")"
  fi
  # No host port: neither the gateway nor the namespace it shares (the
  # engine's) publishes one (the compose half is the self-test's
  # gateway_profile_no_host_ports).
  published=
  for id in $(gw ps -q temporal-gateway loams); do
    published+=$("$rt" port "$id" 2>/dev/null || true)
  done
  if [ -z "$published" ] && [ -n "$(gw ps -q temporal-gateway)" ]; then
    case_ gateway_profile_no_host_ports pass
  else
    case_ gateway_profile_no_host_ports fail "published: ${published:-no gateway container}"
  fi
  addr=loams:7233
  net=(--network "$OC_NETWORK" -e "TEMPORAL_API_KEY=${GW_API_KEY:-lk_temporal_conformance_test_only}")
fi

# go_run <name> <command in the corpus>: the pinned Go image on the corpus.
go_run() {
  local name=$1
  shift
  leg "$name" "${net[@]}" -v "$corpus:/c:ro" -v "$out:/out" -v dx1-gocache:/go/pkg \
    -e GOMAXPROCS=4 -e GOFLAGS=-p=4 -e CGO_ENABLED=0 -e GOCACHE=/go/pkg/build "$GO_IMAGE" \
    sh -ec "cp -r /c /tmp/c && cd /tmp/c/$1"
}

if want go; then
if go_run go-generate "workers/go && go build -o /tmp/corpus . && /tmp/corpus generate -address $addr -namespace default -no-search-attribute -scenarios $SCENARIOS -out /out/histories/go" >"$out/go.log" 2>&1; then
  case_ gateway_dx1_scenarios_go pass
else
  case_ gateway_dx1_scenarios_go fail "$(tail -3 "$out/go.log")"
fi
fi
if want ts; then
# TypeScript: one generator run per scenario, each killed after
# TS_SCENARIO_TIMEOUT seconds, so a scenario that hangs fails alone and the
# ones after it still run; then the histories of every run are exported,
# open runs included, so a hung run's history is kept for diagnosis.
ts_ok=1
if leg ts-generate "${net[@]}" -v "$corpus:/c:ro" -v "$out:/out" -v dx1-npmcache:/root/.npm \
  -e "TS_SCENARIO_TIMEOUT=${TS_SCENARIO_TIMEOUT:-240}" -e "SCENARIOS=$SCENARIOS" -e "ADDR=$addr" "$NODE_IMAGE" \
  sh -ec 'cp -r /c/workers/ts /tmp/ts && cd /tmp/ts && npm ci --no-audit --no-fund >/dev/null && npx tsc -p .
    rc=0
    for s in $(echo "$SCENARIOS" | tr , " "); do
      timeout -k 10 "$TS_SCENARIO_TIMEOUT" node dist/generate.js "$ADDR" default "/out/generate-ts-$s.json" "$s" ||
        { echo "corpus: $s: FAILED or killed after ${TS_SCENARIO_TIMEOUT}s"; rc=1; }
    done
    exit $rc' >"$out/ts.log" 2>&1; then :; else ts_ok=; fi
go_run ts-export "workers/go && go build -o /tmp/corpus . && /tmp/corpus export -address $addr -namespace default -no-search-attribute -prefix corpus-ts- -scenarios $SCENARIOS -out /out/histories/ts" >>"$out/ts.log" 2>&1 || ts_ok=
if [ -n "$ts_ok" ]; then
  case_ gateway_dx1_scenarios_ts pass
else
  case_ gateway_dx1_scenarios_ts fail "$(grep -E 'FAILED|killed|Error' "$out/ts.log" | tail -3)"
fi
fi
for l in $sdks; do
  case $l in
    go | ts) ;;
    *)
      if "leg_${l}_generate" >"$out/$l.log" 2>&1; then
        case_ "gateway_dx1_scenarios_$l" pass
      else
        case_ "gateway_dx1_scenarios_$l" fail "$(tail -3 "$out/$l.log")"
      fi
      ;;
  esac
done
for s in hello signal_then_complete; do
  for lang in $sdks; do
    name=gateway_${s%%_*}_$lang
    if [ -s "$out/histories/$lang/$s.json" ] && grep -q EVENT_TYPE_WORKFLOW_EXECUTION_COMPLETED "$out/histories/$lang/$s.json"; then
      case_ "$name" pass
    else
      case_ "$name" fail "no completed $s history"
    fi
  done
done

# The SDKs' own replayers on the gateway's histories.
if want go; then
if go_run go-replay "replay/go && CORPUS_HISTORIES=/out/histories/go go test -count=1 ./..." >"$out/replay-go.log" 2>&1; then
  case_ gateway_histories_replay_go pass
else
  case_ gateway_histories_replay_go fail "$(tail -5 "$out/replay-go.log")"
fi
fi
if want ts; then
if leg ts-replay --network none -v "$corpus:/c:ro" -v "$out:/out:ro" -v dx1-npmcache:/root/.npm -e CORPUS_HISTORIES=/out/histories/ts "$NODE_IMAGE" \
  sh -ec "cp -r /c/workers/ts /tmp/ts && cd /tmp/ts && npm ci --offline --no-audit --no-fund >/dev/null && npx tsc -p . && node --test dist/replay.test.js" >"$out/replay-ts.log" 2>&1; then
  case_ gateway_histories_replay_ts pass
else
  case_ gateway_histories_replay_ts fail "$(tail -5 "$out/replay-ts.log")"
fi
fi
for l in $sdks; do
  case $l in
    go | ts) ;;
    *)
      if "leg_${l}_replay" >"$out/replay-$l.log" 2>&1; then
        case_ "gateway_histories_replay_$l" pass
      else
        case_ "gateway_histories_replay_$l" fail "$(tail -5 "$out/replay-$l.log")"
      fi
      ;;
  esac
done

# The differential against the corpus.
python3 -I "$here/compare.py" --corpus "$corpus/histories" --gateway "$out/histories" --scenarios "$SCENARIOS" --langs "$SDK_LEGS" --junit "$junit"

echo "temporal-gateway ($mode): results in $junit"
oc_xfail_check temporal/gateway "$junit"
