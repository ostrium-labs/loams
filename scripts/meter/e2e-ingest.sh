#!/usr/bin/env bash
# End-to-end check of the meter's service ingest on a local single node
# (ME1 follow-up (c), D2206): the node agent's real binary forwards cgroup
# usage through POST /internal/v1/meter/partitions/{p}/records, the counter
# owners read it off _system/_meter, and GetEntitlementValue shows it.
#
#   scripts/meter/e2e-ingest.sh [path/to/loams] [path/to/loams-meter-agent]
#
# The binaries default to the shared target's debug builds; build them with
#   cargo build -p loams --features meter --bin loams -p loams-meter-agent
# Needs curl and jq. Uses 127.0.0.1:${E2E_PORT:-18080} and a work directory
# it removes on exit (E2E_KEEP=1 keeps it).
set -euo pipefail

target="${CARGO_TARGET_DIR:-$(cargo metadata --format-version 1 --no-deps | jq -r .target_directory)}"
loams="${1:-$target/debug/loams}"
agent="${2:-$target/debug/loams-meter-agent}"
port="${E2E_PORT:-18080}"
base="http://127.0.0.1:$port"
work="$(mktemp -d "${E2E_DIR:-${TMPDIR:-/tmp}}/meter-e2e.XXXXXX")"
pid=""

cleanup() {
  [ -n "$pid" ] && kill "$pid" 2>/dev/null && wait "$pid" 2>/dev/null || true
  if [ "${E2E_KEEP:-0}" = 1 ]; then echo "kept $work"; else rm -rf "$work"; fi
}
trap cleanup EXIT

say() { echo "e2e: $*"; }
fail() { echo "e2e: FAIL: $*" >&2; [ -f "$work/loams.log" ] && tail -40 "$work/loams.log" >&2; exit 1; }

connect() { # method body
  curl -sS -X POST "$base/$1" -H 'content-type: application/json' -d "$2"
}

say "starting loams dev on $base"
"$loams" dev --data-dir "$work/data" --listen "127.0.0.1:$port" --no-flight-sql \
  >"$work/loams.log" 2>&1 &
pid=$!
for _ in $(seq 1 120); do
  curl -sf "$base/health" >/dev/null 2>&1 && break
  kill -0 "$pid" 2>/dev/null || fail "loams exited"
  sleep 0.5
done
curl -sf "$base/health" >/dev/null || fail "loams did not become healthy"

say "applying a plan: node CPU, metered, hard limit"
plan=$(cat <<'YAML'
plan: e2e
version: 1
meters:
  - slug: node_cpu_usec
    event_type: io.loams.dev.usage.node.cpu.v1
    aggregation: SUM
    value_property: $.cpu_usec
features:
  - key: node_cpu
    meter: node_cpu_usec
    unit: ms
entitlements:
  - feature: node_cpu
    metered: { period: { interval: month }, mode: hard_limit, issue_after_reset: 1000000 }
YAML
)
connect loams.meter.v1.PlanService/ApplyPlan "$(jq -n --arg y "$plan" '{yaml: $y, reason: "e2e"}')" \
  | jq -e '.plan.name == "e2e"' >/dev/null || fail "ApplyPlan"
customer=$(connect loams.meter.v1.CustomerService/CreateCustomer \
  '{"org": "acme", "displayName": "Acme", "subjects": ["org_acme"], "currency": "EUR"}' \
  | jq -r '.customer.id')
[ -n "$customer" ] && [ "$customer" != null ] || fail "CreateCustomer"
connect loams.meter.v1.PlanService/AssignPlan \
  "$(jq -n --arg c "$customer" '{customerId: $c, plan: "e2e", reason: "e2e"}')" \
  | jq -e '.customer.id' >/dev/null || fail "AssignPlan"

say "a fake cgroup for tenant acme and the agent's two scans"
cg="$work/cg/loams.slice/tenant-acme.slice"
mkdir -p "$cg"
cpu() {
  printf 'usage_usec %s\nuser_usec %s\nsystem_usec 0\n' "$1" "$1" >"$cg/cpu.stat"
  echo 0 >"$cg/memory.current"
  echo 0 >"$cg/memory.peak"
  printf 'populated 1\nfrozen 0\n' >"$cg/cgroup.events"
}
cat >"$work/tenants.json" <<'JSON'
{"rules": [{"prefix": "loams.slice/tenant-acme.slice", "org": "acme", "namespace": "prod"}]}
JSON
run_agent() {
  "$agent" --node e2e-node --cgroup-root "$work/cg" --spool-dir "$work/spool" \
    --tenants "$work/tenants.json" --stream-url "$base" --partitions 32 \
    --final-reads false --once >>"$work/agent.log" 2>&1
}
cpu 1000
run_agent || fail "agent (baseline): $(tail -5 "$work/agent.log")"
cpu 6000
# The meter stream is made when the role starts; retry until it is there.
for _ in $(seq 1 20); do
  run_agent && break
  sleep 1
done
grep -qi "error" "$work/agent.log" && say "agent log: $(grep -i error "$work/agent.log" | tail -2)"

say "waiting for the counters to see 5000 usec"
is5000() { [ -n "$1" ] && awk -v u="$1" 'BEGIN { exit !(u + 0 == 5000) }'; }
usage=""
for _ in $(seq 1 120); do
  usage=$(connect loams.meter.v1.EntitlementService/GetEntitlementValue \
    '{"subject": "org_acme", "feature": "node_cpu"}' | jq -r '.value.usage // empty')
  is5000 "$usage" && break
  sleep 0.5
done
is5000 "$usage" || fail "usage is ${usage:-unavailable}, want 5000"
say "PASS: the agent's usage reached the counters (usage $usage)"
