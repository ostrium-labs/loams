#!/usr/bin/env bash
# CFB teardown guard: the only thing that stands between the benchmark and an
# unbounded Confluent bill (Confluent Cloud has no budget API).
#
#   setsid nohup bench/confluent/guard.sh >>~/.cfb-state/guard.log 2>&1 &
#
# Every minute it reads $CFB_STATE/live.tsv, one line per billable resource this
# benchmark created:
#
#   kind  id  name  where  created_epoch  deadline_epoch  usd_per_hour
#
# (usd_per_hour is list price x the resource's current max eCKU / max CFU; the
# driver rewrites it when it raises a cap) and it
#   - deletes a resource whose deadline has passed;
#   - estimates spend = sum over live and deleted resources of
#       usd_per_hour x started hours (Confluent bills whole hours)
#     plus data = $CFB_STATE/data_usd (collect.sh data, from the Metrics API; the
#     guard refreshes it every 10 minutes itself);
#   - deletes everything when the estimate reaches $CFB_STOP_USD (or $CFB_STATE/stop_usd;
#     default 380),
#     when $CFB_STATE/ABORT exists, or at $CFB_HARD_DEADLINE (epoch).
# Deleting a Kafka cluster deletes its topics, ACLs and cluster-scoped API keys;
# Flink compute pools are deleted the same way. Keys and service accounts are
# deleted by teardown.sh (and by this guard on abort).
set -uo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib/cc.sh
source "$here/lib/cc.sh"
stop_usd="${CFB_STOP_USD:-$(cat "$CFB_STATE/stop_usd" 2>/dev/null || echo 380)}"
hard_deadline="${CFB_HARD_DEADLINE:-$(cat "$CFB_STATE/hard_deadline" 2>/dev/null || echo 0)}"
live="$CFB_STATE/live.tsv"; done_tsv="$CFB_STATE/billed_done.tsv"
touch "$live" "$done_tsv"
echo $$ >"$CFB_STATE/guard.pid"

log() { echo "$(date -u +%FT%TZ) $*"; }

hours_started() {  # created_epoch end_epoch -> whole hours started (>=1)
  local s=$(( $2 - $1 )); (( s < 1 )) && s=1
  echo $(( (s + 3599) / 3600 ))
}

estimate() {  # -> USD with 2 decimals
  local now; now="$(date +%s)"
  awk -F'\t' -v now="$now" -v data="$(cat "$CFB_STATE/data_usd" 2>/dev/null || echo 0)" '
    FNR==NR { if ($1!="") { s=now-$5; if (s<1) s=1; h=int((s+3599)/3600); t+=h*$7 } ; next }
    { if ($1!="") { s=$2-$1; if (s<1) s=1; h=int((s+3599)/3600); t+=h*$3 } }
    END { printf "%.2f", t+data }' "$live" "$done_tsv"
}

delete_one() {  # kind id name where created usd_per_hour reason
  local kind="$1" id="$2" path
  case "$kind" in
    kafka-cluster) path="/cmk/v2/clusters/$id?environment=$CFB_ENV" ;;
    flink-pool)    path="/fcpm/v2/compute-pools/$id?environment=$CFB_ENV" ;;
    api-key)       path="/iam/v2/api-keys/$id" ;;
    service-account) path="/iam/v2/service-accounts/$id" ;;
    *) log "unknown kind $kind for $id"; return 1 ;;
  esac
  cc DELETE "$path" >/dev/null
  local st; st="$(cc_status)"
  case "$st" in
    2*|404)
      log "deleted $kind $id ($3) reason=$7 http=$st"
      cc_log_deleted "$kind" "$id" "$3" "$4" "guard: $7; HTTP $st"
      # keep its billed hours in the estimate
      printf '%s\t%s\t%s\n' "$5" "$(date +%s)" "$6" >>"$done_tsv"
      grep -v -P "^$kind\t$id\t" "$live" >"$live.tmp"; mv "$live.tmp" "$live"
      ;;
    *) log "FAILED to delete $kind $id: HTTP $st $(head -c 300 "$CFB_STATE/last.body")"; return 1 ;;
  esac
}

delete_all() {  # reason
  local kind id name where created deadline rate
  # pools first (they write into clusters), then clusters, then keys / accounts
  for k in flink-pool kafka-cluster api-key service-account; do
    while IFS=$'\t' read -r kind id name where created deadline rate; do
      [[ "$kind" == "$k" ]] || continue
      delete_one "$kind" "$id" "$name" "$where" "$created" "$rate" "$1"
    done < <(cat "$live")
  done
}

log "guard start pid $$ stop_usd=$stop_usd hard_deadline=$(date -u -d "@$hard_deadline" +%FT%TZ 2>/dev/null)"
while :; do
  exec {lockfd}>"$CFB_STATE/live.lock"; flock -w 120 "$lockfd" || { log "lock busy; next round"; sleep 30; continue; }
  now="$(date +%s)"
  # Data is billed per GB and is not capped by max eCKU: refresh it from the
  # Metrics API every 10 minutes (the 2026-10-28 morning run showed why).
  if (( now - ${last_data:-0} >= 600 )); then
    "$here/collect.sh" data >"$CFB_STATE/data.last" 2>&1 || log "collect.sh data failed: $(tail -1 "$CFB_STATE/data.last")"
    last_data="$now"
  fi
  est="$(estimate)"
  echo "$est" >"$CFB_STATE/estimate_usd"
  if [[ -e "$CFB_STATE/ABORT" ]]; then
    log "ABORT file present: deleting everything"; delete_all abort
  elif awk -v e="$est" -v s="$stop_usd" 'BEGIN{exit !(e>=s)}'; then
    log "estimate \$$est >= \$$stop_usd: deleting everything"; delete_all budget; touch "$CFB_STATE/ABORT"
  elif (( hard_deadline > 0 && now >= hard_deadline )); then
    log "hard deadline reached: deleting everything"; delete_all hard-deadline
  else
    while IFS=$'\t' read -r kind id name where created deadline rate; do
      [[ -n "$kind" ]] || continue
      (( now >= deadline )) && delete_one "$kind" "$id" "$name" "$where" "$created" "$rate" deadline
    done < <(cat "$live")
  fi
  # exit when nothing is left after an abort or the hard deadline
  if [[ ! -s "$live" ]] && { [[ -e "$CFB_STATE/ABORT" ]] || (( hard_deadline > 0 && now >= hard_deadline )); }; then
    log "nothing live; guard exits (estimate \$$est)"; exit 0
  fi
  flock -u "$lockfd"; exec {lockfd}>&-
  sleep 60
done
