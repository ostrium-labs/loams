"""HE0 driver for OpenLIT 2.1.0 (plan HE1 Task 5): synthetic telemetry only.

Sends OTLP/HTTP spans carrying OpenLIT's GenAI and vector-DB attributes and
error logs through the collector built into the image, signs in as the
default user, then opens the dashboard's reads: each LLM and vector-DB
metric route (requests, tokens, cost, models, providers, environments,
exceptions), the request list with sorting and paging, and a trace and span
view. Standard library only.
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

API = os.environ.get("HE0_OPENLIT_URL", f"http://127.0.0.1:{os.environ.get('HE0_OPENLIT_PORT', '3700')}")
OTLP = os.environ.get("HE0_OPENLIT_OTLP", f"http://127.0.0.1:{os.environ.get('HE0_OPENLIT_OTLP_PORT', '3718')}")
random.seed(75)
failures = []
refused = []
opener = urllib.request.build_opener(urllib.request.HTTPCookieProcessor(http.cookiejar.CookieJar()))


def call(method, url, body=None, form=False, expect_ok=True):
    h = {"Origin": API}
    if form:
        data = urllib.parse.urlencode(body).encode()
        h["Content-Type"] = "application/x-www-form-urlencoded"
    else:
        data = json.dumps(body).encode() if body is not None else None
        h["Content-Type"] = "application/json"
    req = urllib.request.Request(url, data=data, method=method, headers=h)
    try:
        with opener.open(req, timeout=60) as r:
            text = r.read().decode(errors="replace")
            try:
                return r.status, json.loads(text) if text else None
            except ValueError:
                return r.status, text
    except urllib.error.HTTPError as e:
        (failures if expect_ok else refused).append(f"{method} {url[:100]}: {e.code} {e.read()[:200]!r}")
        return e.code, None
    except urllib.error.URLError as e:
        failures.append(f"{method} {url[:100]}: {e}")
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
    sys.exit(f"driver: openlit: {url} did not answer")


def attr(k, v):
    if isinstance(v, bool):
        return {"key": k, "value": {"boolValue": v}}
    if isinstance(v, int):
        return {"key": k, "value": {"intValue": str(v)}}
    if isinstance(v, float):
        return {"key": k, "value": {"doubleValue": v}}
    return {"key": k, "value": {"stringValue": v}}


def send(now_ns):
    spans, logs = [], []
    models = [("openai", "gpt-4o"), ("anthropic", "claude-sonnet"), ("mistral", "mistral-large")]
    for i in range(80):
        trace_id = "%032x" % random.getrandbits(128)
        span_id = "%016x" % random.getrandbits(64)
        start = now_ns - random.randint(1, 3000) * 10**9
        dur = random.randint(100, 4000) * 10**6
        err = i % 11 == 0
        if i % 5 == 4:
            a = [attr("gen_ai.operation.name", "vectordb"), attr("db.system.name", "chroma"), attr("db.operation.name", random.choice(["query", "add"])), attr("db.collection.name", "docs"), attr("db.n_results", 5)]
            name = "chroma.query"
        else:
            system, model = models[i % 3]
            it, ot = random.randint(10, 2000), random.randint(10, 800)
            a = [attr("gen_ai.operation.name", random.choice(["chat", "embeddings"])), attr("gen_ai.system", system), attr("gen_ai.provider.name", system),
                 attr("gen_ai.request.model", model), attr("gen_ai.response.model", model), attr("gen_ai.usage.input_tokens", it),
                 attr("gen_ai.usage.output_tokens", ot), attr("gen_ai.usage.total_tokens", it + ot), attr("gen_ai.usage.cost", round((it + ot) * 1e-5, 6)),
                 attr("gen_ai.request.temperature", 0.7), attr("server.address", "api.example"), attr("gen_ai.endpoint", f"{system}.chat.completions")]
            name = f"{system}.chat"
        spans.append({"traceId": trace_id, "spanId": span_id, "name": name, "kind": 3, "startTimeUnixNano": str(start), "endTimeUnixNano": str(start + dur),
                      "attributes": a, "status": {"code": 2 if err else 1},
                      "events": [{"name": "exception", "timeUnixNano": str(start), "attributes": [attr("exception.type", "RateLimitError"), attr("exception.message", "too many requests"), attr("exception.stacktrace", "at call()")]}] if err else []})
        if err:
            logs.append({"timeUnixNano": str(start), "severityNumber": 17, "severityText": "ERROR", "body": {"stringValue": "rate limited"}, "traceId": trace_id, "spanId": span_id})
    res = {"attributes": [attr("service.name", "he0-chatbot"), attr("deployment.environment", "production"), attr("telemetry.sdk.name", "openlit")]}
    call("POST", OTLP + "/v1/traces", {"resourceSpans": [{"resource": res, "scopeSpans": [{"scope": {"name": "openlit"}, "spans": spans}]}]})
    call("POST", OTLP + "/v1/logs", {"resourceLogs": [{"resource": res, "scopeLogs": [{"scope": {"name": "openlit"}, "logRecords": logs}]}]})
    return [s["traceId"] for s in spans[:2]], [s["spanId"] for s in spans[:2]]


def main():
    wait(API + "/api/auth/csrf")
    wait(OTLP + "/v1/traces")
    traces, span_ids = send(int(time.time() * 10**9))
    _, csrf = call("GET", API + "/api/auth/csrf")
    call("POST", API + "/api/auth/callback/login", {"email": "user@openlit.io", "password": "openlituser", "csrfToken": csrf["csrfToken"], "json": "true"}, form=True)
    time.sleep(15)
    now = time.time()
    iso = lambda t: time.strftime("%Y-%m-%dT%H:%M:%S.000Z", time.gmtime(t))
    windows = [{"start": iso(now - 86400), "end": iso(now + 60), "type": "24H"}, {"start": iso(now - 7 * 86400), "end": iso(now + 60), "type": "7D"}]
    llm = ["request/total", "request/time", "request/duration/average", "request/exist", "request/config", "request/attribute-keys",
           "llm/token/time", "llm/cost/total", "llm/cost/time", "llm/cost/request/average", "llm/cost/application",
           "llm/cost/environment", "llm/cost/model", "llm/cost/provider", "llm/model/top", "llm/model/time", "llm/category", "llm/endpoint",
           "llm/generation-health", "llm/agent-loop", "cost/summary", "exception"]
    vector = ["request/total", "request/time", "vector/application", "vector/environment"]
    for tl in windows:
        for route in llm:
            call("POST", f"{API}/api/metrics/{route}", {"timeLimit": tl, "operationType": "llm"}, expect_ok=False)
        for route in vector:
            call("POST", f"{API}/api/metrics/{route}", {"timeLimit": tl, "operationType": "vectordb"}, expect_ok=False)
    tl = windows[0]
    for token_type in ("input", "output", "total"):
        call("POST", f"{API}/api/metrics/llm/token/request/average", {"timeLimit": tl, "operationType": "llm", "type": token_type}, expect_ok=False)
    for group in ("exception.type", "ServiceName"):
        call("POST", f"{API}/api/metrics/exception/grouped", {"timeLimit": tl, "groupBy": group}, expect_ok=False)
    for sort in ({}, {"type": "Timestamp", "direction": "asc"}, {"type": "Duration", "direction": "desc"}):
        call("POST", f"{API}/api/metrics/request", {"timeLimit": tl, "limit": 10, "offset": 0, "sorting": sort}, expect_ok=False)
    call("POST", f"{API}/api/metrics/request", {"timeLimit": tl, "limit": 10, "offset": 10, "selectedConfig": {"providers": ["openai"], "models": ["gpt-4o"], "maxCost": 1, "applicationNames": ["he0-chatbot"]}}, expect_ok=False)
    call("POST", f"{API}/api/metrics/request/grouped", {"timeLimit": tl, "groupBy": "gen_ai.request.model", "limit": 10, "offset": 0}, expect_ok=False)
    for t in traces:
        call("GET", f"{API}/api/metrics/request/trace/{t}", expect_ok=False)
    for s in span_ids:
        call("GET", f"{API}/api/metrics/request/span/{s}", expect_ok=False)
        call("GET", f"{API}/api/metrics/request/span/{s}/heirarchy", expect_ok=False)
    if refused:
        print(f"driver: openlit: {len(refused)} requests answered with an error:\n  " + "\n  ".join(refused[:20]), file=sys.stderr)
    if failures:
        print("driver: openlit: requests that failed:\n  " + "\n  ".join(failures), file=sys.stderr)
    print("driver: openlit done", file=sys.stderr)


if __name__ == "__main__":
    main()
