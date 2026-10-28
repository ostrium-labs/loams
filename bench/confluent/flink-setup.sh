#!/usr/bin/env bash
# Make the in-region generator on CLUSTER: a faker id table (100k rows/s, the
# connector's maximum, BOUNDED by number-of-rows so a statement ends on its own)
# and a raw sink topic; then `flink-setup.sh ... start N` runs N fan-out INSERT
# statements, each at most ROWS x FANOUT records of SIZE bytes (default 30M x 1 KB =
# about 30 GB, $1.50 of data in), whatever the pool deadline says.
#
#   flink-setup.sh POOL CLUSTER tables PARTITIONS [ROWS=100000]   (Confluent's maximum; ROWS x FANOUT records per statement)
#   flink-setup.sh POOL CLUSTER start N [TAG] [SIZE=1000] [FANOUT=300]
#   flink-setup.sh POOL CLUSTER stop  N [TAG]
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
f="$here/flink.sh"; pool="$1" c="$2"; short="${c#loams-cfb-}"
wait_done() { local i; for i in $(seq 1 30); do s="$("$f" status "$pool" "$1" 2>/dev/null | jq -r .phase 2>/dev/null || true)"; [[ "$s" == COMPLETED || "$s" == FAILED ]] && { echo "$1 $s"; return; }; sleep 3; done; echo "$1 timeout"; }
case "$3" in
  tables)
    "$f" run "$pool" "$c" "cfb-ddl-ids-$short${CFB_RETRY:-}" "CREATE TABLE IF NOT EXISTS cfb_ids (id BIGINT) WITH ('connector' = 'faker', 'fields.id.expression' = '#{Number.randomNumber ''9'',''false''}', 'rows-per-second' = '100000', 'number-of-rows' = '${5:-100000}')" >/dev/null
    wait_done "cfb-ddl-ids-$short${CFB_RETRY:-}"
    "$f" run "$pool" "$c" "cfb-ddl-sink-$short" "CREATE TABLE IF NOT EXISTS cfb_sink (val STRING) DISTRIBUTED INTO $4 BUCKETS WITH ('value.format' = 'raw', 'kafka.retention.time' = '30 min', 'kafka.retention.size' = '200 mb')" >/dev/null
    wait_done "cfb-ddl-sink-$short"
    ;;
  start|stop)
    n="$4" tag="${5:-a}" size="${6:-1000}" fan="${7:-300}"
    arr="$(seq -s, 1 "$fan")"
    for i in $(seq 1 "$n"); do
      name="cfb-push-$short-$tag$i"
      if [[ "$3" == start ]]; then
        "$f" run "$pool" "$c" "$name" "INSERT INTO cfb_sink SELECT CONCAT(CAST(g.id AS STRING), '-', CAST(t.x AS STRING), REPEAT('x', $size)) FROM cfb_ids AS g CROSS JOIN UNNEST(ARRAY[$arr]) AS t(x)" | cut -c1-160
      else
        "$f" stop "$pool" "$name"
      fi
    done ;;
  *) sed -n '2,9p' "$0"; exit 2 ;;
esac
