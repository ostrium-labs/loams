#!/usr/bin/env bash
# CFB collector: Confluent Metrics API (per cluster) and Costs API.
#
#   collect.sh data                    bytes in/out of every benchmark cluster since it was made;
#                                      writes ~/.cfb-state/data_usd (read by guard.sh)
#   collect.sh series CLUSTER FROM TO [PT1M] OUT.json
#                                      per-minute received/sent bytes, requests, partitions,
#                                      connections for one cluster (push tests)
#   collect.sh costs FROM TO OUT.json  Costs API lines for the org (lags up to 72 h)
#
# Same endpoints the loams-confluent crate uses (crates/loams-confluent/src/{metrics,costs}.rs),
# called with curl so the collector runs without a Loams build.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib/cc.sh
source "$here/lib/cc.sh"
tel="https://api.telemetry.confluent.cloud/v2/metrics/cloud/query"
price_gb="${CFB_DATA_USD_PER_GB:-0.05}"

query() {  # metric cluster from to granularity -> JSON array of {timestamp,value}
  local body
  body="$(jq -cn --arg m "$1" --arg c "$2" --arg i "$3/$4" --arg g "$5" \
    '{aggregations:[{metric:$m}], filter:{field:"resource.kafka.id", op:"EQ", value:$c}, granularity:$g, intervals:[$i], limit:1000}')"
  local out; out="$(cc POST "$tel" "$body")"
  [[ "$(cc_status)" == 200 ]] || { echo "[]"; echo "metrics $1 $2: HTTP $(cc_status) $(head -c 200 <<<"$out")" >&2; return 0; }
  jq -c '[.data[]? | {t:.timestamp, v:.value}]' <<<"$out"
}

iso() { date -u -d "@$1" +%Y-%m-%dT%H:%M:%SZ; }

case "${1:-}" in
  data)
    total_in=0; total_out=0; now="$(date +%s)"
    rows=""
    while IFS=$'\t' read -r name id created; do
      from="$(iso $(( created / 60 * 60 - 60 )))"; to="$(iso $(( now / 60 * 60 )))"
      in="$(query io.confluent.kafka.server/received_bytes "$id" "$from" "$to" PT1H | jq '[.[].v]|add // 0')"
      out="$(query io.confluent.kafka.server/sent_bytes "$id" "$from" "$to" PT1H | jq '[.[].v]|add // 0')"
      # Never go down: a deleted cluster answers 403 (no data), so keep the last
      # known totals per cluster in $CFB_STATE/data_bytes/<id> (in out).
      mkdir -p "$CFB_STATE/data_bytes"
      if [[ -s "$CFB_STATE/data_bytes/$id" ]]; then
        read -r pin pout <"$CFB_STATE/data_bytes/$id"
        in="$(awk -v a="$in" -v b="$pin" 'BEGIN{printf "%.0f", (a>b)?a:b}')"
        out="$(awk -v a="$out" -v b="$pout" 'BEGIN{printf "%.0f", (a>b)?a:b}')"
      fi
      echo "$in $out" >"$CFB_STATE/data_bytes/$id"
      rows+="$name $id in=$(awk -v b="$in" 'BEGIN{printf "%.3f GB", b/1e9}') out=$(awk -v b="$out" 'BEGIN{printf "%.3f GB", b/1e9}')"$'\n'
      total_in="$(awk -v a="$total_in" -v b="$in" 'BEGIN{printf "%.0f", a+b}')"
      total_out="$(awk -v a="$total_out" -v b="$out" 'BEGIN{printf "%.0f", a+b}')"
    done < <(cat "$CFB_STATE/clusters_all.tsv" 2>/dev/null; awk -F'\t' '$1=="kafka-cluster"{print $3"\t"$2"\t"$5}' "$CFB_STATE/live.tsv" | while IFS=$'\t' read -r n i c; do grep -q -P "^$n\t" "$CFB_STATE/clusters_all.tsv" 2>/dev/null || printf '%s\t%s\t%s\n' "$n" "$i" "$c"; done)
    usd="$(awk -v i="$total_in" -v o="$total_out" -v p="$price_gb" 'BEGIN{printf "%.2f", (i+o)/1e9*p}')"
    echo "$usd" >"$CFB_STATE/data_usd"
    printf '%s' "$rows"
    echo "total in $(awk -v b="$total_in" 'BEGIN{printf "%.3f", b/1e9}') GB, out $(awk -v b="$total_out" 'BEGIN{printf "%.3f", b/1e9}') GB -> data \$$usd at \$$price_gb/GB"
    ;;
  series)
    id="$2" from="$3" to="$4" g="${5:-PT1M}" out="$6"
    jq -n --arg id "$id" --arg from "$from" --arg to "$to" --arg g "$g" \
      --argjson rx "$(query io.confluent.kafka.server/received_bytes "$id" "$from" "$to" "$g")" \
      --argjson tx "$(query io.confluent.kafka.server/sent_bytes "$id" "$from" "$to" "$g")" \
      --argjson rq "$(query io.confluent.kafka.server/request_count "$id" "$from" "$to" "$g")" \
      --argjson pc "$(query io.confluent.kafka.server/partition_count "$id" "$from" "$to" "$g")" \
      --argjson cn "$(query io.confluent.kafka.server/active_connection_count "$id" "$from" "$to" "$g")" \
      --argjson rr "$(query io.confluent.kafka.server/received_records "$id" "$from" "$to" "$g")" \
      '{cluster:$id, from:$from, to:$to, granularity:$g, received_bytes:$rx, sent_bytes:$tx, request_count:$rq,
        partition_count:$pc, active_connection_count:$cn, received_records:$rr}' >"$out"
    jq -c '{cluster, points:(.received_bytes|length), max_rx_MBps:(([.received_bytes[].v]|max // 0)/60/1e6), max_partitions:([.partition_count[].v]|max)}' "$out"
    ;;
  costs)
    from="$2" to="$3" out="$4"
    all="[]"; next="/billing/v1/costs?start_date=$from&end_date=$to&page_size=1000"
    while [[ -n "$next" && "$next" != null ]]; do
      page="$(cc GET "$next")"
      [[ "$(cc_status)" == 200 ]] || { echo "costs: HTTP $(cc_status)" >&2; break; }
      all="$(jq -c --argjson a "$all" '$a + [.data[]?]' <<<"$page")"
      next="$(jq -r '.metadata.next // empty' <<<"$page")"
    done
    jq '.' <<<"$all" >"$out"
    jq -c '{lines:length, amount:([.[].amount]|add // 0), by_type:(group_by(.line_type)|map({(.[0].line_type):([.[].amount]|add)})|add)}' "$out"
    ;;
  *) sed -n '2,13p' "$0"; exit 2 ;;
esac
