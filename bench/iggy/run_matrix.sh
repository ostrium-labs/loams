#!/usr/bin/env bash
# BQ1-iggy: one (shards, partitions) configuration. Run the WHOLE script under
#   .he0-shared/benchslot.sh bq1-iggy bench/iggy/run_matrix.sh <shards> <partitions>
# Layout (14 cores): iggy-server 0-3 (N shards), Kafka gateway 4-7, load generators 8-13.
set -u
S="${1:?shards}"; P="${2:?partitions}"; HERE="$(cd "$(dirname "$0")" && pwd)"
export BIN="${BIN:-/mnt/Projects/bench-data/bq1-iggy/bin}"
DATA="${DATA:-/mnt/Projects/bench-data/bq1-iggy}"; export DATA
OUT="${OUT:-$HERE/../results/bq1/iggy}"; LOG="$DATA/logs"; mkdir -p "$OUT" "$LOG"
TAG="s${S}p${P}"; PY="${PY:-/home/dinakaran/Documents/Ostriumlabs/loams-wt/cfb-bench/bench/confluent/.venv/bin/python}"
LOAD="taskset -c 8-13"; ADDR=127.0.0.1:48090
BENCH="$LOAD $BIN/iggy-bench -u iggy -p bq1-local -m ${MSG:-100} -P 1000 -w 3s"
SZ="${DATA_MB:-1000}MB"
"$HERE/stack.sh" up "$S" gw || exit 1
SPID=$(cat "$DATA/run/server.pid"); GPID=$(cat "$DATA/run/gateway.pid")
jif() { awk '{print $14+$15}' /proc/$1/stat; }
now() { date +%s%N; }
# cpu_wrap <label> <cmd...>: run cmd; then print a line with cores used by server/gateway during it
cpu_wrap() { local l=$1; shift; local s0=$(jif $SPID) g0=$(jif $GPID) t0=$(now)
  "$@"; local s1=$(jif $SPID) g1=$(jif $GPID) t1=$(now)
  awk -v l="$l" -v s=$((s1-s0)) -v g=$((g1-g0)) -v d=$((t1-t0)) 'BEGIN{d/=1e9; printf "{\"label\":\"cpu-%s\",\"server_cores\":%.2f,\"gateway_cores\":%.2f,\"wall_s\":%.1f}\n",l,s/100/d,g/100/d,d}'; }
run_to() { local f=$1; shift; "$@" >"$f" 2>&1; }
nat() { # <label> <iggy-bench args...>
  local l=$1; shift; cpu_wrap "$l-$TAG" run_to "$LOG/$l-$TAG.log" $BENCH -T "$SZ" "$@" tcp --server-address $ADDR
  python3 "$HERE/parse_bench.py" "$l-$TAG" "$LOG/$l-$TAG.log"; }
kaf() { # <label> <kafka_load args...>
  local l=$1; shift; cpu_wrap "$l-$TAG" bash -c "$LOAD $PY $HERE/kafka_load.py $* 2>>$LOG/k-$TAG.err | sed 's/^{/{\"label\":\"$l-$TAG\",/' >$LOG/$l-$TAG.json"
  cat "$LOG/$l-$TAG.json"; }
{
  echo "{\"label\":\"meta\",\"shards\":$S,\"partitions\":$P,\"loams_commit\":\"$(git -C "$HERE" rev-parse --short HEAD)\",\"kernel\":\"$(uname -r)\",\"ts\":\"$(date -u +%FT%TZ)\"}"
  nat native-produce balanced-producer -s 1 -a "$P" -p 6
  nat native-consume balanced-consumer-group -s 1 -c "$P" -g 1
  "$HERE/stack.sh" down >/dev/null; "$HERE/stack.sh" up "$S" gw >/dev/null; SPID=$(cat "$DATA/run/server.pid"); GPID=$(cat "$DATA/run/gateway.pid")
  nat native-pc balanced-producer-and-consumer-group -s 1 -a "$P" -p 4 -c "$P" -g 1
  kaf kafka-produce-acks1 produce --topic kp1 --partitions "$P" --procs 6 --size 100 --seconds 20 --acks 1
  kaf kafka-produce-acksall produce --topic kpa --partitions "$P" --procs 6 --size 100 --seconds 20 --acks all
  kaf kafka-consume consume --topic kc --partitions "$P" --procs 6 --size 100 --preload 3000000
  kaf kafka-metadata metadata --topic kp1 --no-create --procs 6 --conns 4 --depth 32 --seconds 15
} | tee "$OUT/$TAG.jsonl"
"$HERE/stack.sh" down
