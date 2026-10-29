#!/usr/bin/env bash
# Kafka gateway A/B (footnote path): baseline vs Fetch page-size fix; in-flight 1 vs 5; compression. Run under benchslot.
# usage: ab_kafka.sh <label> <gateway-binary>
set -u
HERE="$(cd "$(dirname "$0")" && pwd)"; export BIN="${BIN:-/mnt/Projects/bench-data/bq1-iggy/bin}"
LABEL=$1; export GW_BIN=$2; DATA=${DATA:-/mnt/Projects/bench-data/bq1-iggy}; export DATA
OUT="$HERE/../results/bq1/iggy"; mkdir -p "$OUT"
PY=/home/dinakaran/Documents/Ostriumlabs/loams-wt/cfb-bench/bench/confluent/.venv/bin/python; L="taskset -c 8-13"
"$HERE/stack.sh" up 4 gw >&2 || exit 1
j() { sed "s/^{/{\"label\":\"$1\",\"gateway\":\"$LABEL\",/"; }
{
 $L $PY $HERE/kafka_load.py consume --topic kc --partitions 4 --procs 4 --size 100 --preload 3000000 | j kafka-consume-p4
 for cfg in "1 none" "5 none" "5 lz4" "5 zstd"; do set -- $cfg
   $L $PY $HERE/kafka_load.py produce --topic kp$1$2 --partitions 4 --procs 6 --size 100 --seconds 15 --acks 1 --linger-ms 50 --in-flight $1 --compression $2 | j kafka-produce-inflight$1-$2
 done
} | tee "$OUT/ab-$LABEL.jsonl"
"$HERE/stack.sh" down >&2
