#!/usr/bin/env bash
# Cross-cloud Cluster Linking: a destination-initiated link on a Dedicated cluster
# pulling a topic from a cluster on another cloud, then the driver produces on the
# source and reads the mirror on the destination (end-to-end latency with OTel
# traces: kafka.produce on the source cloud, kafka.deliver across the link).
#
#   link.sh up   SRC DST TOPIC     create TOPIC on SRC, link SRC->DST, mirror TOPIC
#   link.sh run  SRC DST TOPIC     driver latency: produce on SRC, consume the mirror on DST
#   link.sh down SRC DST TOPIC     delete the mirror topic and the link
#
# Pricing (billing dimensions): $0.25 per link-hour on each side + $0.25/GB mirrored.
# The link's credentials (the SRC key) go into a 0600 request file, never argv.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib/cc.sh
source "$here/lib/cc.sh"
cmd="$1" src="$2" dst="$3" topic="$4"
S="$CFB_STATE/clusters/$src" D="$CFB_STATE/clusters/$dst"
link="cfb-${src#loams-cfb-}-to-${dst#loams-cfb-}"
dst_id="$(jq -r .id "$D/cluster.json")" dst_rest="$(jq -r .rest "$D/cluster.json")"
src_id="$(jq -r .id "$S/cluster.json")" src_bs="$(jq -r .bootstrap "$S/cluster.json")"
base="$dst_rest/kafka/v3/clusters/$dst_id"
day="${CFB_DAY:-2026-10-28}"
case "$cmd" in
  up)
    "$here/.venv/bin/python" - "$src_bs" "$S/key" "$topic" <<'EOF'
import sys
from confluent_kafka.admin import AdminClient, NewTopic
bs, kf, topic = sys.argv[1:4]
kv = dict(l.rstrip("\n").split("=", 1) for l in open(kf) if "=" in l)
a = AdminClient({"bootstrap.servers": bs, "security.protocol": "SASL_SSL", "sasl.mechanisms": "PLAIN",
                 "sasl.username": kv["id"], "sasl.password": kv["secret"]})
f = a.create_topics([NewTopic(topic, num_partitions=6, replication_factor=3, config={"retention.ms": "7200000"})])[topic]
try:
    f.result(120); print("created", topic)
except Exception as e:
    print("topic:", e)
EOF
    req="$CFB_STATE/link-$link.json"
    (umask 077; jq -n --arg sid "$src_id" --arg bs "$src_bs" \
      --arg jaas "org.apache.kafka.common.security.plain.PlainLoginModule required username=\"$(sed -n 's/^id=//p' "$S/key")\" password=\"$(sed -n 's/^secret=//p' "$S/key")\";" \
      '{source_cluster_id:$sid, configs:[{name:"bootstrap.servers",value:$bs},{name:"security.protocol",value:"SASL_SSL"},
        {name:"sasl.mechanism",value:"PLAIN"},{name:"sasl.jaas.config",value:$jaas}]}' >"$req")
    cc_keyed "$D/key" POST "$base/links?link_name=$link" "@$req" >/dev/null; echo "link $link: HTTP $(cc_status) $(head -c 300 "$CFB_STATE/last.body")"
    rm -f "$req"
    echo "$(date -u +%FT%TZ) link-up $link" >>"$CFB_STATE/events.log"
    for i in $(seq 1 10); do
      cc_keyed "$D/key" POST "$base/links/$link/mirrors" "$(jq -cn --arg t "$topic" '{source_topic_name:$t}')" >/dev/null
      st="$(cc_status)"; [[ "$st" == 20* ]] && break; sleep 15
    done
    echo "mirror $topic: HTTP $st $(head -c 300 "$CFB_STATE/last.body")"
    ;;
  run)
    out="$here/results/$day/link/$link.json"; mkdir -p "$(dirname "$out")"
    OTEL_EXPORTER_OTLP_ENDPOINT="${OTEL_EXPORTER_OTLP_ENDPOINT:-http://127.0.0.1:4318}" "$here/.venv/bin/python" "$here/driver.py" \
      --target "$link" --kind cluster-link --bootstrap "$src_bs" --key-file "$S/key" \
      --cloud "$(jq -r '.cloud|ascii_downcase' "$S/cluster.json")->$(jq -r '.cloud|ascii_downcase' "$D/cluster.json")" \
      --region "$(jq -r .region "$S/cluster.json")->$(jq -r .region "$D/cluster.json")" \
      --consume-bootstrap "$(jq -r .bootstrap "$D/cluster.json")" --consume-key-file "$D/key" \
      --topic "$topic" --keep-topic --trace-sample 0.05 --drain-s 120 --out "$out" \
      latency --size 1024 --acks all --rate 10 --seconds "${CFB_LINK_SECONDS:-180}" 2>>"$CFB_STATE/driver-stderr.log" | cut -c1-500
    ;;
  down)
    cc_keyed "$D/key" DELETE "$base/topics/$topic" >/dev/null; echo "mirror topic delete: HTTP $(cc_status)"
    cc_keyed "$D/key" DELETE "$base/links/$link?force=true" >/dev/null; echo "link delete: HTTP $(cc_status)"
    echo "$(date -u +%FT%TZ) link-down $link" >>"$CFB_STATE/events.log"
    ;;
esac
