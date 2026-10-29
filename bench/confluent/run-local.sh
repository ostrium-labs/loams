#!/usr/bin/env bash
# LOCAL comparison baseline (not cloud): the same driver and scenarios against
# Apache Kafka 4.3.1 (container, KRaft, one node) and the Loams Iggy Kafka gateway
# (release build of ostrium-labs/iggy `loams`), both on this laptop over loopback.
#
#   run-local.sh kafka|iggy [latency|txn|tput|all]
#
# Kafka 4.3.1:  podman run ... docker.io/apache/kafka:4.3.1 on 127.0.0.1:39092 (see README)
# Iggy gateway: iggy-wt/cfb-baseline/cfb-run.sh up      on 127.0.0.1:39093
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
py="$here/.venv/bin/python"
day="${CFB_DAY:-2026-10-28}"
export OTEL_EXPORTER_OTLP_ENDPOINT="${OTEL_EXPORTER_OTLP_ENDPOINT:-http://127.0.0.1:4318}"
case "${1:?kafka|iggy}" in
  kafka) target=apache-kafka-4.3.1-local kind=apache-kafka-local bs="${CFB_KAFKA_BOOTSTRAP:-127.0.0.1:39092}" ;;
  iggy)  target=iggy-kafka-gateway-local kind=iggy-kafka-local  bs="${CFB_IGGY_BOOTSTRAP:-127.0.0.1:39093}" ;;
  *) echo "kafka|iggy" >&2; exit 2 ;;
esac
what="${2:-all}"
res="$here/results/$day/local"
drv() { local out="$1"; shift
  "$py" "$here/driver.py" --target "$target" --kind "$kind" --bootstrap "$bs" --cloud local --region laptop \
    --run "$(date +%H%M%S)" --out "$out" "$@" 2>>"${CFB_STATE:-$HOME/.cfb-state}/driver-stderr.log" | cut -c1-400; }
if [[ "$what" == latency || "$what" == all ]]; then
  for size in 100 1024 10240; do for acks in 1 all; do
    # the same low rate as the WAN matrix, so the latency columns compare like for like
    drv "$res/latency/$target-s$size-a$acks.json" --partitions 3 latency --size "$size" --acks "$acks" --rate 10 --seconds 60
  done; done
fi
if [[ "$what" == txn || "$what" == all ]]; then
  drv "$res/txn/$target.json" --partitions 3 txn --txns 60 --msgs-per-txn 10 --size 512 --abort-every 5
fi
if [[ "$what" == tput || "$what" == all ]]; then
  for size in 1024 16384; do
    drv "$res/tput/$target-s$size.json" --partitions 6 throughput --size "$size" --acks all --seconds 30 --max-mb 4096
  done
fi
