#!/usr/bin/env bash
# The BQ1 combined run: every Loams engine and its native load generator at
# the same time on one machine, each on its own cores (budget.sh), for one
# fixed window. Writes bench/results/bq1/all/<ts>-d<depth>.json and .md.
#
#   .he0-shared/benchslot.sh bq1-all bench/all/run.sh [--duration 60]
#       [--depths 1,16] [--quiet] [--only slot,...] [--skip slot,...] [--label L]
#
#   --duration S   the measured window (default 60)
#   --depths L     one round per depth, all on the same servers (default 1,16)
#   --quiet        open the quiet window (.he0-shared/quiet) first: no new
#                  builds start, wait for rustc to finish and load < 2
#   --only/--skip  restrict the slots (for smoke tests)
#
# Slots: kv, kv-tikv, sql, pg, house, iggy, durable, gateway, graph, meter, git.
# A slot's script is $NATIVE_<SLOT> (upper case, '-' -> '_'; for a lane's
# script before it is merged) or the in-repo one named below. A slot whose
# script is missing is skipped and listed in the result.
#
# Alignment: scripts that take --start-at (CONTRACT.md) all start their
# window at T0. Older ones (no --start-at) start at T0 - lead, where lead is
# their setup time measured in a calibration run, so their window lands near
# T0 too; the per-core samples in the result show the real overlap.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../.." && pwd)"
# shellcheck source=budget.sh
source "$here/budget.sh"
DATA=${BQ1_ALL_DATA:-/mnt/Projects/bench-data/bq1-all}
export BQ1_ALL_BIN=${BQ1_ALL_BIN:-$DATA/bin}
shared=${SHARED:-$HOME/Documents/Ostriumlabs/loams-wt/.he0-shared}
out="$root/bench/results/bq1/all"
duration=60 depths=1,16 quiet=0 only="" skip="" label=""
SETUP_S=${SETUP_S:-75}
while [ $# -gt 0 ]; do
  case $1 in
    --duration) duration=$2; shift 2 ;;
    --depths) depths=$2; shift 2 ;;
    --quiet) quiet=1; shift ;;
    --only) only=$2; shift 2 ;;
    --skip) skip=$2; shift 2 ;;
    --label) label=$2; shift 2 ;;
    *) echo "run.sh: unknown argument $1" >&2; exit 2 ;;
  esac
done
mkdir -p "$out" "$DATA/logs"
log() { printf '[run %s] %s\n' "$(date +%H:%M:%S)" "$*" >&2; }

# ---- slots --------------------------------------------------------------
# slot | engine | in-repo script | own cores (servers, in-process) | load cores
#      | align (startat | lead:<s>) | extra args (D = depth)
slots=(
  "kv|kv|bench/all/native/kv.sh|$KV_CORES|$KV_CORES|startat|--ops get,put"
  "kv-tikv|kv|bench/all/native/kv-tikv.sh|$TIKV_CORES|$SQL_LOAD|startat|--ops get,put"
  "sql|sql|bench/all/native/sql.sh|$TIDB_CORES|$SQL_LOAD|startat|"
  "pg|pg|bench/all/native/pg.sh|$PG_CORES|$PG_LOAD|startat|"
  "house|house|bench/house/native-load.sh|$HOUSE_CORES|$HOUSE_LOAD|lead:${HOUSE_LEAD:-5}|--ops point --port-base 48600"
  "iggy|iggy|bench/all/native/iggy.sh|$IGGY_CORES|$IGGY_LOAD|startat|"
  "durable|durable|bench/all/native/durable.sh|$RESONATE_CORES|$DURABLE_LOAD|startat|"
  "gateway|gateway|bench/all/native/gateway.sh|$GATEWAY_CORES|$GATEWAY_LOAD|startat|"
  "graph|graph|bench/graph/native-load.sh|$GRAPH_CORES|$GRAPH_CORES|lead:${GRAPH_LEAD:-15}|--op point"
  "meter|meter|bench/meter/native-load.sh|$METER_CORES|$METER_CORES|lead:${METER_LEAD:-1}|"
  "git|git|bench/git/native-load.sh|$GIT_CORES|$GIT_CORES|lead:${GIT_LEAD:-2}|--op push"
)

in_list() { [[ ",$2," == *",$1,"* ]]; }

# The arguments each script takes. Lane scripts written before CONTRACT.md
# read --cores as the engine's whole budget; see README.md.
slot_args() { # slot depth start_at own load
  local slot=$1 d=$2 at=$3 own=$4 load=$5
  # Lane scripts read a bare number as a core count: write one core as N-N.
  [[ $own =~ ^[0-9]+$ ]] && own="$own-$own"
  [[ $load =~ ^[0-9]+$ ]] && load="$load-$load"
  case $slot in
    house) echo "--duration $duration --cores $own --client-cores $load --depth $d" ;;
    graph|git|meter) echo "--duration $duration --cores $own --depth $d" ;;
    *) echo "--duration $duration --cores $load --depth $d --start-at $at" ;;
  esac
}

resolve() { # slot in-repo-path -> script path or ""
  local var
  var="NATIVE_$(echo "$1" | tr 'a-z-' 'A-Z_')"
  if [ -n "${!var:-}" ]; then echo "${!var}"; return; fi
  [ -x "$root/$2" ] && echo "$root/$2" || echo ""
}

# ---- host sampling -------------------------------------------------------
sampler_pid=""
start_sampler() { # file: "<epoch> <loadavg1> | per-cpu lines"
  local f=$1
  (
    while :; do
      { printf '%s %s\n' "$(date +%s.%N)" "$(cut -d' ' -f1 /proc/loadavg)"; grep '^cpu' /proc/stat; } >> "$f"
      sleep 1
    done
  ) &
  sampler_pid=$!
}
stop_sampler() { [ -n "$sampler_pid" ] && kill "$sampler_pid" 2> /dev/null; wait "$sampler_pid" 2> /dev/null || true; sampler_pid=""; }

cleanup() {
  stop_sampler
  [ "$quiet" = 1 ] && rm -f "$shared/quiet"
  "$here/engines.sh" down || true
}
trap cleanup EXIT

# ---- quiet window --------------------------------------------------------
if [ "$quiet" = 1 ]; then
  echo $(( $(date +%s) + 1200 )) > "$shared/quiet"
  log "quiet window open; waiting for rustc and load < 2 (at most 8 min)"
  for _ in $(seq 96); do
    if ! pgrep -x rustc > /dev/null && awk '{exit !($1 < 2)}' /proc/loadavg; then break; fi
    sleep 5
  done
  log "quiet: load $(cut -d' ' -f1-3 /proc/loadavg), rustc $(pgrep -x rustc | wc -l)"
fi

# ---- servers -------------------------------------------------------------
log "starting servers"
"$here/engines.sh" up

commit=$(git -C "$root" rev-parse --short HEAD)
IFS=, read -r -a depth_list <<< "$depths"
for depth in "${depth_list[@]}"; do
  ts=$(date -u +%Y%m%dT%H%M%SZ)
  rdir="$DATA/logs/$ts-d$depth"; mkdir -p "$rdir"
  t0=$(( $(date +%s) + SETUP_S ))
  log "round depth=$depth: window $(date -d "@$t0" +%T) + ${duration}s"
  start_sampler "$rdir/samples.txt"
  meta="$rdir/slots.tsv"; : > "$meta"
  pids=()
  for row in "${slots[@]}"; do
    IFS='|' read -r slot engine path own load align extra <<< "$row"
    if [ -n "$only" ] && ! in_list "$slot" "$only"; then continue; fi
    if [ -n "$skip" ] && in_list "$slot" "$skip"; then continue; fi
    script=$(resolve "$slot" "$path")
    if [ -z "$script" ]; then
      printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$slot" "$engine" "missing:$path" "$own" "$load" "$align" 0 0 >> "$meta"
      log "$slot: no script ($path), skipped"; continue
    fi
    read -r -a args <<< "$(slot_args "$slot" "$depth" "$t0" "$own" "$load") $extra"
    launch=$(date +%s)
    if [[ $align == lead:* ]]; then launch=$(( t0 - ${align#lead:} )); fi
    (
      now=$(date +%s); [ "$launch" -gt "$now" ] && sleep $(( launch - now ))
      date +%s.%N > "$rdir/$slot.started"
      # A lane Iggy script (NATIVE_IGGY) wants BIN; the House's lib.sh
      # reads BIN too, for its own binaries, so only Iggy gets it.
      if [ "$slot" = iggy ]; then export BIN=$BQ1_ALL_BIN; fi
      PERSONS=${PERSONS:-100000} timeout $(( SETUP_S + duration + 240 )) "$script" "${args[@]}" \
        > "$rdir/$slot.jsonl" 2> "$rdir/$slot.log"
      echo "$? $(date +%s.%N)" > "$rdir/$slot.ended"
    ) &
    pid=$!
    pids+=("$pid")
    printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$slot" "$engine" "$script" "$own" "$load" "$align" \
      "$(sha256sum "$script" | cut -c1-12)" "${args[*]}" >> "$meta"
  done
  for p in "${pids[@]}"; do wait "$p" || true; done
  stop_sampler
  python3 -I "$here/aggregate.py" --dir "$rdir" --t0 "$t0" --duration "$duration" --depth "$depth" \
    --commit "$commit" --label "$label" --out "$out/$ts-d$depth"
  log "round depth=$depth done: $out/$ts-d$depth.{json,md}"
done
