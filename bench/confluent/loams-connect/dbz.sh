#!/usr/bin/env bash
# The Debezium-decoded source (#1303), when up.sh ran with CFB_DEBEZIUM=1:
# writes the repository's Debezium fixtures (Postgres create/update/delete,
# JSON converter) to $CFB_TOPIC_DBZ and prints the change events that land
# in the Loams stream ${CFB_STREAM}-dbz (type, loamsop, loamslsn, data).
set -euo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
pid_alive "$CFB_RUN/engine.pid" || die "the engine is not running (up.sh)"
[[ -f $CFB_RUN/instances/cfb-dbz.yaml ]] || die "no cfb-dbz instance: run up.sh with CFB_DEBEZIUM=1"
write_client_config
FX="$REPO/fabric/crates/loams-connector-kafka/tests/fixtures/debezium"
compact() { python3 -I -c 'import json,sys;print(json.dumps(json.load(open(sys.argv[1])),separators=(",",":")))' "$1"; }
key=$(compact "$FX/pg-key.json")
for f in pg-create pg-update pg-delete; do
  printf '%s\t%s\n' "$key" "$(compact "$FX/$f.json")"
done | kafka_tool kafka-console-producer.sh --topic "$CFB_TOPIC_DBZ" \
  --reader-property parse.key=true --reader-property key.separator=$'\t' >/dev/null
log "sent 3 Debezium events to $CFB_TOPIC_DBZ"
export ENGINE_URL ENGINE_KEY_FILE="$SECRETS/engine-key"
python3 -I - "$CFB_NAMESPACE" "${CFB_STREAM}-dbz" <<'EOF'
import base64, json, os, sys, time, urllib.request
ns, stream = sys.argv[1:3]
key = open(os.environ["ENGINE_KEY_FILE"]).read().strip()
def fetch():
    req = urllib.request.Request(os.environ["ENGINE_URL"] + "/loams.stream.v1.StreamService/FetchCloudEvents",
        data=json.dumps({"namespace": ns, "stream": stream, "partition": 0, "offset": "0", "maxWaitMs": 1000}).encode(),
        headers={"content-type": "application/json", "connect-protocol-version": "1", "authorization": "Bearer " + key})
    return json.load(urllib.request.urlopen(req, timeout=10))
evs = []
for _ in range(30):
    try:
        evs = json.loads(base64.b64decode(fetch().get("jsonBatch", "")) or b"[]")
    except Exception as e:
        print("fetch:", e, file=sys.stderr)
    if len(evs) >= 3:
        break
    time.sleep(1)
for e in evs:
    print(json.dumps({k: e.get(k) for k in ("type", "source", "id", "loamsop", "loamslsn", "subject", "data")}))
sys.exit(0 if len(evs) >= 3 else 1)
EOF
