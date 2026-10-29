#!/usr/bin/env bash
# The Loams cases for every DBOS SDK (DX0 Task 5), sourced by dbos-<l>/run.sh
# after it defines:
#   L          the language tag (py, ts, go, java): case names are dbos_<L>_*
#   app        app <ENV=value...> -- <command> [args]: runs that language's
#              dbos-common/crash/<L> app in its runner (the same commands in
#              every language: setup, start, resume, count, enqueue, serve,
#              queued, set-event, get-event)
#   case_      case_ <name> <pass|fail> [message]
#   URL        postgresql://postgres:<password>@localhost:5432 (the runner's forwarder)
# Every case checks rows the app's steps wrote, or event timestamps, never
# an exit code alone.
# shellcheck shell=bash

# Each case's app output goes to $OC_OUT/dbos-<L>-<case>.log (the nightly
# uploads it), and a failure message carries its tail (DX1 ruling T5.R5).
caselog() { echo "$OC_OUT/dbos-$L-$1.log"; }
logtail() { tail -n 12 "$(caselog "$1")" 2>/dev/null | cut -c1-300; }

json_eq() { python3 -c 'import json,sys; sys.exit(0 if json.loads(sys.argv[1]) == json.loads(sys.argv[2]) else 1)' "$1" "$2" 2>/dev/null; }

# dbos_ensure_db <database>: create it on the direct endpoint if missing,
# owned by the suites' role (`postgres`, DX1 R0.12).
dbos_ensure_db() {
  local admin
  admin=$(oc_pg_mode direct)
  [ "$(oc_psql "$admin" "SELECT count(*) FROM pg_database WHERE datname = '$1'")" = 1 ] ||
    oc_psql "$admin" "CREATE DATABASE \"$1\" OWNER postgres"
}

# dbos_crash_case <name> <database> [ENV=value...]: SIGKILL when step 4 starts
# (steps 1-3 are checkpointed), restart, resume; each step's row exactly once.
dbos_crash_case() {
  local name=$1 db=$2 wf got
  shift 2
  wf="crash-$(date +%s%N)"
  # The polling case uses a fresh database: create it as the admin, owned by
  # the suites' role, before the app connects (its `setup` creates only the
  # table, and DBOS creates its system tables on launch). DX1 ruling T5.R2.
  if ! dbos_ensure_db "$db"; then case_ "$name" fail "could not create database $db"; return; fi
  local log
  log=$(caselog "$name")
  : >"$log"
  if ! app DBOS_URL="$URL/$db" "$@" -- setup >>"$log" 2>&1; then case_ "$name" fail "setup failed: $(logtail "$name")"; return; fi
  if app DBOS_URL="$URL/$db" CRASH_AT=4 "$@" -- start "$wf" >>"$log" 2>&1; then
    case_ "$name" fail "the app was not killed at step 4: $(logtail "$name")"; return
  fi
  if ! app DBOS_URL="$URL/$db" "$@" -- resume "$wf" >>"$log" 2>&1; then case_ "$name" fail "resume failed: $(logtail "$name")"; return; fi
  got=$(app DBOS_URL="$URL/$db" "$@" -- count "$wf" 2>>"$log" | tail -n 1) || true
  if json_eq "$got" '{"1": 1, "2": 1, "3": 1, "4": 1, "5": 1}'; then case_ "$name" pass
  else case_ "$name" fail "rows per step: ${got:-none} (want each of 1-5 once): $(logtail "$name")"; fi
}

# dbos_queue_case <name>: 1,000 workflows on queue `loams` (concurrency 10),
# dequeued by two processes; each runs exactly once.
dbos_queue_case() {
  local name=$1 prefix p1 p2 got log
  prefix="q$(date +%s)"
  log=$(caselog "$name")
  : >"$log"
  app DBOS_URL="$URL/dbos_$L" DBOS_EXECUTOR=e1 -- serve 180 >>"$log.e1" 2>&1 &
  p1=$!
  app DBOS_URL="$URL/dbos_$L" DBOS_EXECUTOR=e2 -- serve 180 >>"$log.e2" 2>&1 &
  p2=$!
  sleep 8
  app DBOS_URL="$URL/dbos_$L" DBOS_EXECUTOR=e3 -- enqueue "$prefix" 1000 >>"$log" 2>&1 || true
  wait "$p1" "$p2" || true
  cat "$log.e1" "$log.e2" >>"$log" 2>/dev/null || true
  rm -f "$log.e1" "$log.e2"
  got=$(app DBOS_URL="$URL/dbos_$L" -- queued "$prefix" 2>>"$log" | tail -n 1) || true
  if json_eq "$got" '{"workflows": 1000, "max_runs": 1}'; then case_ "$name" pass
  else case_ "$name" fail "${got:-none} (want 1000 workflows, each once): $(logtail "$name")"; fi
}

# dbos_notify_case <name> <max seconds> [hook]: a process waits for an event;
# another sets it (after the hook, if any); the waiter gets it within max.
dbos_notify_case() {
  local name=$1 max=$2 hook=${3:-} wf set got getter log
  wf="ev-$(date +%s%N)"
  log=$(caselog "$name")
  : >"$log"
  app DBOS_URL="$URL/dbos_$L" DBOS_EXECUTOR=g -- get-event "$wf" 240 >"$OC_OUT/dbos-$L-$name.get" 2>>"$log" &
  getter=$!
  sleep 8
  if [ -n "$hook" ] && ! "$hook" >>"$log" 2>&1; then
    wait "$getter" || true
    case_ "$name" fail "$hook failed: $(logtail "$name")"; return
  fi
  set=$(app DBOS_URL="$URL/dbos_$L" DBOS_EXECUTOR=s -- set-event "$wf" v1 5 2>>"$log" | grep set_at | head -n 1) || true
  wait "$getter" || true
  got=$(grep got_at "$OC_OUT/dbos-$L-$name.get" | tail -n 1) || true
  if python3 - "$set" "$got" "$max" <<'PY'; then case_ "$name" pass; else case_ "$name" fail "set ${set:-nothing}, got ${got:-nothing}, max ${max}s: $(logtail "$name")"; fi
import json, sys
if not sys.argv[1] or not sys.argv[2]:
    sys.exit(1)
s, g, m = json.loads(sys.argv[1]), json.loads(sys.argv[2]), float(sys.argv[3])
lat = g["got_at"] - s["set_at"]
print(f"latency {lat:.3f}s")
sys.exit(0 if g["value"] == "v1" and lat < m else 1)
PY
}

# The compute stops and starts while a process listens.
dbos_bounce_compute() {
  local c
  c=$(oc_compose ps -q compute1 | head -n 1)
  [ -n "$c" ] || { echo "no compute1 container"; return 1; }
  "$(oc_container_cmd)" stop -t 10 "$c" >/dev/null || return 1
  sleep 20
  "$(oc_container_cmd)" start "$c" >/dev/null || return 1
  # A compute that never answers again (#1185) fails this case, not the run.
  oc_wait_pg "$(oc_pg_mode direct)" 300 || { "$(oc_container_cmd)" logs --tail 30 "$c" 2>&1; return 1; }
  sleep 5
}

# dbos_cases <runner service> [polling]: every Loams case for language $L.
# `polling` adds dbos_<L>_polling_mode (the SDKs with a switch, R0.11).
dbos_cases() {
  local runner=$1 polling=${2:-}
  dbos_crash_case "dbos_${L}_crash_recovery_exactly_once" "dbos_$L"
  dbos_queue_case "dbos_${L}_queue_concurrency"
  dbos_notify_case "dbos_${L}_notify_latency" 1
  dbos_notify_case "dbos_${L}_notify_after_compute_resume" 30 dbos_bounce_compute
  if [ -n "$polling" ]; then
    dbos_crash_case "dbos_${L}_polling_mode" "dbos_${L}_poll_$(date +%s)" DBOS_LISTEN=0
  fi
  # Through PgDog transaction pooling: the forwarder pointed at pgdog-txn. A
  # failure to bring that up fails this case, not the run (the first local
  # run stopped here; DX1 ruling T5.R10).
  local txn="dbos_${L}_txn_mode_xfail" tlog
  tlog=$(caselog "$txn.setup")
  if oc_compose --txn --profile dbos up -d pgdog-txn >"$tlog" 2>&1 &&
    DBOS_PG_TARGET=pgdog-txn:6432 oc_compose --txn --profile dbos up -d --no-deps --force-recreate dbos-pg >>"$tlog" 2>&1 &&
    oc_compose --txn --profile dbos up -d --no-deps --force-recreate "$runner" >>"$tlog" 2>&1; then
    sleep 3
    dbos_crash_case "$txn" "dbos_$L"
  else
    case_ "$txn" fail "could not start PgDog for the transaction-mode leg: $(tail -n 8 "$tlog" | cut -c1-300)"
  fi
  oc_compose --txn --profile dbos rm -s -f pgdog-txn >>"$tlog" 2>&1 || true
  oc_compose --profile dbos up -d --no-deps --force-recreate dbos-pg "$runner" >>"$tlog" 2>&1 || true
}
