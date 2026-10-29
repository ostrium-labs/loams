#!/usr/bin/env bash
# In-region load generation with Confluent Cloud for Apache Flink (the laptop
# uplink cannot reach any eCKU limit, so the "push to max" runs inside the region).
#
#   flink.sh run    POOL CLUSTER NAME 'SQL'   submit a statement (catalog loams-ci, database CLUSTER)
#   flink.sh status POOL NAME
#   flink.sh stop   POOL NAME                 delete the statement
#   flink.sh list   POOL
#
# POOL is a loams-cfb-flink-* pool made by cfb.sh (its Flink key is in ~/.cfb-state/pools/POOL/key).
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib/cc.sh
source "$here/lib/cc.sh"
org="${CFB_ORG:-4edaf91e-6d1c-461f-bc18-33f9763c100b}"
pool="$2"; pdir="$CFB_STATE/pools/$pool"; [[ -s "$pdir/pool.json" ]] || { echo "no pool $pool" >&2; exit 1; }
cloud="$(jq -r '.cloud|ascii_downcase' "$pdir/pool.json")"; region="$(jq -r .region "$pdir/pool.json")"
pool_id="$(jq -r .id "$pdir/pool.json")"
base="https://flink.$region.$cloud.confluent.cloud/sql/v1/organizations/$org/environments/$CFB_ENV/statements"
case "$1" in
  run)
    cluster="$3" name="$4" sql="$5"
    body="$(jq -cn --arg n "$name" --arg o "$org" --arg e "$CFB_ENV" --arg s "$sql" --arg p "$pool_id" --arg db "$cluster" \
      '{name:$n, organization_id:$o, environment_id:$e,
        spec:{statement:$s, compute_pool_id:$p, stopped:false,
              properties:{"sql.current-catalog":"loams-ci","sql.current-database":$db}}}')"
    out="$(cc_keyed "$pdir/key" POST "$base" "$body")"
    echo "HTTP $(cc_status) $(jq -c '{name, phase:.status.phase, detail:.status.detail, errors}' <<<"$out" 2>/dev/null || head -c 400 <<<"$out")"
    echo "$(date -u +%FT%TZ) flink run $pool $cluster $name" >>"$CFB_STATE/events.log"
    ;;
  status)
    cc_keyed "$pdir/key" GET "$base/$3" | jq -c 2>/dev/null '{name, phase:.status.phase, detail:(.status.detail|tostring|.[0:400]), scaling:.status.scaling_status}'
    ;;
  stop)
    cc_keyed "$pdir/key" DELETE "$base/$3" >/dev/null; echo "HTTP $(cc_status) deleted statement $3"
    echo "$(date -u +%FT%TZ) flink stop $pool $3" >>"$CFB_STATE/events.log"
    ;;
  list)
    cc_keyed "$pdir/key" GET "$base?page_size=100" | jq -c '.data[]? | {name, phase:.status.phase, db:.spec.properties["sql.current-database"]}'
    ;;
  *) sed -n '2,11p' "$0"; exit 2 ;;
esac
