#!/usr/bin/env bash
# Latency of PD's keyspace HTTP API (SQ1 Task 1, measurement 1):
# N creates (POST /pd/api/v2/keyspaces), then for each keyspace
# PUT /pd/api/v2/keyspaces/{name}/state to DISABLED and then ARCHIVED.
# Writes target-spike/keyspace.jsonl: {op, name, id, ms}.
# jq and awk programs are single-quoted on purpose.
# shellcheck disable=SC2016
set -euo pipefail
# shellcheck source=lib.sh source-path=SCRIPTDIR
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

n="${1:-20}"
run="k$(date +%H%M%S)"
: > "$SPIKE_OUT/keyspace.jsonl"

# timed <curl args...>: prints "<http code> <ms>" and the body to stderr-free stdout.
timed() {
  curl -sS -o "$SPIKE_OUT/.body" -w '%{http_code} %{time_total}\n' \
    -H 'content-type: application/json' "$@"
}

names=()
for i in $(seq -w 1 "$n"); do
  name="${run}_$i"
  read -r code secs < <(timed -X POST "$PD_URL/pd/api/v2/keyspaces" -d "{\"name\":\"$name\"}")
  [[ "$code" == 200 ]] || { log "create $name: HTTP $code $(cat "$SPIKE_OUT/.body")"; exit 1; }
  id="$(jq -r .id "$SPIKE_OUT/.body")"
  emit keyspace.jsonl --arg op create --arg name "$name" --argjson id "$id" \
    --argjson s "$secs" '{op:$op, name:$name, id:$id, ms:($s*1000)}'
  names+=("$name")
done
log "created ${#names[@]} keyspaces"

for state in DISABLED ARCHIVED; do
  for name in "${names[@]}"; do
    read -r code secs < <(timed -X PUT "$PD_URL/pd/api/v2/keyspaces/$name/state" -d "{\"state\":\"$state\"}")
    [[ "$code" == 200 ]] || { log "$state $name: HTTP $code $(cat "$SPIKE_OUT/.body")"; exit 1; }
    emit keyspace.jsonl --arg op "$state" --arg name "$name" --argjson id 0 \
      --argjson s "$secs" '{op:$op, name:$name, id:$id, ms:($s*1000)}'
  done
done
# Does ARCHIVED -> TOMBSTONE work through the same API? Recorded, not timed.
read -r code _ < <(timed -X PUT "$PD_URL/pd/api/v2/keyspaces/${names[0]}/state" -d '{"state":"TOMBSTONE"}')
emit keyspace.jsonl --arg op TOMBSTONE_probe --arg name "${names[0]}" --argjson code "$code" \
  --arg body "$(head -c 300 "$SPIKE_OUT/.body")" '{op:$op, name:$name, http:$code, body:$body}'
rm -f "$SPIKE_OUT/.body"
log "done: $SPIKE_OUT/keyspace.jsonl"
