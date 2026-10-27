"""HE0 driver for SigNoz v0.145.0 (plan HE1 Task 5): synthetic telemetry only.

Sends OTLP/HTTP traces (parent and child spans, errors, attributes), logs
(severities, attributes, trace links) and metrics (gauge, sum, histogram)
through the collector, registers the first admin, then runs the query
service's builder queries over each signal and request type (raw, time
series, scalar, table), with filters, group-by, having, order and limits,
plus the services, top-operations and field-suggestion reads the UI makes.
With HE0_SIGNOZ_RESULTS set, each request's status and a summary of its
answer (how many values it holds, and the names in it: services, span names,
attribute keys, severities) are written there as JSON, so a run on the House
can be compared with the oracle's (`compare.py`, HE1 R9.64). Standard library
only.
"""

import json
import os
import random
import re
import sys
import time
import urllib.error
import urllib.parse
import urllib.request

API = os.environ.get("HE0_SIGNOZ_URL", f"http://127.0.0.1:{os.environ.get('HE0_SIGNOZ_PORT', '3500')}")
OTLP = os.environ.get("HE0_SIGNOZ_OTLP", f"http://127.0.0.1:{os.environ.get('HE0_SIGNOZ_OTLP_PORT', '3518')}")
random.seed(75)
failures = []
results = []
# Strings that differ between runs: ids, times, numbers, UUIDs.
VOLATILE = re.compile(r"^([0-9a-f]{16}|[0-9a-f]{32}|[0-9a-f-]{36}|-?[0-9.e+-]+|\d{4}-\d\d-\d\dT.*|.*\d{10,}.*)$")


def summarize(body):
    """How many scalar values an answer holds, and its stable strings."""
    values, names = 0, set()

    def walk(v):
        nonlocal values
        if isinstance(v, dict):
            for k, x in v.items():
                walk(x)
        elif isinstance(v, list):
            for x in v:
                walk(x)
        elif v is not None:
            values += 1
            if isinstance(v, str) and v and not VOLATILE.match(v) and len(v) < 80:
                names.add(v)
    walk(body)
    return {"values": values, "names": sorted(names)}


def call(method, url, body=None, token=None, params=None, expect_ok=True, label=None):
    label = label or f"{method} {urllib.parse.urlparse(url).path}"
    status, answer = _call(method, url, body, token, params, expect_ok)
    if "/api/" in url:
        data = answer.get("data") if isinstance(answer, dict) else answer
        results.append({"label": label, "status": status, **summarize(data)})
    return status, answer


def _call(method, url, body=None, token=None, params=None, expect_ok=True):
    if params:
        url += "?" + urllib.parse.urlencode(params)
    data = json.dumps(body).encode() if body is not None else None
    h = {"Content-Type": "application/json"}
    if token:
        h["Authorization"] = f"Bearer {token}"
    req = urllib.request.Request(url, data=data, method=method, headers=h)
    try:
        with urllib.request.urlopen(req, timeout=60) as r:
            text = r.read().decode(errors="replace")
            try:
                return r.status, json.loads(text) if text else None
            except ValueError:
                return r.status, text
    except urllib.error.HTTPError as e:
        if expect_ok:
            failures.append(f"{method} {url[:120]}: {e.code} {e.read()[:300]!r}")
        return e.code, None
    except urllib.error.URLError as e:
        if expect_ok:
            failures.append(f"{method} {url[:120]}: {e}")
        return 0, None


def wait(url):
    for _ in range(180):
        try:
            with urllib.request.urlopen(url, timeout=5):
                return
        except urllib.error.HTTPError:
            return
        except Exception:
            time.sleep(2)
    sys.exit(f"driver: signoz: {url} did not answer")


def attr(k, v):
    if isinstance(v, bool):
        return {"key": k, "value": {"boolValue": v}}
    if isinstance(v, int):
        return {"key": k, "value": {"intValue": str(v)}}
    if isinstance(v, float):
        return {"key": k, "value": {"doubleValue": v}}
    return {"key": k, "value": {"stringValue": v}}


def resource(service):
    return {"attributes": [attr("service.name", service), attr("deployment.environment", "he0"), attr("host.name", "he0-host")]}


def send_telemetry(now_ns):
    services = ["frontend", "checkout", "payments"]
    spans_by_service = {s: [] for s in services}
    logs_by_service = {s: [] for s in services}
    for t in range(60):
        trace_id = "%032x" % random.getrandbits(128)
        root = "%016x" % random.getrandbits(64)
        start = now_ns - random.randint(1, 1800) * 10**9
        dur = random.randint(2, 900) * 10**6
        err = t % 9 == 0
        spans_by_service["frontend"].append({
            "traceId": trace_id, "spanId": root, "name": random.choice(["GET /", "GET /cart", "POST /checkout"]),
            "kind": 2, "startTimeUnixNano": str(start), "endTimeUnixNano": str(start + dur),
            "attributes": [attr("http.method", "GET"), attr("http.route", "/cart"), attr("http.status_code", 500 if err else 200), attr("user.tier", random.choice(["free", "pro"]))],
            "status": {"code": 2 if err else 1},
        })
        child = "%016x" % random.getrandbits(64)
        svc = random.choice(services[1:])
        spans_by_service[svc].append({
            "traceId": trace_id, "spanId": child, "parentSpanId": root, "name": "db.query",
            "kind": 3, "startTimeUnixNano": str(start + 10**6), "endTimeUnixNano": str(start + dur // 2),
            "attributes": [attr("db.system", "postgresql"), attr("db.statement", "SELECT 1"), attr("peer.service", "postgres")],
            "events": [{"name": "exception", "timeUnixNano": str(start + 2 * 10**6), "attributes": [attr("exception.type", "Timeout"), attr("exception.message", "deadline")]}] if err else [],
            "status": {"code": 2 if err else 0},
        })
        for k in range(2):
            logs_by_service[svc].append({
                "timeUnixNano": str(start + k * 10**6), "severityNumber": 17 if err else 9, "severityText": "ERROR" if err else "INFO",
                "body": {"stringValue": f"request {t} step {k}"}, "traceId": trace_id, "spanId": child,
                "attributes": [attr("code.function", "handle"), attr("http.status_code", 500 if err else 200), attr("latency_ms", dur / 1e6)],
            })
    traces = {"resourceSpans": [{"resource": resource(s), "scopeSpans": [{"scope": {"name": "he0"}, "spans": sp}]} for s, sp in spans_by_service.items()]}
    logs = {"resourceLogs": [{"resource": resource(s), "scopeLogs": [{"scope": {"name": "he0"}, "logRecords": lr}]} for s, lr in logs_by_service.items()]}
    points = []
    hist = []
    for i in range(30):
        ts = str(now_ns - (30 - i) * 60 * 10**9)
        points.append({"timeUnixNano": ts, "asDouble": random.random() * 100, "attributes": [attr("cpu", f"cpu{i % 4}")]})
        hist.append({"timeUnixNano": ts, "startTimeUnixNano": str(int(ts) - 60 * 10**9), "count": "10", "sum": 55.0,
                     "bucketCounts": ["1", "3", "4", "2", "0"], "explicitBounds": [1.0, 5.0, 10.0, 50.0], "attributes": [attr("route", "/cart")]})
    metrics = {"resourceMetrics": [{"resource": resource("checkout"), "scopeMetrics": [{"scope": {"name": "he0"}, "metrics": [
        {"name": "he0.cpu.usage", "unit": "%", "gauge": {"dataPoints": points}},
        {"name": "he0.requests", "unit": "1", "sum": {"aggregationTemporality": 2, "isMonotonic": True, "dataPoints": [dict(p, asDouble=float(i * 10)) for i, p in enumerate(points)]}},
        {"name": "he0.latency", "unit": "ms", "histogram": {"aggregationTemporality": 2, "dataPoints": hist}},
    ]}]}]}
    for path, body in (("/v1/traces", traces), ("/v1/logs", logs), ("/v1/metrics", metrics)):
        call("POST", OTLP + path, body)


def main():
    wait(API + "/api/v2/healthz")
    wait(OTLP + "/v1/traces")
    now_ms = int(time.time() * 1000)
    send_telemetry(now_ms * 10**6)
    email, password = "he0@example.com", "He0-password-he0!"
    call("POST", API + "/api/v1/register", {"name": "he0", "orgName": "he0", "email": email, "password": password}, expect_ok=False)
    _, ctx = call("GET", API + "/api/v2/sessions/context", params={"email": email, "ref": API})
    org = ((ctx or {}).get("data") or {}).get("orgs", [{}])[0].get("id")
    _, login = call("POST", API + "/api/v2/sessions/email_password", {"email": email, "password": password, "orgId": org})
    token = ((login or {}).get("data") or {}).get("accessToken")
    if not token:
        sys.exit(f"driver: signoz: no token: {failures}")
    time.sleep(25)  # the collector's batch and span-metrics flush
    start, end = now_ms - 3600 * 1000, int(time.time() * 1000) + 60000

    def key(name, ctx=None, dtype=None):
        k = {"name": name}
        if ctx:
            k["fieldContext"] = ctx
        if dtype:
            k["fieldDataType"] = dtype
        return k

    def q(signal, request_type, aggs=None, filt=None, group=None, order=None, limit=None, having=None, select=None, step=60, source=None):
        spec = {"signal": signal, "name": "A", "stepInterval": step}
        if source:
            spec["source"] = source
        if aggs:
            spec["aggregations"] = aggs
        if filt:
            spec["filter"] = {"expression": filt}
        if group:
            spec["groupBy"] = group
        if order:
            spec["order"] = order
        if limit:
            spec["limit"] = limit
        if having:
            spec["having"] = {"expression": having}
        if select:
            spec["selectFields"] = select
        body = {"schemaVersion": "v1", "start": start, "end": end, "requestType": request_type,
                "compositeQuery": {"queries": [{"type": "builder_query", "spec": spec}]}, "formatOptions": {"formatTableResultForUI": request_type == "table"}, "noCache": True}
        call("POST", API + "/api/v5/query_range", body, token, expect_ok=False,
             label=f"query_range {signal} {request_type} {json.dumps(spec, sort_keys=True)}")

    svc = [key("service.name", "resource", "string")]
    for sig in ("traces", "logs"):
        q(sig, "raw", limit=20, order=[{"key": key("timestamp"), "direction": "desc"}])
        q(sig, "raw", limit=10, filt="service.name = 'checkout' AND http.status_code >= 500")
        q(sig, "time_series", aggs=[{"expression": "count()"}], group=svc)
        q(sig, "scalar", aggs=[{"expression": "count()"}, {"expression": "count_distinct(trace_id)"}], group=svc, order=[{"key": key("count()"), "direction": "desc"}], having="count() > 1")
        q(sig, "table", aggs=[{"expression": "count()"}], group=svc + [key("http.status_code", "attribute", "int64")], limit=10)
    q("traces", "time_series", aggs=[{"expression": "p99(duration_nano)"}, {"expression": "avg(duration_nano)"}], group=svc, filt="has_error = true OR name CONTAINS 'cart'")
    q("traces", "scalar", aggs=[{"expression": "rate()"}, {"expression": "countIf(has_error = true)"}], group=[key("name", "span")])
    q("traces", "raw", limit=10, filt="user.tier IN ('pro') AND name EXISTS", select=[key("name", "span"), key("duration_nano", "span"), key("user.tier", "attribute", "string")])
    q("logs", "time_series", aggs=[{"expression": "count()"}], filt="severity_text = 'ERROR' AND body CONTAINS 'request'", group=[key("severity_text", "log")])
    q("logs", "raw", limit=10, filt="body REGEXP 'step [0-9]' AND latency_ms > 1")
    for metric, temporality, tagg, sagg in (("he0.cpu.usage", "unspecified", "avg", "max"), ("he0.requests", "cumulative", "rate", "sum"), ("he0.latency", "cumulative", "increase", "p90")):
        call("POST", API + "/api/v5/query_range", {"schemaVersion": "v1", "start": start, "end": end, "requestType": "time_series",
             "compositeQuery": {"queries": [{"type": "builder_query", "spec": {"signal": "metrics", "name": "A", "stepInterval": 60,
              "aggregations": [{"metricName": metric, "timeAggregation": tagg, "spaceAggregation": sagg, "temporality": temporality}],
              "groupBy": svc}}]}, "noCache": True}, token, expect_ok=False)
    call("POST", API + "/api/v5/query_range", {"schemaVersion": "v1", "start": start, "end": end, "requestType": "time_series",
         "compositeQuery": {"queries": [{"type": "builder_query", "spec": {"signal": "traces", "name": "A", "stepInterval": 60, "aggregations": [{"expression": "count()"}]}},
                                        {"type": "builder_query", "spec": {"signal": "traces", "name": "B", "stepInterval": 60, "aggregations": [{"expression": "countIf(has_error = true)"}]}},
                                        {"type": "builder_formula", "spec": {"name": "F1", "expression": "B / A * 100"}}]}, "noCache": True}, token, expect_ok=False)
    call("POST", API + "/api/v5/query_range", {"schemaVersion": "v1", "start": start, "end": end, "requestType": "time_series",
         "compositeQuery": {"queries": [{"type": "clickhouse_sql", "spec": {"name": "A", "query": "SELECT toStartOfMinute(timestamp) AS ts, count() AS value FROM signoz_traces.distributed_signoz_index_v3 WHERE timestamp > now() - INTERVAL 1 HOUR GROUP BY ts ORDER BY ts"}}]}}, token, expect_ok=False)
    ns = {"start": str(start * 10**6), "end": str(end * 10**6)}
    call("POST", API + "/api/v1/services", ns, token, expect_ok=False)
    call("POST", API + "/api/v1/service/top_operations", {**ns, "service": "frontend"}, token, expect_ok=False)
    call("POST", API + "/api/v1/service/top_level_operations", ns, token, expect_ok=False)
    for sig in ("traces", "logs", "metrics"):
        call("GET", API + "/api/v1/fields/keys", token=token, params={"signal": sig, "startUnixMilli": start, "endUnixMilli": end, "searchText": ""}, expect_ok=False)
        call("GET", API + "/api/v1/fields/values", token=token, params={"signal": sig, "name": "service.name", "startUnixMilli": start, "endUnixMilli": end}, expect_ok=False)
    call("GET", API + "/api/v1/traces/" + "0" * 32, token=token, expect_ok=False)
    call("POST", API + "/api/v2/traces/waterfall/" + "0" * 32, {}, token, expect_ok=False)
    call("GET", API + "/api/v1/metrics", token=token, params={"start": start, "end": end, "limit": 10}, expect_ok=False)
    call("POST", API + "/api/v1/metrics", {"start": start, "end": end, "limit": 10, "offset": 0, "orderBy": {"columnName": "samples", "order": "desc"}}, token, expect_ok=False)
    if os.environ.get("HE0_SIGNOZ_RESULTS"):
        with open(os.environ["HE0_SIGNOZ_RESULTS"], "w") as f:
            json.dump(results, f, indent=1)
    if failures:
        print("driver: signoz: requests that failed:\n  " + "\n  ".join(failures), file=sys.stderr)
    print("driver: signoz done", file=sys.stderr)


if __name__ == "__main__":
    main()
