#!/usr/bin/env bash
# Iggy native-API load generator for the BQ1-ALL combined run (contract: bench/all/CONTRACT.md).
#   native-load.sh --duration S --cores LIST --depth N [--target HOST:PORT] [--server-cores LIST]
#                  [--start-at EPOCH] [--ops produce,consume] [--transport tcp|quic]
#                  [--conns N] [--partitions P] [--size B] [--shards N] [--user U --pass P]
# Load generator: iggy-bench (native TCP or QUIC), pinned to --cores. depth = messages per batch (depth_kind
# "batch"): 1 is one message per send/poll, 1000 is the default batching. conns = producers (and
# consumers, at most one per partition), default = number of --cores.
# --ops: produce -> balanced-producer; produce,consume (default) -> producers and a consumer group running
# together in one window; consume -> a prefill (setup), then a consumer group over it.
# iggy-bench sizes its run by data volume, not time, so a short calibration run sets -T to fit --duration.
# Without --target it starts its own iggy-server (fresh data under $BQ1_DATA) on --server-cores and
# removes it on exit. BIN must hold iggy-server and iggy-bench. qps counts messages.
set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
DUR=20; CORES=""; DEPTH=1000; TARGET=""; SCORES=0-3; START_AT=0; OPS="produce,consume"; TRANSPORT=tcp
CONNS=""; PARTS=4; SIZE=100; SHARDS=""; USER=iggy; PASS=bq1-local
while [ $# -gt 0 ]; do case "$1" in
  --duration) DUR=$2; shift 2;; --cores) CORES=$2; shift 2;; --depth) DEPTH=$2; shift 2;;
  --target) TARGET=$2; shift 2;; --server-cores) SCORES=$2; shift 2;; --start-at) START_AT=$2; shift 2;;
  --ops) OPS=$2; shift 2;; --transport) TRANSPORT=$2; shift 2;; --conns) CONNS=$2; shift 2;;
  --partitions) PARTS=$2; shift 2;; --size) SIZE=$2; shift 2;; --shards) SHARDS=$2; shift 2;;
  --user) USER=$2; shift 2;; --pass) PASS=$2; shift 2;;
  *) echo "unknown argument: $1" >&2; exit 2;; esac; done
[ -n "$CORES" ] || { echo "--cores is required" >&2; exit 2; }
BIN="${BIN:?BIN: dir with iggy-server and iggy-bench}"; export BIN
export DATA="${BQ1_DATA:-/mnt/Projects/bench-data/bq1-iggy}"; LOG="$DATA/logs"; mkdir -p "$LOG"
NCORES=$(taskset -c "$CORES" nproc); CONNS="${CONNS:-$NCORES}"
[ -n "$SHARDS" ] || SHARDS=$(taskset -c "$SCORES" nproc)
OWN=0; ADDR="$TARGET"
cleanup() { [ "$OWN" = 1 ] && "$HERE/stack.sh" down >&2; }; trap cleanup EXIT
if [ -z "$ADDR" ]; then OWN=1
  SERVER_CPUS="$SCORES" "$HERE/stack.sh" up "$SHARDS" >&2 || exit 1
  [ "$TRANSPORT" = quic ] && ADDR=127.0.0.1:48080 || ADDR=127.0.0.1:48090; fi
C=$(( CONNS < PARTS ? CONNS : PARTS ))
bench() { # <data-mb> <args...>   (iggy-bench; -T = total data volume)
  local mb=$1; shift
  taskset -c "$CORES" "$BIN/iggy-bench" -u "$USER" -p "$PASS" -m "$SIZE" -P "$DEPTH" -T "${mb}MB" "$@" "$TRANSPORT" --server-address "$ADDR"
}
PYP='import re,sys
t=re.sub(r"\x1b\[[0-9;]*m","",open(sys.argv[1],errors="replace").read())'
rate() { python3 -c "$PYP
m=re.search(r\"(Producers|Consumers) Results: Total throughput: ([\d.]+) MB/s\",t); print(m.group(2) if m else 5)" "$1"; }
emit() { # <logfile> <t_start> <t_end> <late> ; one JSON line per Producers/Consumers result in the log
  python3 - "$1" "$2" "$3" "$4" "$DEPTH" "$NCORES" "$TRANSPORT" "$CONNS" "$SIZE" "${OWN:-0}" "$SHARDS" <<'PY'
import json,re,sys
path,t0,t1,late,depth,cores,tr,conns,size,own,shards=sys.argv[1:]
t=re.sub(r"\x1b\[[0-9;]*m","",open(path,errors="replace").read())
err=len(re.findall(r"\bERROR\b|panicked",t)); n=0
for m in re.finditer(r"(Producers|Consumers) Results: Total throughput: ([\d.]+) MB/s, (\d+) messages/s.*?p50 latency: ([\d.]+) ms.*?p99 latency: ([\d.]+) ms, p999 latency: ([\d.]+) ms.*?total time: ([\d.]+) s",t):
    n+=1; qps=int(m.group(3))
    d={"engine":"iggy","op":"produce" if m.group(1)=="Producers" else "consume","qps":float(qps),
       "p50_ms":float(m.group(4)),"p99_ms":float(m.group(5)),"p999_ms":float(m.group(6)),"depth":int(depth),"cores":int(cores),
       "errors":err,"rps":round(qps/max(int(depth),1),1),"depth_kind":"batch","conns":int(conns),"window_s":float(m.group(7)),
       "t_start":float(t0),"t_end":float(t1),"late_s":float(late),"api":"iggy native "+tr+" (iggy-bench, batch="+depth+", msg="+size+"B)"}
    if own=="1": d["server_cores"]=int(shards)
    print(json.dumps(d))
if n==0: print(json.dumps({"engine":"iggy","op":"produce","qps":0.0,"p50_ms":None,"p99_ms":None,"p999_ms":None,"depth":int(depth),"cores":int(cores),"errors":err+1,"note":"no result in iggy-bench output"}))
sys.exit(0 if n else 3)
PY
}
echo "native-load: iggy $TRANSPORT $ADDR ops=$OPS depth=$DEPTH conns=$CONNS cores=$CORES" >&2
# ---- setup: calibrate MB/s on a throwaway topic, size the data volume to --duration -------------------------
bench 30 balanced-producer -s 1 -a "$PARTS" -p "$CONNS" >"$LOG/nl-cal.log" 2>&1
RATE=$(rate "$LOG/nl-cal.log"); MB=$(python3 -c "print(max(30,min(3000,int(float('$RATE')*$DUR))))")
if [ "$OPS" = consume ]; then   # prefill (setup, not measured)
  bench "$MB" --reuse-streams balanced-producer -s 1 -a "$PARTS" -p "$CONNS" >"$LOG/nl-prefill.log" 2>&1; fi
# ---- wait for the common start, then measure --------------------------------------------------------------
NOW=$(date +%s); LATE=0
if [ "$START_AT" -gt "$NOW" ]; then sleep $((START_AT - NOW)); elif [ "$START_AT" -gt 0 ]; then LATE=$((NOW - START_AT)); fi
T0=$(date +%s.%N)
case "$OPS" in
  produce) bench "$MB" --reuse-streams balanced-producer -s 1 -a "$PARTS" -p "$CONNS" >"$LOG/nl-run.log" 2>&1;;
  consume) bench "$MB" --reuse-streams balanced-consumer-group -s 1 -c "$C" -g 1 >"$LOG/nl-run.log" 2>&1;;
  *) bench "$MB" --reuse-streams balanced-producer-and-consumer-group -s 1 -a "$PARTS" -p "$CONNS" -c "$C" -g 1 >"$LOG/nl-run.log" 2>&1;;
esac
T1=$(date +%s.%N)
emit "$LOG/nl-run.log" "$T0" "$T1" "$LATE"
