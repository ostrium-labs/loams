#!/usr/bin/env bash
# HE0 driver for Langfuse (v3 and v4 share the public API): synthetic traffic
# only (plan HE1, "No query text from real users"). It ingests traces,
# observations and scores through the public ingestion API, waits for the
# worker to write them to ClickHouse, then reads them back through the public
# read endpoints, which is what sends Langfuse's SELECTs.
#
#   HE0_LANGFUSE_URL   default http://127.0.0.1:${HE0_LANGFUSE_PORT:-3100}
#   HE0_TRACES         traces to ingest (default 50)
set -euo pipefail

URL=${HE0_LANGFUSE_URL:-http://127.0.0.1:${HE0_LANGFUSE_PORT:-3100}}
AUTH="pk-lf-he0:sk-lf-he0" # LANGFUSE_INIT_PROJECT_*_KEY in compose.yaml
TRACES=${HE0_TRACES:-50}

api() { curl -fsS -u "$AUTH" -H 'content-type: application/json' "$@"; }

for _ in $(seq 1 120); do
  curl -fsS "$URL/api/public/health" >/dev/null 2>&1 && break
  sleep 2
done
curl -fsS "$URL/api/public/health" >/dev/null

# Traces and observations go in as OTLP (JSON), the path both lines take and
# the only one v4's default `events_only` write mode stores; v3 also gets the
# legacy ingestion batch. Per trace: a span, a generation with usage, two
# scores; every tenth trace is sent again (ReplacingMergeTree versions).
payloads=$(python3 -I - "$TRACES" <<'PY'
import json, sys, time, hashlib
n = int(sys.argv[1])
now = time.time_ns()
def hexid(s, size):
    return hashlib.sha256(s.encode()).hexdigest()[:size]
def attr(k, v):
    if isinstance(v, bool):
        return {"key": k, "value": {"boolValue": v}}
    if isinstance(v, int):
        return {"key": k, "value": {"intValue": str(v)}}
    if isinstance(v, list):
        return {"key": k, "value": {"arrayValue": {"values": [{"stringValue": x} for x in v]}}}
    return {"key": k, "value": {"stringValue": v}}
spans, scores, legacy, ids = [], [], [], []
for i in range(n):
    t = hexid(f"he0-trace-{i}", 32)
    ids.append(t)
    root, gen = hexid(f"he0-span-{i}", 16), hexid(f"he0-gen-{i}", 16)
    start = now - (120 + i) * 10**9
    common = [attr("langfuse.trace.name", f"he0-{i % 5}"), attr("user.id", f"user-{i % 7}"),
              attr("session.id", f"session-{i % 4}"), attr("langfuse.trace.tags", ["he0", f"t{i % 3}"]),
              attr("langfuse.release", "he0")]
    spans.append({"traceId": t, "spanId": root, "name": "retrieve", "kind": 1,
                  "startTimeUnixNano": str(start), "endTimeUnixNano": str(start + 10**9),
                  "attributes": common + [attr("langfuse.observation.type", "span"),
                                          attr("langfuse.observation.input", '{"q":"synthetic"}')]})
    spans.append({"traceId": t, "spanId": gen, "parentSpanId": root, "name": "answer", "kind": 1,
                  "startTimeUnixNano": str(start + 10**8), "endTimeUnixNano": str(start + 9 * 10**8),
                  "attributes": common + [attr("langfuse.observation.type", "generation"),
                                          attr("gen_ai.request.model", "gpt-4o-mini"),
                                          attr("gen_ai.usage.input_tokens", 100 + i),
                                          attr("gen_ai.usage.output_tokens", 20 + i)]})
    if i % 10 == 0:
        spans.append(dict(spans[-2], endTimeUnixNano=str(start + 2 * 10**9)))
    scores.append({"traceId": t, "name": "quality", "value": (i % 10) / 10, "dataType": "NUMERIC"})
    scores.append({"traceId": t, "observationId": gen, "name": "label",
                   "value": "good" if i % 2 else "bad", "dataType": "CATEGORICAL"})
    legacy.append({"id": f"he0-legacy-{i}", "type": "trace-create",
                   "timestamp": "%sZ" % time.strftime("%Y-%m-%dT%H:%M:%S", time.gmtime()),
                   "body": {"id": f"he0-legacy-{i}", "name": "he0-legacy", "userId": f"user-{i % 7}"}})
otlp = {"resourceSpans": [{"resource": {"attributes": [attr("service.name", "he0")]},
                           "scopeSpans": [{"scope": {"name": "he0"}, "spans": spans}]}]}
print(json.dumps({"otlp": otlp, "scores": scores, "legacy": {"batch": legacy}, "ids": ids}))
PY
)
field() { python3 -I -c 'import json,sys; v=json.loads(sys.stdin.read())[sys.argv[1]]; print(json.dumps(v) if not isinstance(v, str) else v)' "$1" <<<"$payloads"; }

api -X POST "$URL/api/public/otel/v1/traces" --data "$(field otlp)" >/dev/null
while IFS= read -r score; do
  api -X POST "$URL/api/public/scores" --data "$score" >/dev/null
done < <(python3 -I -c 'import json,sys; [print(json.dumps(s)) for s in json.loads(sys.stdin.read())["scores"]]' <<<"$payloads")
if [ "${HE0_APP:-langfuse-v4}" = langfuse-v3 ]; then
  api -X POST "$URL/api/public/ingestion" --data "$(field legacy)" >/dev/null
fi
mapfile -t ids < <(python3 -I -c 'import json,sys; [print(i) for i in json.loads(sys.stdin.read())["ids"]]' <<<"$payloads")

# The worker writes in batches; wait until the last trace's observations read
# back (v4's events_only mode serves only the v2 read API).
# v3 reads OTLP/JSON span ids as protobuf JSON bytes (base64), so the hex ids
# sent above are not the ids it stores; v3 reads back and opens the traces of
# its legacy batch instead (he0-legacy-<i>), which it stores as sent.
if [ "${HE0_APP:-langfuse-v4}" = langfuse-v3 ]; then
  for i in "${!ids[@]}"; do ids[i]="he0-legacy-$i"; done
fi
last=${ids[$((TRACES - 1))]}
# v4 serves observations v2 (events_only); v3 has neither that endpoint nor
# observations for a legacy trace, so it reads the trace itself.
if [ "${HE0_APP:-langfuse-v4}" = langfuse-v3 ]; then
  seen() { api "$URL/api/public/traces/$last" 2>/dev/null | grep -q '"id"'; }
else
  seen() { api "$URL/api/public/v2/observations?limit=1&traceId=$last" 2>/dev/null | grep -q '"traceId"'; }
fi
for _ in $(seq 1 90); do
  seen && break
  sleep 2
done
seen || { echo "driver: trace $last never read back" >&2; exit 1; }

from=$(date -u -d '-1 day' +%Y-%m-%dT%H:%M:%SZ)
reads=(
  "/api/public/traces?limit=50"
  "/api/public/traces?limit=10&userId=user-1&tags=he0&orderBy=timestamp.desc"
  "/api/public/traces?limit=10&sessionId=session-2&fromTimestamp=$from"
  "/api/public/traces/${ids[0]}"
  "/api/public/observations?limit=50"
  "/api/public/observations?limit=10&type=GENERATION&name=answer"
  "/api/public/v2/observations?limit=50"
  "/api/public/v2/observations?limit=10&type=GENERATION&name=answer"
  "/api/public/v2/observations?limit=10&traceId=${ids[1]}"
  "/api/public/v2/observations?limit=10&userId=user-1&fromStartTime=$from"
  "/api/public/scores?limit=50"
  "/api/public/v2/scores?limit=50"
  "/api/public/v2/scores?limit=10&name=quality&dataType=NUMERIC"
  "/api/public/sessions?limit=10"
  "/api/public/sessions/session-1"
  "/api/public/metrics/daily?limit=7"
  "/api/public/metrics/daily?limit=7&traceName=he0-1"
)
ran=0
for path in "${reads[@]}"; do
  code=$(curl -sS -o /dev/null -w '%{http_code}' -u "$AUTH" "$URL$path")
  case $code in
    200) ran=$((ran + 1)) ;;
    404 | 405 | 410) echo "driver: GET $path: $code (not in this version)" >&2 ;;
    *) echo "driver: GET $path failed: $code" >&2; exit 1 ;;
  esac
done

# Deletes produce Langfuse's mutations (ALTER ... DELETE) on its tables.
code=$(curl -sS -o /dev/null -w '%{http_code}' -u "$AUTH" -X DELETE "$URL/api/public/traces/${ids[$((TRACES - 2))]}")
echo "driver: DELETE trace: $code" >&2
echo "driver: ${HE0_APP:-langfuse} ingested $TRACES traces and ran $ran reads"

# The UI's own queries: every main page and a trace, session and user detail
# page in a headless browser (HE0 Task 3, pages.sh). HE0_PAGES=off skips it
# (no Node or browser on the machine).
if [ "${HE0_PAGES:-on}" != off ]; then
  HE0_LANGFUSE_URL=$URL HE0_DETAIL="${ids[0]},session-1,user-1" bash "$(dirname "${BASH_SOURCE[0]}")/pages.sh"
fi
