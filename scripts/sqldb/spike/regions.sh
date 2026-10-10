#!/usr/bin/env bash
# Regions per keyspace and the cost of many keyspaces (SQ1 Task 1, measurement 3).
#  1. Bootstrap one fresh keyspace with split-table = true and one with false,
#     and count the regions in each keyspace's txn range [x|id, x|id+1).
#  2. Create and bootstrap keyspaces sequentially (split-table = false) up to
#     each checkpoint in CHECKPOINTS ("20 50 100", cumulative), and record the
#     cluster's region count, PD and TiKV RSS, and TiKV's heartbeat metrics.
# Writes target-spike/regions.jsonl and target-spike/density.jsonl.
# jq and awk programs are single-quoted on purpose.
# shellcheck disable=SC2016
set -euo pipefail
# shellcheck source=lib.sh source-path=SCRIPTDIR
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

CHECKPOINTS="${CHECKPOINTS:-20 50 100}"
HB_WINDOW="${HB_WINDOW:-30}"
PORT="$(tidb_port 0)"
run="r$(date +%H%M%S)"
: > "$SPIKE_OUT/regions.jsonl"
: > "$SPIKE_OUT/density.jsonl"
trap 'tidb_stop 0' EXIT

# PD stores region keys memcomparable-encoded and hex-printed: the 4-byte
# prefix x|id encodes as 78 <id 3 bytes> 00000000 FB.
ks_bounds() {
  printf '78%06X00000000FB 78%06X00000000FB\n' "$1" $(( $1 + 1 ))
}

# regions_in <keyspace id>: regions overlapping the keyspace's txn range.
regions_in() {
  local lo hi
  read -r lo hi < <(ks_bounds "$1")
  curl -fsS "$PD_URL/pd/api/v1/regions" | jq --arg lo "$lo" --arg hi "$hi" \
    '[.regions[] | select(.start_key < $hi and (.end_key == "" or .end_key > $lo))] | length'
}

region_total() { curl -fsS "$PD_URL/pd/api/v1/regions/count" | jq .count; }

# bootstrap <keyspace> <split-table>: start TiDB, wait for SELECT 1, stop it.
bootstrap() {
  tidb_start 0 "$1" "$2" false
  wait_port "$PORT" 600 && wait_query "$PORT" 120
  tidb_stop 0
}

# settle: wait until the region count stops changing for 5 s (max 60 s).
settle() {
  local prev=-1 cur i
  for i in $(seq 12); do
    cur="$(region_total)"
    [[ "$cur" == "$prev" ]] && return 0
    prev="$cur"; sleep 5
  done
  log "region count still moving after 60 s (i=$i)"
}

# --- 1. regions per empty database -----------------------------------------
for split in true false; do
  ks="${run}s${split:0:1}"
  ks_create "$ks" >/dev/null
  id="$(ks_id "$ks")"
  before="$(regions_in "$id")"
  t0="$(now_ms)"; bootstrap "$ks" "$split"; ms=$(( $(now_ms) - t0 ))
  settle
  after="$(regions_in "$id")"
  emit regions.jsonl --arg ks "$ks" --argjson id "$id" --argjson split "$split" \
    --argjson before "$before" --argjson after "$after" --argjson ms "$ms" --argjson total "$(region_total)" \
    '{keyspace:$ks, id:$id, split_table:$split, regions_before_bootstrap:$before,
      regions_after_bootstrap:$after, bootstrap_ms:$ms, cluster_regions:$total}'
  log "split-table=$split: $before -> $after regions in keyspace $ks (id $id)"
done

# --- 2. density --------------------------------------------------------------
metric_sum() { # metric_sum <metrics text file> <regex on the series line>
  awk -v re="$2" '$0 !~ /^#/ && $0 ~ re { s += $NF } END { printf "%.0f\n", s }' "$1"
}

snapshot() { # snapshot <n keyspaces bootstrapped>
  local m1="$SPIKE_OUT/.m1" m2="$SPIKE_OUT/.m2" p1="$SPIKE_OUT/.p1" p2="$SPIKE_OUT/.p2"
  settle
  curl -fsS "$TIKV_STATUS/metrics" > "$m1"; curl -fsS "$PD_URL/metrics" > "$p1"
  sleep "$HB_WINDOW"
  curl -fsS "$TIKV_STATUS/metrics" > "$m2"; curl -fsS "$PD_URL/metrics" > "$p2"
  local hb1 hb2 phb1 phb2
  hb1="$(metric_sum "$m1" '^tikv_pd_heartbeat_message_total')"
  hb2="$(metric_sum "$m2" '^tikv_pd_heartbeat_message_total')"
  phb1="$(metric_sum "$p1" '^pd_scheduler_region_heartbeat[{].*status="ok"')"
  phb2="$(metric_sum "$p2" '^pd_scheduler_region_heartbeat[{].*status="ok"')"
  emit density.jsonl --argjson n "$1" --argjson regions "$(region_total)" \
    --argjson tikv_regions "$(metric_sum "$m2" '^tikv_raftstore_region_count[{].*type="region"')" \
    --argjson hibernated "$(metric_sum "$m2" '^tikv_raftstore_hibernated_peer_state[{].*state="hibernated"')" \
    --argjson awaken "$(metric_sum "$m2" '^tikv_raftstore_hibernated_peer_state[{].*state="awaken"')" \
    --argjson hb_per_s "$(awk -v a="$hb1" -v b="$hb2" -v w="$HB_WINDOW" 'BEGIN { printf "%.2f", (b-a)/w }')" \
    --argjson pd_hb_per_s "$(awk -v a="$phb1" -v b="$phb2" -v w="$HB_WINDOW" 'BEGIN { printf "%.2f", (b-a)/w }')" \
    --argjson pd_rss "$(rss_kib "$SPIKE_PROJECT-pd-1")" --argjson tikv_rss "$(rss_kib "$SPIKE_PROJECT-tikv-1")" \
    --argjson pd_cg "$(cgroup_mib "$SPIKE_PROJECT-pd-1")" --argjson tikv_cg "$(cgroup_mib "$SPIKE_PROJECT-tikv-1")" \
    '{keyspaces:$n, cluster_regions:$regions, tikv_region_count:$tikv_regions,
      hibernated_peers:$hibernated, awaken_peers:$awaken,
      tikv_pd_heartbeats_per_s:$hb_per_s, pd_region_heartbeats_ok_per_s:$pd_hb_per_s,
      pd_rss_kib:$pd_rss, tikv_rss_kib:$tikv_rss, pd_cgroup_mib:$pd_cg, tikv_cgroup_mib:$tikv_cg}'
  rm -f "$m1" "$m2" "$p1" "$p2"
  log "density at $1 keyspaces: $(tail -1 "$SPIKE_OUT/density.jsonl")"
}

snapshot 0
done_n=0
for cp in $CHECKPOINTS; do
  while (( done_n < cp )); do
    done_n=$(( done_n + 1 ))
    ks="${run}d$done_n"
    ks_create "$ks" >/dev/null
    t0="$(now_ms)"; bootstrap "$ks" false
    emit regions.jsonl --arg ks "$ks" --argjson n "$done_n" --argjson ms $(( $(now_ms) - t0 )) \
      '{keyspace:$ks, density_n:$n, bootstrap_ms:$ms}'
  done
  snapshot "$done_n"
done
log "done: $SPIKE_OUT/regions.jsonl, $SPIKE_OUT/density.jsonl"
