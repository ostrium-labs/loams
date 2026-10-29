#!/usr/bin/env bash
# The BQ1-kv native-API runs (the headline numbers), by phase:
#   .he0-shared/benchslot.sh bq1-kv bench/kv/native-suite.sh <phase>
# Phases: local (memory + embedded, get/put/mix at depth 1/16/128 on 6 cores),
# scale (1/2/4 single-core processes, memory, mix, depth 1 and 16),
# tikv (starts the playground; loams-kv on TiKV and TiKV RawKV, depth 1/16/128,
# then 1/2/4 client processes).
# Each result line gets the load average, iowait share and host state added
# and is appended to bench/results/bq1/kv/native-<phase>.jsonl.
# Quiet window (bench-1b-qps.md): holds .he0-shared/quiet for the phase and
# waits up to 5 minutes for rustc to finish and the load to drop below 2.
set -euo pipefail
H=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
ROOT=$(cd "$H/../.." && pwd)
SHARED=$(cd "$ROOT/.." && pwd)/.he0-shared
phase=$1
OUT=$ROOT/bench/results/bq1/kv/native-$phase.jsonl
D=${D:-10}
NL=$H/native-load.sh

echo $(( $(date +%s) + 1200 )) >"$SHARED/quiet"
trap 'rm -f "$SHARED/quiet"; [ -z "${PG_STARTED:-}" ] || "$ROOT/scripts/tikv/playground.sh" stop --tag loams-bq1kv >/dev/null 2>&1 || true' EXIT
for _ in $(seq 1 60); do
  load=$(cut -d' ' -f1 /proc/loadavg)
  if ! pgrep -x rustc >/dev/null && awk -v l="$load" 'BEGIN{exit !(l < 2)}'; then break; fi
  sleep 5
done

cpu_line() { awk '/^cpu /{print $2+$3+$4+$5+$6+$7+$8, $6}' /proc/stat; }
run() { # run <taskset cpus> <native-load args...>
  local cpus=$1; shift
  read -r t0 w0 < <(cpu_line)
  local res
  res=$(taskset -c "$cpus" "$NL" "$@")
  read -r t1 w1 < <(cpu_line)
  local load rustc
  load=$(cut -d' ' -f1-3 /proc/loadavg)
  rustc=$(pgrep -cx rustc || true)
  while IFS= read -r line; do
    [ -n "$line" ] || continue
    jq -c --arg load "$load" --arg cpus "$cpus" --argjson rustc "${rustc:-0}" \
      --argjson iowait "$(awk -v a="$((t1 - t0))" -v b="$((w1 - w0))" 'BEGIN{printf "%.3f", (a>0)?b/a:0}')" \
      --arg at "$(date -u +%FT%TZ)" --arg commit "$(git -C "$ROOT" rev-parse --short HEAD)" \
      '. + {loadavg: $load, iowait_share: $iowait, rustc_running: $rustc, taskset: $cpus, at: $at, commit: $commit}' \
      <<<"$line" | tee -a "$OUT"
  done <<<"$res"
}

parallel() { # parallel <n> <native-load args...>: n processes, process i on core 4+i
  local n=$1; shift
  local dir
  dir=$(mktemp -d -p /mnt/Projects/bench-data/bq1-kv par.XXXX)
  for ((i = 0; i < n; i++)); do
    taskset -c "$((4 + i))" "$NL" "$@" >"$dir/$i.json" &
  done
  wait
  jq -sc --argjson n "$n" --arg load "$(cut -d' ' -f1-3 /proc/loadavg)" \
    '{engine: "kv", op: (.[0].op + ".x" + ($n|tostring) + "proc"), processes: $n,
      qps: (map(.qps)|add), rps: (map(.rps)|add), depth: .[0].depth, cores: $n,
      p50_ms: (map(.p50_ms)|max), p99_ms: (map(.p99_ms)|max), p999_ms: (map(.p999_ms)|max),
      errors: (map(.errors)|add), per_process_qps: map(.qps), loadavg: $load}' "$dir"/*.json | tee -a "$OUT"
  rm -rf "$dir"
}

case $phase in
  local)
    for backend in memory embedded; do
      for depth in 1 16 128; do
        run 0-5 --backend "$backend" --op get,put,mix --depth "$depth" --cores 6 --duration "$D"
      done
    done
    ;;
  scale)
    for depth in 1 16; do
      for n in 1 2 4; do
        parallel "$n" --backend memory --op mix --depth "$depth" --cores 1 --duration "$D"
      done
    done
    ;;
  tikv)
    PG_STARTED=1
    "$ROOT/scripts/tikv/playground.sh" start --tag loams-bq1kv --force >&2
    tikv=$(pgrep -f -- "tikv-server .*/\.tiup/data/loams-bq1kv/" | head -1)
    pd=$(pgrep -f -- "pd-server .*/\.tiup/data/loams-bq1kv/" | head -1)
    taskset -a -p -c 4-9 "$tikv" >/dev/null
    taskset -a -p -c 10-11 "$pd" >/dev/null
    sleep 10 # the first region and the PD leader settle
    for backend in tikv-raw tikv; do
      for depth in 1 16 128; do
        CONCURRENCY=256 run 0-3 --backend "$backend" --op get,put,mix --depth "$depth" --cores 4 --duration "$D"
      done
    done
    for n in 1 2 4; do # 1, 2, 4 client processes (cores 0..n-1) on one TiKV
      dir=$(mktemp -d -p /mnt/Projects/bench-data/bq1-kv par.XXXX)
      for ((i = 0; i < n; i++)); do
        CONCURRENCY=128 taskset -c "$i" "$NL" --backend tikv --op mix --depth 16 --cores 1 --duration "$D" >"$dir/$i.json" &
      done
      wait
      jq -sc --argjson n "$n" --arg load "$(cut -d' ' -f1-3 /proc/loadavg)" \
        '{engine: "kv", op: ("tikv.mix.x" + ($n|tostring) + "proc"), processes: $n,
          qps: (map(.qps)|add), rps: (map(.rps)|add), depth: 16, cores: $n,
          p50_ms: (map(.p50_ms)|max), p99_ms: (map(.p99_ms)|max), p999_ms: (map(.p999_ms)|max),
          errors: (map(.errors)|add), per_process_qps: map(.qps), loadavg: $load}' "$dir"/*.json | tee -a "$OUT"
      rm -rf "$dir"
    done
    ;;
  *) echo "phases: local scale tikv" >&2; exit 2 ;;
esac
