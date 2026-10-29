#!/usr/bin/env bash
# The WAN matrix from the laptop driver against every live benchmark cluster.
#
#   run-wan.sh rtt      [CLUSTERS...]   TCP / TLS / Kafka metadata round trips
#   run-wan.sh latency  [CLUSTERS...]   produce + end-to-end latency over sizes x acks (low rate)
#   run-wan.sh txn      [CLUSTERS...]   transactions / EOS (read_committed sees no aborted records)
#   run-wan.sh tput     [CLUSTERS...]   how much this uplink can push (bounded by --max-mb)
#   run-wan.sh parallel SCENARIO        the same scenario on every cluster at once
#
# Results: bench/confluent/results/$CFB_DAY/<scenario>/<cluster>-<params>.json
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
py="$here/.venv/bin/python"
day="${CFB_DAY:-2026-10-28}"
res="$here/results/$day"
state="${CFB_STATE:-$HOME/.cfb-state}"
export OTEL_EXPORTER_OTLP_ENDPOINT="${OTEL_EXPORTER_OTLP_ENDPOINT:-http://127.0.0.1:4318}"

clusters() {
  if (($#)); then printf '%s\n' "$@"; else awk -F'\t' '$1=="kafka-cluster"{print $3}' "$state/live.tsv"; fi
}

drv() {  # cluster outfile args...
  local c="$1" out="$2"; shift 2
  local d="$state/clusters/$c"
  local cloud region kind
  cloud="$(jq -r '.cloud|ascii_downcase' "$d/cluster.json")"; region="$(jq -r .region "$d/cluster.json")"
  kind="confluent-cloud-$(jq -r '.kind|ascii_downcase' "$d/cluster.json")"
  mkdir -p "$(dirname "$out")"
  "$py" "$here/driver.py" --target "$c" --kind "$kind" --bootstrap "$(jq -r .bootstrap "$d/cluster.json")" \
    --key-file "$d/key" --cloud "$cloud" --region "$region" --run "$(date +%H%M%S)" --out "$out" "$@" 2>>"$state/driver-stderr.log" \
    | cut -c1-400
}

scenario() {  # name cluster
  local s="$1" c="$2"
  case "$s" in
    rtt) drv "$c" "$res/rtt/$c.json" rtt --samples 30 --interval 0.5 ;;
    latency)
      local size acks
      for size in 100 1024 10240; do
        for acks in 1 all; do
          drv "$c" "$res/latency/$c-s$size-a$acks.json" --partitions 3 latency --size "$size" --acks "$acks" --rate 10 --seconds 60
        done
      done ;;
    txn) drv "$c" "$res/txn/$c.json" --partitions 3 txn --txns 60 --msgs-per-txn 10 --size 512 --abort-every 5 ;;
    tput) drv "$c" "$res/tput/$c.json" --partitions 6 throughput --size 16384 --acks all --seconds 60 --max-mb 48 ;;
    *) echo "unknown scenario $s" >&2; return 2 ;;
  esac
}

cmd="${1:?scenario}"; shift || true
if [[ "$cmd" == parallel ]]; then
  s="${1:?scenario}"; shift || true
  res="$res/parallel"
  pids=()
  for c in $(clusters "$@"); do
    (CFB_PAR=1 scenario "$s" "$c" | sed "s/^/[$c] /") &
    pids+=($!)
  done
  for p in "${pids[@]}"; do wait "$p" || true; done
else
  for c in $(clusters "$@"); do echo "== $cmd $c"; scenario "$cmd" "$c" || true; done
fi
