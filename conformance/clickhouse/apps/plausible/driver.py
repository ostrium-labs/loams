"""HE0 driver for Plausible CE v3.2.1 (plan HE1 Task 5): synthetic traffic only.

Sends pageviews, custom events with properties, revenue events and
engagement events through the public events API (/api/event), waits for the
ingest buffer to flush, then opens every dashboard read of the public site
(/api/stats/<domain>/...) with and without filters, comparisons and each
period, the Stats API v1 (aggregate, breakdown, timeseries, realtime) and
the Stats API v2 query endpoint. Standard library only.
"""

import json
import os
import random
import sys
import time
import urllib.error
import urllib.parse
import urllib.request

URL = os.environ.get("HE0_PLAUSIBLE_URL", f"http://127.0.0.1:{os.environ.get('HE0_PLAUSIBLE_PORT', '3400')}")
DOMAIN = "he0.example"
KEY = "he0" + "k" * 61
random.seed(75)
failures = []

UAS = [
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0 Safari/537.36",
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 14_5) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.5 Safari/605.1.15",
    "Mozilla/5.0 (iPhone; CPU iPhone OS 17_5 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.5 Mobile/15E148 Safari/604.1",
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:131.0) Gecko/20100101 Firefox/131.0",
]


def call(method, path, body=None, params=None, headers=None, expect_ok=True):
    url = URL + path
    if params:
        url += "?" + urllib.parse.urlencode(params)
    data = json.dumps(body).encode() if body is not None else None
    h = {"Content-Type": "application/json", "User-Agent": UAS[0]}
    h.update(headers or {})
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
            failures.append(f"{method} {path} {params or ''}: {e.code} {e.read()[:200]!r}")
        return e.code, None


def wait():
    for _ in range(150):
        try:
            with urllib.request.urlopen(URL + "/api/health", timeout=5):
                return
        except Exception:
            time.sleep(2)
    sys.exit("driver: plausible did not answer")


def main():
    wait()
    paths = ["/", "/pricing", "/docs", "/docs/install", "/blog/post-1", "/signup"]
    refs = ["", "https://www.google.com/", "https://news.ycombinator.com/", "https://duckduckgo.com/"]
    sent = 0
    for visit in range(60):
        ip = f"203.0.113.{visit % 250 + 1}" if visit % 2 else f"198.51.100.{visit % 250 + 1}"
        h = {"User-Agent": UAS[visit % len(UAS)], "X-Forwarded-For": ip}
        for step in range(random.randint(1, 4)):
            path = paths[min(step + visit % 3, len(paths) - 1)]
            q = "?utm_source=newsletter&utm_medium=email&utm_campaign=launch&utm_content=a&utm_term=b" if step == 0 and visit % 4 == 0 else ""
            ev = {"name": "pageview", "url": f"http://{DOMAIN}{path}{q}", "domain": DOMAIN, "referrer": refs[visit % len(refs)] if step == 0 else "", "screen_width": [1920, 390, 1440][visit % 3]}
            call("POST", "/api/event", ev, headers=h)
            sent += 1
            call("POST", "/api/event", {"n": "engagement", "u": ev["url"], "d": DOMAIN, "sd": 40 + step, "e": 5000 + 100 * visit}, headers=h, expect_ok=False)
            if path == "/signup":
                call("POST", "/api/event", {"name": "Signup", "url": ev["url"], "domain": DOMAIN, "props": {"plan": random.choice(["free", "pro"]), "seats": str(visit % 7)}}, headers=h)
                call("POST", "/api/event", {"name": "Purchase", "url": ev["url"], "domain": DOMAIN, "revenue": {"amount": 10 + visit, "currency": "USD"}}, headers=h)
                sent += 2
    print(f"driver: plausible: {sent} events sent; waiting for the buffer", file=sys.stderr)
    time.sleep(12)

    s = f"/api/stats/{DOMAIN}"
    base = [{"period": p} for p in ("realtime", "day", "7d", "30d", "month", "6mo", "12mo", "year", "all")]
    base.append({"period": "custom", "from": time.strftime("%Y-%m-%d", time.gmtime(time.time() - 3 * 86400)), "to": time.strftime("%Y-%m-%d")})
    filters = [
        [["is", "visit:source", ["Google"]]],
        [["is", "event:page", ["/pricing"]]],
        [["contains", "event:page", ["docs"]]],
        [["is", "event:goal", ["Signup"]]],
        [["is", "event:props:plan", ["pro"]]],
        [["is_not", "visit:browser", ["Safari"]], ["is", "visit:device", ["Desktop"]]],
    ]
    reports = ["sources", "channels", "utm_mediums", "utm_sources", "utm_campaigns", "utm_contents", "utm_terms", "pages", "entry-pages", "exit-pages", "countries", "regions", "cities", "browsers", "browser-versions", "operating-systems", "operating-system-versions", "screen-sizes", "conversions"]
    call("GET", f"{s}/current-visitors")
    for p in base:
        call("GET", f"{s}/top-stats", params=p)
        call("GET", f"{s}/main-graph", params={**p, "metric": "visitors"}, expect_ok=p["period"] != "realtime")
        for r in reports:
            call("GET", f"{s}/{r}", params=p, expect_ok=False)
    day = {"period": "7d"}
    for f in filters:
        fp = {**day, "filters": json.dumps(f)}
        call("GET", f"{s}/top-stats", params=fp, expect_ok=False)
        call("GET", f"{s}/main-graph", params={**fp, "metric": "visitors"}, expect_ok=False)
        for r in ("sources", "pages", "conversions", "browsers"):
            call("GET", f"{s}/{r}", params=fp, expect_ok=False)
    for metric in ("visitors", "pageviews", "visits", "bounce_rate", "visit_duration", "views_per_visit", "events", "conversion_rate", "total_revenue", "scroll_depth"):
        call("GET", f"{s}/main-graph", params={**day, "metric": metric, "interval": "hour"}, expect_ok=False)
    call("GET", f"{s}/top-stats", params={**day, "comparison": "previous_period"}, expect_ok=False)
    call("GET", f"{s}/main-graph", params={**day, "metric": "visitors", "comparison": "year_over_year"}, expect_ok=False)
    call("GET", f"{s}/pages", params={**day, "detailed": "true", "order_by": json.dumps([["visitors", "desc"]])}, expect_ok=False)
    call("GET", f"{s}/referrers/Google", params=day, expect_ok=False)
    call("GET", f"{s}/custom-prop-values/plan", params=day, expect_ok=False)
    for f in ("source", "page", "country", "browser", "goal", "prop_key"):
        call("GET", f"{s}/suggestions/{f}", params={**day, "q": ""}, expect_ok=False)

    auth = {"Authorization": f"Bearer {KEY}"}
    v1 = "/api/v1/stats"
    call("GET", f"{v1}/realtime/visitors", params={"site_id": DOMAIN}, headers=auth)
    call("GET", f"{v1}/aggregate", params={"site_id": DOMAIN, "period": "7d", "metrics": "visitors,pageviews,bounce_rate,visit_duration,visits,views_per_visit"}, headers=auth)
    call("GET", f"{v1}/timeseries", params={"site_id": DOMAIN, "period": "7d", "metrics": "visitors,pageviews"}, headers=auth)
    for prop in ("event:page", "visit:source", "visit:country", "event:props:plan", "event:goal"):
        call("GET", f"{v1}/breakdown", params={"site_id": DOMAIN, "period": "7d", "property": prop, "metrics": "visitors,events"}, headers=auth, expect_ok=False)
    q = [
        {"metrics": ["visitors", "pageviews", "bounce_rate"], "date_range": "7d"},
        {"metrics": ["visitors"], "date_range": "7d", "dimensions": ["time:hour"]},
        {"metrics": ["visitors", "events"], "date_range": "7d", "dimensions": ["event:page"], "filters": [["contains", "event:page", ["docs"]]], "order_by": [["visitors", "desc"]]},
        {"metrics": ["conversion_rate", "total_revenue"], "date_range": "7d", "dimensions": ["event:goal"]},
        {"metrics": ["visitors"], "date_range": "all", "dimensions": ["visit:source", "visit:device"], "include": {"imports": True, "total_rows": True}},
        {"metrics": ["visitors", "percentage"], "date_range": "30d", "dimensions": ["event:props:plan"]},
    ]
    for body in q:
        call("POST", "/api/v2/query", {"site_id": DOMAIN, **body}, headers=auth, expect_ok=False)
    call("GET", f"/{DOMAIN}/export", params=day, expect_ok=False)
    if failures:
        print("driver: plausible: reads that failed:\n  " + "\n  ".join(failures), file=sys.stderr)
    print("driver: plausible done", file=sys.stderr)


if __name__ == "__main__":
    main()
