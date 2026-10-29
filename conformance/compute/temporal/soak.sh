#!/usr/bin/env bash
# The compat-mode soak (DX0 Task 3, step 2): soak/ (one workflow, one
# activity, at SOAK_RATE starts per second for SOAK_DURATION) against the
# stock server on Loams Postgres. Halfway through, the history service is
# restarted; at two thirds, Loams Postgres's compute is stopped for
# SOAK_COMPUTE_DOWN seconds and started again (a compute is stateless: its
# timeline stays on the pageserver, which is what a suspend does). Then the
# summary must show every started workflow completed.
#
#   soak.sh <suite> [smoke]
#
# `smoke` starts SMOKE_ITERATIONS (20) workflows with no faults. Writes
# $OC_OUT/<suite>-soak.json (or -smoke.json) and JUnit cases in
# $OC_OUT/<suite>.junit.xml.
set -euo pipefail
# shellcheck source=conformance/compute/lib.sh
. "$(dirname "$0")/../lib.sh"

suite=${1:?soak.sh <suite> [smoke]}
mode=${2:-soak}
rt=$(oc_container_cmd)
: "${SOAK_RATE:=20}" "${SOAK_DURATION:=30m}" "${SOAK_COMPUTE_DOWN:=30}" "${SMOKE_ITERATIONS:=20}"
: "${TEMPORAL_ADDRESS:=temporal-frontend:7233}"
here=$(cd "$(dirname "$0")" && pwd)
"$rt" volume create dx0-gocache >/dev/null 2>&1 || true

seconds() { # 30m, 90s, 2h -> seconds
  case $1 in *h) echo $((${1%h} * 3600)) ;; *m) echo $((${1%m} * 60)) ;; *s) echo "${1%s}" ;; *) echo "$1" ;; esac
}

run_soak() { # <out.json> <flags...>
  local out=$1
  shift
  "$rt" run --rm --network "$OC_NETWORK" -v "$here/soak:/soak:ro" -v dx0-gocache:/go/pkg \
    -v "$OC_OUT:/out" -e GOMAXPROCS=4 -e GOFLAGS=-p=4 -e CGO_ENABLED=0 -w /soak "$GO_IMAGE" \
    sh -ec "go build -o /tmp/soak . && /tmp/soak -address $TEMPORAL_ADDRESS -out /out/$(basename "$out") $*"
}

# The container of a compose service (the stack's project).
container_of() { oc_compose ps -q "$1" | head -n 1; }

if [ "$mode" = smoke ]; then
  out=$OC_OUT/$suite-smoke.json
  if run_soak "$out" -iterations "$SMOKE_ITERATIONS" -rate 5 -drain 3m -start-retry-for 1m -run-id "smoke$(date +%s)"; then
    oc_junit_case "$suite" temporal_smoke_hello pass
  else
    oc_junit_case "$suite" temporal_smoke_hello fail "$(cat "$out" 2>/dev/null || echo 'no summary')"
    exit 1
  fi
  exit 0
fi

out=$OC_OUT/$suite-soak.json
total=$(seconds "$SOAK_DURATION")
run_soak "$out" -rate "$SOAK_RATE" -duration "$SOAK_DURATION" -drain 10m -run-id "soak$(date +%s)" \
  >"$OC_OUT/$suite-soak.log" 2>&1 &
soak_pid=$!

history_ok=fail history_msg="not reached"
compute_ok=fail compute_msg="not reached"
sleep $((total / 2))
hist=$(container_of temporal-history)
if [ -n "$hist" ] && "$rt" restart "$hist" >/dev/null; then
  if oc_temporal_wait "$TEMPORAL_ADDRESS" 300; then history_ok=pass history_msg=""; else history_msg="the frontend did not recover"; fi
else
  history_msg="could not restart temporal-history"
fi
now=$((total / 2 + 0))
sleep $((total * 2 / 3 - now > 0 ? total * 2 / 3 - now : 0))
comp=$(container_of "${SOAK_COMPUTE_SERVICE:-compute1}")
if [ -n "$comp" ] && "$rt" stop -t 10 "$comp" >/dev/null; then
  sleep "$SOAK_COMPUTE_DOWN"
  "$rt" start "$comp" >/dev/null
  if oc_wait_pg "$(oc_pg_mode direct)" 300; then compute_ok=pass compute_msg=""; else compute_msg="Loams Postgres did not come back"; fi
else
  compute_msg="could not stop ${SOAK_COMPUTE_SERVICE:-compute1}"
fi

set +e
wait "$soak_pid"
rc=$?
set -e
summary=$(cat "$out" 2>/dev/null || echo '{}')
if [ "$rc" -eq 0 ]; then
  oc_junit_case "$suite" temporal_soak_no_failed_workflows pass
else
  oc_junit_case "$suite" temporal_soak_no_failed_workflows fail "$summary"
fi
# A recovery counts only when the run around it lost nothing.
[ "$rc" -eq 0 ] || { [ "$history_ok" = pass ] && history_msg="recovered, but the soak lost or failed workflows"; history_ok=fail; }
[ "$rc" -eq 0 ] || { [ "$compute_ok" = pass ] && compute_msg="recovered, but the soak lost or failed workflows"; compute_ok=fail; }
oc_junit_case "$suite" temporal_history_restart_recovers "$history_ok" "$history_msg"
oc_junit_case "$suite" temporal_compute_resume_recovers "$compute_ok" "$compute_msg"
echo "soak.sh: $summary"
