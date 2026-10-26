#!/usr/bin/env bash
# The nightly Confluent smoke (plan CC1 Task 10, design 80 §11), run by
# scripts/byoc/confluent-ci.sh smoke against the run's own cluster with Loams'
# data key (the D1826 grant set):
#
#   1. the data key produces to the module's topics (Kafka REST v3), 50 records,
#      and the produce latency p50/p99 from this host is printed;
#   2. the data key is refused what D1826 forbids: create a topic, raise
#      retention.bytes, add partitions, delete a topic, create an ACL, write a
#      topic it was not granted;
#   3. the engine round trip (Postgres -> stream -> transactional Kafka sink ->
#      topic -> Kafka source -> collection) runs when CONFLUENT_SMOKE_ENGINE is
#      set and the release has the connect role (CC1 Task 1); otherwise it says
#      it was skipped.
#
# Environment: CONFLUENT_SMOKE_REST (https://pkc-...:443), CONFLUENT_SMOKE_CLUSTER
# (lkc-...), CONFLUENT_SMOKE_KEY_FILE (lines id=... and secret=..., mode 0600),
# CONFLUENT_SMOKE_TOPIC, CONFLUENT_SMOKE_CHECKPOINTS, CONFLUENT_SMOKE_RECORDS
# (default 50), CONFLUENT_SMOKE_ENGINE (optional). Prints no secret.
# shellcheck disable=SC2015 # A && B || C reports a failed check
set -euo pipefail
rest="${CONFLUENT_SMOKE_REST:?}"; cluster="${CONFLUENT_SMOKE_CLUSTER:?}"
keyfile="${CONFLUENT_SMOKE_KEY_FILE:?}"; topic="${CONFLUENT_SMOKE_TOPIC:?}"
checkpoints="${CONFLUENT_SMOKE_CHECKPOINTS:-__loams_checkpoints}"
records="${CONFLUENT_SMOKE_RECORDS:-50}"
base="$rest/kafka/v3/clusters/$cluster"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
(umask 077; printf 'user = "%s:%s"\n' "$(sed -n 's/^id=//p' "$keyfile")" "$(sed -n 's/^secret=//p' "$keyfile")" >"$work/curl.cfg")
fail=0
ok() { echo "ok   $*"; }
bad() { echo "FAIL $*"; fail=1; }

# call METHOD URL [body] -> prints "status seconds"
call() {
  local args=(-sS -K "$work/curl.cfg" -X "$1" -H 'Content-Type: application/json' --max-time 30 -o /dev/null -w '%{http_code} %{time_total}\n')
  [[ $# -lt 3 ]] || args+=(--data "$3")
  curl "${args[@]}" "$2" || echo "000 0"
}

# A new key can take a minute to be accepted everywhere.
for _ in $(seq 12); do
  read -r code _ < <(call POST "$base/topics/$topic/records" '{"key":{"type":"STRING","data":"warmup"},"value":{"type":"JSON","data":{"n":-1}}}')
  [[ "$code" == 200 ]] && break
  sleep 10
done
[[ "$code" == 200 ]] && ok "the data key produces to $topic" || bad "produce to $topic answered HTTP $code"

: >"$work/lat"
errors=0
for i in $(seq 1 "$records"); do
  read -r code t < <(call POST "$base/topics/$topic/records" "{\"key\":{\"type\":\"STRING\",\"data\":\"k$((i % 10))\"},\"value\":{\"type\":\"JSON\",\"data\":{\"n\":$i}}}")
  if [[ "$code" == 200 ]]; then echo "$t" >>"$work/lat"; else errors=$((errors + 1)); fi
done
((errors == 0)) && ok "$records records produced" || bad "$errors of $records produces failed"
if [[ -s "$work/lat" ]]; then
  sort -n "$work/lat" | awk '{a[NR]=$1} END {p50=a[int((NR+1)*0.50)]; p99=a[int(NR*0.99) > 0 ? int(NR*0.99) : 1];
    printf "info produce latency over Kafka REST from this host (%d records): p50 %.0f ms, p99 %.0f ms\n", NR, p50*1000, p99*1000}'
fi
read -r code _ < <(call POST "$base/topics/$checkpoints/records" '{"key":{"type":"STRING","data":"smoke"},"value":{"type":"JSON","data":{"checkpoint":0}}}')
[[ "$code" == 200 ]] && ok "the data key writes the checkpoint topic" || bad "checkpoint topic answered HTTP $code"

# What the data key must not do (D1826).
refused() {  # what, then call args
  local what="$1"; shift
  read -r code _ < <(call "$@")
  [[ "$code" == 403 || "$code" == 401 ]] && ok "refused: $what" || bad "the data key could $what (HTTP $code)"
}
refused "create a topic" POST "$base/topics" "{\"topic_name\":\"$topic-smoke-new\",\"partitions_count\":1}"
refused "raise retention.bytes" PUT "$base/topics/$topic/configs/retention.bytes" '{"value":"1073741824"}'
refused "add partitions" PATCH "$base/topics/$topic" '{"partitions_count":3}'
refused "delete a topic" DELETE "$base/topics/$topic"
refused "create an ACL" POST "$base/acls" '{"resource_type":"TOPIC","resource_name":"*","pattern_type":"LITERAL","principal":"User:*","host":"*","operation":"ALL","permission":"ALLOW"}'
refused "write a topic it was not granted" POST "$base/topics/loams-smoke-not-granted/records" '{"value":{"type":"JSON","data":1}}'

if [[ -n "${CONFLUENT_SMOKE_ENGINE:-}" ]]; then
  # The round trip needs the connect role (CC1 Task 1) on the VM; until the
  # nightly's release has it, the engine leg reports a skip rather than a pass.
  echo "skip engine round trip: the connect role is not in the release the nightly installs yet (CC1 Task 1)"
else
  echo "skip engine round trip (CONFLUENT_SMOKE_ENGINE is not set: Confluent-only run)"
fi
exit "$fail"
