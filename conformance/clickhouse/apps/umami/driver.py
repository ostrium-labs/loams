"""HE0 driver for Umami 3.4.0 (plan HE1 Task 5): synthetic traffic only.

Logs in as the default admin, creates a website, sends pageviews, custom
events with data, identified sessions and a batch through the public
collection API (/api/send, /api/batch), then opens every analytics read the
dashboard makes: stats, pageviews, each metric type, expanded metrics,
values, events, event series and stats, sessions, realtime and each report
(funnel, goal, retention, UTM, journey, revenue, performance, attribution).
Standard library only.
"""

import json
import os
import random
import sys
import time
import urllib.error
import urllib.parse
import urllib.request

URL = os.environ.get("HE0_UMAMI_URL", f"http://127.0.0.1:{os.environ.get('HE0_UMAMI_PORT', '3300')}")
UA = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0 Safari/537.36"
random.seed(75)
failures = []


def call(method, path, body=None, token=None, params=None, headers=None, expect_ok=True):
    url = URL + path
    if params:
        url += "?" + urllib.parse.urlencode(params)
    data = json.dumps(body).encode() if body is not None else None
    h = {"Content-Type": "application/json", "User-Agent": UA}
    if token:
        h["Authorization"] = f"Bearer {token}"
    h.update(headers or {})
    req = urllib.request.Request(url, data=data, method=method, headers=h)
    try:
        with urllib.request.urlopen(req, timeout=60) as r:
            text = r.read().decode(errors="replace")
            return r.status, (json.loads(text) if text else None)
    except urllib.error.HTTPError as e:
        if expect_ok:
            failures.append(f"{method} {path}: {e.code} {e.read()[:200]!r}")
        return e.code, None


def wait():
    for _ in range(120):
        try:
            with urllib.request.urlopen(URL + "/api/heartbeat", timeout=5):
                return
        except Exception:
            time.sleep(2)
    sys.exit("driver: umami did not answer")


def main():
    wait()
    _, login = call("POST", "/api/auth/login", {"username": "admin", "password": "umami"})
    token = login["token"]
    _, site = call("POST", "/api/websites", {"name": "he0", "domain": "he0.example"}, token)
    wid = site["id"]

    paths = ["/", "/pricing", "/docs", "/docs/install", "/blog/post-1", "/signup"]
    referrers = ["", "https://www.google.com/", "https://news.ycombinator.com/", "https://twitter.com/"]
    langs = ["en-US", "de-DE", "fr-FR", "ja-JP"]
    screens = ["1920x1080", "390x844", "1440x900"]
    sent = 0
    for visit in range(40):
        cache = None
        ua = UA.replace("130.0", f"{120 + visit % 10}.0")
        for step in range(random.randint(1, 5)):
            path = paths[min(step, len(paths) - 1)] if visit % 3 else random.choice(paths)
            q = "?utm_source=news&utm_medium=email&utm_campaign=launch" if step == 0 and visit % 4 == 0 else ""
            payload = {
                "website": wid,
                "hostname": "he0.example",
                "language": langs[visit % len(langs)],
                "referrer": referrers[visit % len(referrers)] if step == 0 else "",
                "screen": screens[visit % len(screens)],
                "title": f"Page {path}",
                "url": path + q,
            }
            headers = {"User-Agent": ua}
            if cache:
                headers["x-umami-cache"] = cache
            _, r = call("POST", "/api/send", {"type": "event", "payload": payload}, headers=headers)
            cache = (r or {}).get("cache", cache)
            sent += 1
            if step == 1:
                ev = dict(payload, name="signup-click", data={"plan": random.choice(["free", "pro"]), "seats": visit % 7, "revenue": 10 * visit, "currency": "USD"})
                call("POST", "/api/send", {"type": "event", "payload": ev}, headers=headers)
                sent += 1
        if visit % 5 == 0:
            ident = {"website": wid, "hostname": "he0.example", "id": f"user-{visit}", "data": {"tier": "gold", "age": 30 + visit}}
            call("POST", "/api/send", {"type": "identify", "payload": ident}, headers={"User-Agent": ua, **({"x-umami-cache": cache} if cache else {})})
    batch = [
        {"type": "event", "payload": {"website": wid, "hostname": "he0.example", "url": f"/batch/{i}", "title": "batch", "language": "en-US", "screen": "1920x1080", "referrer": ""}}
        for i in range(20)
    ]
    call("POST", "/api/batch", batch)
    print(f"driver: umami: {sent + len(batch)} events sent", file=sys.stderr)
    time.sleep(3)

    now = int(time.time() * 1000)
    rng = {"startAt": now - 3 * 86400 * 1000, "endAt": now + 60000}
    tz = {"timezone": "UTC"}
    w = f"/api/websites/{wid}"
    call("GET", f"{w}/stats", token=token, params=rng)
    call("GET", f"{w}/stats", token=token, params={**rng, "path": "/pricing"})
    call("GET", f"{w}/stats", token=token, params={**rng, "compare": "prev"}, expect_ok=False)
    for unit in ("hour", "day"):
        call("GET", f"{w}/pageviews", token=token, params={**rng, **tz, "unit": unit})
    call("GET", f"{w}/pageviews", token=token, params={**rng, **tz, "unit": "day", "compare": "yoy"}, expect_ok=False)
    types = ["path", "entry", "exit", "title", "query", "referrer", "channel", "domain", "country", "region", "city", "browser", "os", "device", "language", "screen", "event", "hostname", "tag", "distinctId"]
    for t in types:
        call("GET", f"{w}/metrics", token=token, params={**rng, "type": t}, expect_ok=False)
        call("GET", f"{w}/metrics/expanded", token=token, params={**rng, "type": t}, expect_ok=False)
    call("GET", f"{w}/metrics", token=token, params={**rng, "type": "path", "search": "docs", "limit": 5, "offset": 1})
    call("GET", f"{w}/metrics", token=token, params={**rng, "type": "browser", "country": "US", "os": "Linux"}, expect_ok=False)
    for t in ("path", "event", "referrer", "fullPath", "channel"):
        call("GET", f"{w}/values", token=token, params={**rng, "type": t}, expect_ok=False)
    call("GET", f"{w}/active", token=token)
    call("GET", f"{w}/events", token=token, params={**rng, "pageSize": 5})
    call("GET", f"{w}/events", token=token, params={**rng, "search": "signup"})
    call("GET", f"{w}/events/series", token=token, params={**rng, **tz, "unit": "hour"})
    call("GET", f"{w}/events/stats", token=token, params=rng)
    call("GET", f"{w}/event-data/events", token=token, params=rng, expect_ok=False)
    call("GET", f"{w}/event-data/fields", token=token, params=rng, expect_ok=False)
    call("GET", f"{w}/event-data/properties", token=token, params=rng, expect_ok=False)
    call("GET", f"{w}/event-data/values", token=token, params={**rng, "event": "signup-click", "propertyName": "plan"}, expect_ok=False)
    call("GET", f"{w}/event-data/stats", token=token, params=rng, expect_ok=False)
    call("GET", f"{w}/session-data/properties", token=token, params=rng, expect_ok=False)
    call("GET", f"{w}/session-data/values", token=token, params={**rng, "propertyName": "tier"}, expect_ok=False)
    _, sessions = call("GET", f"{w}/sessions", token=token, params={**rng, "pageSize": 10})
    call("GET", f"{w}/sessions/stats", token=token, params=rng, expect_ok=False)
    call("GET", f"{w}/sessions/weekly", token=token, params={**rng, **tz}, expect_ok=False)
    for s in ((sessions or {}).get("data") or [])[:3]:
        sid = s.get("id")
        call("GET", f"{w}/sessions/{sid}", token=token, expect_ok=False)
        call("GET", f"{w}/sessions/{sid}/activity", token=token, params=rng, expect_ok=False)
        call("GET", f"{w}/sessions/{sid}/properties", token=token, expect_ok=False)
    call("GET", f"/api/realtime/{wid}", token=token, params={**tz, "unit": "minute"})

    iso = {"startDate": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(rng["startAt"] / 1000)), "endDate": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(rng["endAt"] / 1000))}

    def report(kind, parameters=None):
        body = {"websiteId": wid, "type": kind, "filters": {**rng, "timezone": "UTC", "unit": "day"}, "parameters": {**iso, **(parameters or {})}}
        call("POST", f"/api/reports/{kind}", body, token, expect_ok=False)

    report("funnel", {"window": 60, "steps": [{"type": "path", "value": "/"}, {"type": "path", "value": "/pricing"}, {"type": "event", "value": "signup-click"}]})
    report("goal", {"type": "path", "value": "/signup"})
    report("goal", {"type": "event", "value": "signup-click"})
    report("retention")
    report("utm")
    report("journey", {"steps": 4})
    report("revenue", {"currency": "USD"})
    report("performance")
    report("attribution", {"model": "first-click", "type": "path", "step": "/signup"})
    report("breakdown", {"fields": ["os", "browser"]})
    if failures:
        print("driver: umami: reads that failed:\n  " + "\n  ".join(failures), file=sys.stderr)
    print("driver: umami done", file=sys.stderr)


if __name__ == "__main__":
    main()
