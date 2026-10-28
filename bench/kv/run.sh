#!/usr/bin/env bash
# BQ1-kv harness (docs/benchmarks/2026-10-28-1b-qps/kv.md): runs the Redis
# gateway (`loams-gateway --module resp`) on pinned cores, drives it with
# bench/kv/loadgen on disjoint cores, and records the loadgen's JSON plus the
# CPU seconds every server process used while it ran.
#
#   bench/kv/run.sh <name> <store> <shards> <server-cpus> <loadgen-cpus> [loadgen args...]
#
#   store:        memory | embedded | tikv (tikv needs PD at $PD, default 127.0.0.1:19379)
#   shards:       gateway processes, one store each (memory/embedded) or one
#                 root each in one TiKV keyspace (tikv)
#   server-cpus:  one taskset list per shard, separated by '/', or one list
#                 shared by all shards (e.g. "0-5" or "4/5/6/7")
#   loadgen-cpus: a taskset list
#
# Every run must be wrapped by .he0-shared/benchslot.sh (bench/kv/README.md).
# Output: $OUT/<name>.json (default bench/results/bq1/kv). Data lives under
# $DATA (default /mnt/Projects/bench-data/bq1-kv) and is deleted afterwards.
set -euo pipefail

name=$1 store=$2 shards=$3 server_cpus=$4 loadgen_cpus=$5
shift 5
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
TARGET=${TARGET:-/mnt/Projects/rust-cache/target/release}
GW=$TARGET/loams-gateway
LG=$TARGET/kv-loadgen
OUT=${OUT:-$ROOT/bench/results/bq1/kv}
DATA=${DATA:-/mnt/Projects/bench-data/bq1-kv}
PD=${PD:-127.0.0.1:19379}
KEYSPACE=${KEYSPACE:-loams_test_live}
BASE_PORT=${BASE_PORT:-16390}
EXTRA_PIDS=${EXTRA_PIDS:-} # other server processes to account (TiKV, PD)
mkdir -p "$OUT" "$DATA"

IFS=/ read -r -a cpu_lists <<<"$server_cpus"
pids=()
cleanup() {
  for p in "${pids[@]}"; do kill "$p" 2>/dev/null || true; done
  for p in "${pids[@]}"; do wait "$p" 2>/dev/null || true; done
  rm -rf "${DATA:?}/$name"
}
trap cleanup EXIT

addrs=()
for ((i = 0; i < shards; i++)); do
  port=$((BASE_PORT + i))
  dir=$DATA/$name/shard$i
  mkdir -p "$dir"
  case $store in
    memory) section=$'kind = "memory"\nname = "bq1-'"$name-$i"'"' ;;
    embedded) section=$'kind = "embedded"\npath = "'"$dir"'/resp.redb"\nkeyspace = "kv"' ;;
    tikv) section=$'kind = "tikv"\npd = ["'"$PD"'"]\nkeyspace = "'"$KEYSPACE"'"\nroot = "bq1-'"$name-$i-$$"'"' ;;
    *) echo "unknown store $store" >&2; exit 2 ;;
  esac
  cat >"$dir/resp.toml" <<EOF
listen = "127.0.0.1:$port"
databases = 16
max_clients = 10000
durability = "durable"
[store]
$section
EOF
  cpus=${cpu_lists[$i]:-${cpu_lists[0]}}
  RUST_LOG=warn taskset -c "$cpus" "$GW" --module resp --config "$dir/resp.toml" >"$dir/gw.log" 2>&1 &
  pids+=($!)
  addrs+=("127.0.0.1:$port")
done

for a in "${addrs[@]}"; do
  for _ in $(seq 1 100); do
    (exec 3<>"/dev/tcp/${a%:*}/${a#*:}") 2>/dev/null && break
    sleep 0.1
  done
done

ticks() { # utime+stime of a pid, in clock ticks
  awk '{print $14 + $15}' "/proc/$1/stat" 2>/dev/null || echo 0
}
acct=("${pids[@]}")
for p in $EXTRA_PIDS; do acct+=("$p"); done
declare -A before
for p in "${acct[@]}"; do before[$p]=$(ticks "$p"); done
t0=$(date +%s.%N)

addr_list=$(IFS=,; echo "${addrs[*]}")
result=$(taskset -c "$loadgen_cpus" "$LG" --addr "$addr_list" --label "$name" "$@")

t1=$(date +%s.%N)
hz=$(getconf CLK_TCK)
cpu_json="{"
first=1
for p in "${acct[@]}"; do
  used=$(( $(ticks "$p") - ${before[$p]} ))
  comm=$(cat "/proc/$p/comm" 2>/dev/null || echo gone)
  [ $first = 1 ] || cpu_json+=","
  first=0
  cpu_json+="\"$comm-$p\":$(awk -v u="$used" -v h="$hz" 'BEGIN{printf "%.2f", u/h}')"
done
cpu_json+="}"

for ((i = 0; i < shards; i++)); do
  if grep -qiE "error|panic" "$DATA/$name/shard$i/gw.log"; then
    echo "shard $i log:" >&2
    tail -5 "$DATA/$name/shard$i/gw.log" >&2
  fi
done

jq -c --arg store "$store" --argjson shards "$shards" --arg scpu "$server_cpus" \
  --arg lcpu "$loadgen_cpus" --argjson cpu "$cpu_json" \
  --arg wall "$(awk -v a="$t0" -v b="$t1" 'BEGIN{printf "%.1f", b-a}')" \
  --arg commit "$(git -C "$ROOT" rev-parse --short HEAD)" \
  --arg when "$(date -u +%FT%TZ)" \
  '. + {store: $store, shards: $shards, server_cpus: $scpu, loadgen_cpus: $lcpu,
        cpu_seconds: $cpu, wall_s: ($wall|tonumber), commit: $commit, at: $when}' \
  <<<"$result" >"$OUT/$name.json"
jq -c '{label, store, shards, qps: .total.qps, p50: .total.p50_us, p99: .total.p99_us, p999: .total.p999_us, errors: .error_replies, failed: .failed_conns, cpu: .cpu_seconds}' "$OUT/$name.json"
