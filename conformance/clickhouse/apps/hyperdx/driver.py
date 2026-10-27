"""HE0 driver for HyperDX 2.40.0 (plan HE1 Task 5): synthetic telemetry only.

Sends OTLP/HTTP traces, logs and metrics through the ClickStack collector,
registers a user, then runs the queries HyperDX renders on the server: the
external API's chart series (each aggregation, granularity, group-by and
where language, over logs, traces and metrics) and search. The dashboard's
own queries are built in the browser and sent through /clickhouse-proxy, so
they are not driven here (HE0 Task 5 ruling). Standard library only.
"""

import http.cookiejar
import json
import os
import random
import sys
import time
import urllib.error
import urllib.parse
import urllib.request

API = os.environ.get("HE0_HYPERDX_URL", f"http://127.0.0.1:{os.environ.get('HE0_HYPERDX_PORT', '3600')}")
OTLP = os.environ.get("HE0_HYPERDX_OTLP", f"http://127.0.0.1:{os.environ.get('HE0_HYPERDX_OTLP_PORT', '3618')}")
random.seed(75)
failures = []
# Requests the application answered with an error; reported, not fatal.
refused = []
jar = http.cookiejar.CookieJar()
opener = urllib.request.build_opener(urllib.request.HTTPCookieProcessor(jar))


INGEST_KEY = None


def call(method, url, body=None, token=None, params=None, expect_ok=True, form=False):
    if params:
        url += "?" + urllib.parse.urlencode(params)
    h = {}
    if form:
        data = urllib.parse.urlencode(body).encode()
        h["Content-Type"] = "application/x-www-form-urlencoded"
    else:
        data = json.dumps(body).encode() if body is not None else None
        h["Content-Type"] = "application/json"
    if token:
        h["Authorization"] = f"Bearer {token}"
    elif url.startswith(OTLP) and INGEST_KEY:
        h["Authorization"] = INGEST_KEY
    req = urllib.request.Request(url, data=data, method=method, headers=h)
    try:
        with opener.open(req, timeout=60) as r:
            text = r.read().decode(errors="replace")
            try:
                return r.status, json.loads(text) if text else None
            except ValueError:
                return r.status, text
    except urllib.error.HTTPError as e:
        (failures if expect_ok else refused).append(f"{method} {url[:120]}: {e.code} {e.read()[:300]!r}")
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
    sys.exit(f"driver: hyperdx: {url} did not answer")


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
    wait(API + "/health")
    email, password = "he0@example.com", "He0-password-he0!"
    call("POST", API + "/register/password", {"email": email, "password": password, "confirmPassword": password}, expect_ok=False)
    call("POST", API + "/login/password", {"email": email, "password": password}, form=True, expect_ok=False)
    _, me = call("GET", API + "/me")
    key = (me or {}).get("accessKey")
    if not key:
        sys.exit(f"driver: hyperdx: no access key: {failures}")
    # The collector gets its OTLP receivers from the app over OpAMP once a
    # team exists, and takes the team's ingestion key.
    _, team = call("GET", API + "/team")
    global INGEST_KEY
    INGEST_KEY = (team or {}).get("apiKey")
    wait(OTLP + "/v1/traces")
    now_ms = int(time.time() * 1000)
    send_telemetry(now_ms * 10**6)
    time.sleep(20)
    _, sources = call("GET", API + "/api/v2/sources", token=key)
    by_kind = {s.get("kind"): s.get("id") for s in ((sources or {}).get("data") or [])}
    start, end = now_ms - 3600 * 1000, int(time.time() * 1000) + 60000
    iso = lambda ms: time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(ms / 1000))

    def series(sid, items, granularity=None, group=None, ret=None):
        body = {"startTime": start, "endTime": end, "series": [{"where": "", **s, "sourceId": sid, "groupBy": group or []} for s in items]}
        if granularity:
            body["granularity"] = granularity
        if ret:
            body["seriesReturnType"] = ret
        call("POST", API + "/api/v2/charts/series", body, key, expect_ok=False)

    for kind in ("log", "trace"):
        sid = by_kind.get(kind)
        if not sid:
            failures.append(f"no {kind} source")
            continue
        series(sid, [{"aggFn": "count"}])
        series(sid, [{"aggFn": "count"}], granularity="1m", group=["ServiceName"])
        series(sid, [{"aggFn": "count_distinct", "field": "TraceId"}], granularity="5m")
        series(sid, [{"aggFn": "count", "where": "SeverityText:ERROR" if kind == "log" else "StatusCode:Error"}, {"aggFn": "count"}], ret="ratio")
        series(sid, [{"aggFn": "count", "where": "ServiceName = 'checkout'", "whereLanguage": "sql"}], granularity="auto")
        for search in ("", "checkout", "ServiceName:frontend AND -SeverityText:INFO"):
            call("POST", API + "/api/v2/search", {"sourceId": sid, "startTime": iso(start), "endTime": iso(end), "where": search}, key, expect_ok=False)
        call("POST", API + "/api/v2/search", {"sourceId": sid, "startTime": iso(start), "endTime": iso(end), "where": "ServiceName = 'payments'", "whereLanguage": "sql", "limit": 5}, key, expect_ok=False)
    tsid = by_kind.get("trace")
    if tsid:
        for fn in ("avg", "max", "min", "sum", "any", "last_value"):
            series(tsid, [{"aggFn": fn, "field": "Duration"}], granularity="1m", group=["SpanName"])
        for level in (0.5, 0.9, 0.95, 0.99):
            series(tsid, [{"aggFn": "quantile", "level": level, "field": "Duration"}], granularity="1m", group=["SpanName"])
    msid = by_kind.get("metric")
    if msid:
        series(msid, [{"aggFn": "avg", "metricName": "he0.cpu.usage", "metricDataType": "gauge"}], granularity="1m")
        series(msid, [{"aggFn": "increase", "metricName": "he0.requests", "metricDataType": "sum"}], granularity="5m", group=["ServiceName"])
        series(msid, [{"aggFn": "quantile", "level": 0.95, "metricName": "he0.latency", "metricDataType": "histogram"}], granularity="1m")
    call("GET", API + "/api/v2/dashboards", token=key, expect_ok=False)
    call("GET", API + "/api/v2/alerts", token=key, expect_ok=False)
    if refused:
        print(f"driver: hyperdx: {len(refused)} requests answered with an error:\n  " + "\n  ".join(refused), file=sys.stderr)
    if failures:
        print("driver: hyperdx: requests that failed:\n  " + "\n  ".join(failures), file=sys.stderr)
    print("driver: hyperdx done", file=sys.stderr)


if __name__ == "__main__":
    main()
