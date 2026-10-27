#!/usr/bin/env python3
"""HE0 driver for OpenPanel (plan HE1 Tasks 4 and 9): synthetic traffic only.

Run by driver.sh (scripts/house/apps.sh openpanel drive). It talks to the
unmodified OpenPanel containers through their public HTTP surface only:

1. signs a user up and creates an organization, a project and a write client
   (tRPC `auth.signUpEmail`, `onboarding.project`), as the onboarding page does;
2. ingests a fixed, seeded week of events through `POST /api/track` with the
   client's secret, so the API's Redis buffers and the worker's batch inserts
   into ClickHouse run (screen views, sign-ups, purchases with revenue,
   identify, increment);
3. waits until every event is readable;
4. calls the tRPC procedures the dashboard's pages call: the overview, the
   events and profiles lists, a line chart, a bar chart with a breakdown, a
   metric, a pie, the funnel, the conversion and the retention (cohort) charts,
   the user journey and the realtime views.

Each answer is written to $HE0_OUT/openpanel/responses/<name>.json (raw, never
committed) so a run on clickhouse-server and a run on the House can be compared
(`compare.py`). Any tRPC error fails the run and is listed at the end.
Stdlib only.
"""

from __future__ import annotations

import datetime as dt
import json
import os
import random
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

URL = os.environ.get(
    "HE0_OPENPANEL_URL",
    f"http://127.0.0.1:{os.environ.get('HE0_OPENPANEL_PORT', '3200')}",
).rstrip("/")
API = f"{URL}/api"
OUT = Path(os.environ.get("HE0_OUT_APP", "conformance/clickhouse/apps/.out/openpanel"))
SEED = int(os.environ.get("HE0_SEED", "75"))
USERS = int(os.environ.get("HE0_OPENPANEL_USERS", "24"))
DAYS = int(os.environ.get("HE0_OPENPANEL_DAYS", "7"))

EMAIL = "he0-driver@example.invalid"
PASSWORD = "he0-driver-password-1"
PAGES = ["/", "/pricing", "/docs", "/docs/install", "/blog", "/blog/house", "/signup"]
REFERRERS = ["", "https://www.google.com/", "https://news.ycombinator.com/", "https://github.com/"]
AGENTS = [
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36",
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.6 Safari/605.1.15",
    "Mozilla/5.0 (iPhone; CPU iPhone OS 18_6 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.6 Mobile/15E148 Safari/604.1",
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:143.0) Gecko/20100101 Firefox/143.0",
]
# Documentation ranges (RFC 5737), so no real address is used.
IPS = ["192.0.2.", "198.51.100.", "203.0.113."]


class Driver:
    def __init__(self) -> None:
        self.cookie = ""
        self.errors: list[str] = []
        self.calls = 0
        self.tracked = False
        self.identified: set[str] = set()
        # Sent once the identified profiles are stored (an increment on a
        # profile the API has not stored yet answers 404).
        self.increments: list[tuple[str, str, str]] = []
        (OUT / "responses").mkdir(parents=True, exist_ok=True)

    # -- HTTP ---------------------------------------------------------------
    def request(self, method, url, body=None, headers=None, timeout=60):
        data = None if body is None else json.dumps(body).encode()
        req = urllib.request.Request(url, data=data, method=method)
        req.add_header("content-type", "application/json")
        if self.cookie:
            req.add_header("cookie", self.cookie)
        for k, v in (headers or {}).items():
            req.add_header(k, v)
        try:
            with urllib.request.urlopen(req, timeout=timeout) as resp:
                for value in resp.headers.get_all("set-cookie") or []:
                    if value.startswith("session="):
                        self.cookie = value.split(";", 1)[0]
                return resp.status, resp.read()
        except urllib.error.HTTPError as err:
            return err.code, err.read()

    def mutate(self, proc, payload):
        status, raw = self.request("POST", f"{API}/trpc/{proc}", {"json": payload})
        doc = json.loads(raw or b"null")
        if status != 200:
            raise SystemExit(f"driver: {proc} answered {status}: {raw[:600]!r}")
        return doc["result"]["data"]["json"]

    def query(self, name, proc, payload):
        """One dashboard read; errors are collected, not raised."""
        self.calls += 1
        qs = urllib.parse.urlencode({"input": json.dumps({"json": payload})})
        started = time.monotonic()
        status, raw = self.request("GET", f"{API}/trpc/{proc}?{qs}", timeout=120)
        ms = int((time.monotonic() - started) * 1000)
        try:
            doc = json.loads(raw)
        except ValueError:
            doc = {"raw": raw[:2000].decode(errors="replace")}
        (OUT / "responses" / f"{name}.json").write_text(
            json.dumps({"proc": proc, "status": status, "ms": ms, "body": doc}, indent=1, sort_keys=True)
        )
        if status != 200 or "error" in doc:
            message = json.dumps(doc.get("error", doc))[:400] if isinstance(doc, dict) else str(doc)[:400]
            self.errors.append(f"{name} ({proc}) {status}: {message}")
            print(f"driver: FAIL {name} {status} {ms}ms", flush=True)
            return None
        print(f"driver: ok   {name} {ms}ms", flush=True)
        return doc["result"]["data"]["json"]

    # -- setup --------------------------------------------------------------
    def wait_healthy(self):
        for _ in range(180):
            try:
                status, _ = self.request("GET", f"{API}/healthcheck", timeout=5)
                if status == 200:
                    return
            except OSError:
                pass
            time.sleep(2)
        raise SystemExit(f"driver: {API}/healthcheck never answered 200")

    def sign_up(self):
        status, raw = self.request(
            "POST",
            f"{API}/trpc/auth.signUpEmail",
            {"json": {"email": EMAIL, "password": PASSWORD, "confirmPassword": PASSWORD,
                      "firstName": "He0", "lastName": "Driver"}},
        )
        if status != 200:
            # Already signed up on an earlier drive of the same stack.
            self.mutate("auth.signInEmail", {"email": EMAIL, "password": PASSWORD})
        if not self.cookie:
            raise SystemExit(f"driver: no session cookie after sign-up ({status}: {raw[:300]!r})")

    def project(self):
        state = OUT / "client.json"
        if state.exists():
            return json.loads(state.read_text())
        client = self.mutate(
            "onboarding.project",
            {"organization": "HE0 Org", "project": "HE0 Shop", "domain": "https://shop.example.invalid",
             "cors": [], "website": True, "app": False, "backend": True, "timezone": "UTC"},
        )
        info = {"clientId": client["id"], "secret": client["secret"], "projectId": client["projectId"]}
        state.write_text(json.dumps(info))
        return info

    # -- ingest -------------------------------------------------------------
    def track(self, client, ua, ip, body):
        # The worker writes the day's device-id salts when it starts; until it
        # has, /track answers 500 ("No salt found"), so the first events retry.
        for _ in range(60):
            status, raw = self.request(
                "POST", f"{API}/track", body,
                {"openpanel-client-id": client["clientId"], "openpanel-client-secret": client["secret"],
                 "user-agent": ua, "x-client-ip": ip, "origin": "https://shop.example.invalid"},
            )
            # An increment on a profile the buffers have not stored yet is
            # 404 ("Profile not found") until they flush.
            retry = (status == 500 and not self.tracked) or (
                status == 404 and body.get("type") == "increment")
            if not retry:
                break
            time.sleep(2)
        self.tracked = self.tracked or status in (200, 202)
        if status not in (200, 202):
            raise SystemExit(f"driver: /track answered {status}: {raw[:300]!r}")

    def ingest(self, client) -> int:
        rng = random.Random(SEED)
        today = dt.datetime.now(dt.UTC).replace(hour=0, minute=0, second=0, microsecond=0)
        sent = 0
        for u in range(USERS):
            ua = AGENTS[u % len(AGENTS)]
            ip = IPS[u % len(IPS)] + str(10 + u)
            device = f"he0-device-{u:03d}"
            profile = f"he0-user-{u:03d}"
            first_day = rng.randrange(DAYS)
            for day in range(first_day, DAYS):
                if day != first_day and rng.random() < 0.45:
                    continue
                at = today - dt.timedelta(days=DAYS - 1 - day) + dt.timedelta(
                    hours=rng.randrange(1, 20), minutes=rng.randrange(60))
                ref = rng.choice(REFERRERS)
                visits = rng.randint(2, 6)
                for step in range(visits):
                    at += dt.timedelta(seconds=rng.randint(20, 240))
                    path = PAGES[0] if step == 0 else rng.choice(PAGES)
                    props = {"__path": f"https://shop.example.invalid{path}", "__title": f"HE0 {path}",
                             "__timestamp": at.isoformat(), "__ip": ip, "__deviceId": device,
                             "plan": rng.choice(["free", "pro", "team"])}
                    if step == 0 and ref:
                        props["__referrer"] = ref
                    if step == 0 and u % 3 == 0:
                        props["utm_source"] = "newsletter"
                    payload = {"name": "screen_view", "properties": props}
                    if day != first_day:
                        payload["profileId"] = profile
                    self.track(client, ua, ip, {"type": "track", "payload": payload})
                    sent += 1
                if day == first_day and rng.random() < 0.7:
                    at += dt.timedelta(seconds=30)
                    self.track(client, ua, ip, {"type": "track", "payload": {
                        "name": "sign_up", "profileId": profile,
                        "properties": {"__timestamp": at.isoformat(), "__ip": ip, "__deviceId": device,
                                       "method": rng.choice(["email", "github"])}}})
                    self.track(client, ua, ip, {"type": "identify", "payload": {
                        "profileId": profile, "firstName": f"User{u}", "lastName": "He0",
                        "email": f"user{u}@example.invalid", "properties": {"plan": "free"}}})
                    self.identified.add(profile)
                    sent += 1
                if rng.random() < 0.3:
                    at += dt.timedelta(seconds=60)
                    self.track(client, ua, ip, {"type": "track", "payload": {
                        "name": "purchase", "profileId": profile,
                        "properties": {"__timestamp": at.isoformat(), "__ip": ip, "__deviceId": device,
                                       "__revenue": rng.choice([900, 1900, 4900]), "sku": rng.choice(["a", "b"])}}})
                    if profile in self.identified:
                        self.increments.append((ua, ip, profile))
                    sent += 1
        # Live traffic (no __timestamp): OpenPanel opens sessions only for
        # events newer than 15 minutes, so this pass fills the sessions table
        # (its VersionedCollapsingMergeTree sign rows), the overview's session
        # metrics and the realtime pages. Device ids come from the salted
        # ip + user agent, as for a browser.
        for u in range(USERS // 2):
            ua = AGENTS[u % len(AGENTS)]
            ip = IPS[(u + 1) % len(IPS)] + str(100 + u)
            for step in range(rng.randint(1, 4)):
                path = PAGES[0] if step == 0 else rng.choice(PAGES)
                props = {"__path": f"https://shop.example.invalid{path}", "__title": f"HE0 {path}", "__ip": ip}
                if step == 0 and u % 2 == 0:
                    props["__referrer"] = rng.choice(REFERRERS[1:])
                payload = {"name": "screen_view", "properties": props}
                if u % 3 == 0:
                    payload["profileId"] = f"he0-user-{u:03d}"
                self.track(client, ua, ip, {"type": "track", "payload": payload})
                sent += 1
            if u % 4 == 0:
                self.track(client, ua, ip, {"type": "track", "payload": {
                    "name": "purchase", "profileId": f"he0-user-{u:03d}",
                    "properties": {"__ip": ip, "__revenue": 1900, "sku": "a"}}})
                sent += 1
        return sent

    def wait_ingested(self, client, sent):
        """Poll the events list total until it stops moving (buffers flushed)."""
        last, stable = -1, 0
        for _ in range(90):
            got = self.count_events(client)
            print(f"driver: events visible {got}/{sent}", flush=True)
            if got >= sent:
                return got
            stable = stable + 1 if got == last else 0
            if stable >= 10:
                return got
            last = got
            time.sleep(3)
        return last

    def wait_profile_property(self, client, profile, prop):
        """Wait until the profile buffer has flushed an increment, so the reads
        (and a later replay of them) see the profiles in their final state."""
        for _ in range(60):
            qs = urllib.parse.urlencode({"input": json.dumps({"json": {
                "projectId": client["projectId"], "profileId": profile}})})
            status, raw = self.request("GET", f"{API}/trpc/profile.byId?{qs}")
            try:
                props = json.loads(raw)["result"]["data"]["json"]["properties"]
            except (KeyError, TypeError, ValueError):
                props = {}
            if status == 200 and prop in (props or {}):
                return
            time.sleep(2)
        print(f"driver: {profile} never showed {prop}", flush=True)

    def count_events(self, client):
        qs = urllib.parse.urlencode({"input": json.dumps({"json": {
            "projectId": client["projectId"], "chartType": "metric", "interval": "day", "range": "30d",
            "series": [{"type": "event", "id": "A", "name": "*", "segment": "event", "filters": []}],
            "breakdowns": [], "metric": "sum", "previous": False,
            # A fresh display name per poll: chart reads are cached by input.
            "name": f"he0-count-{time.time_ns()}"}})})
        status, raw = self.request("GET", f"{API}/trpc/chart.chart?{qs}")
        if status != 200:
            return 0
        try:
            return int(json.loads(raw)["result"]["data"]["json"]["series"][0]["metrics"]["sum"])
        except (KeyError, IndexError, TypeError, ValueError):
            return 0

    # -- reads ----------------------------------------------------------------
    def reads(self, client):
        pid = client["projectId"]
        rng = {"range": "7d"}
        base = {"projectId": pid, "filters": [], **rng}
        q = self.query
        # The overview page.
        q("overview-stats", "overview.stats", {**base, "interval": "day"})
        for mode in ("page", "entry", "exit", "bot"):
            q(f"overview-top-pages-{mode}", "overview.topPages", {**base, "mode": mode})
        for column in ("referrer_name", "referrer_type", "utm_source", "country", "browser", "os", "device"):
            q(f"overview-top-{column}", "overview.topGeneric", {**base, "column": column})
        q("overview-top-generic-series", "overview.topGenericSeries", {**base, "column": "browser", "interval": "day"})
        q("overview-top-events", "overview.topEvents", base)
        q("overview-top-conversions", "overview.topConversions", base)
        q("overview-top-link-out", "overview.topLinkOut", base)
        q("overview-user-journey", "overview.userJourney", {**base, "steps": 4})
        q("overview-map", "overview.map", base)
        q("overview-live-visitors", "overview.liveVisitors", {"projectId": pid})
        q("overview-live-data", "overview.liveData", {"projectId": pid})
        # Charts (the report editor and dashboards).
        series_a = {"type": "event", "id": "A", "name": "screen_view", "segment": "event", "filters": []}
        series_b = {"type": "event", "id": "B", "name": "sign_up", "segment": "user", "filters": []}
        series_c = {"type": "event", "id": "C", "name": "purchase", "segment": "event", "filters": []}
        report = {"projectId": pid, "interval": "day", "breakdowns": [], "metric": "sum",
                  "previous": False, **rng}
        q("chart-linear", "chart.chart", {**report, "chartType": "linear", "series": [series_a, series_b], "previous": True})
        q("chart-bar-breakdown", "chart.chart", {**report, "chartType": "bar", "series": [series_a],
                                                  "breakdowns": [{"id": "A", "name": "path"}]})
        q("chart-pie-browser", "chart.chart", {**report, "chartType": "pie", "series": [series_a],
                                                "breakdowns": [{"id": "A", "name": "browser"}]})
        q("chart-metric-unique", "chart.chart", {**report, "chartType": "metric",
                                                  "series": [{**series_a, "segment": "user"}]})
        q("chart-area-sessions", "chart.chart", {**report, "chartType": "area",
                                                  "series": [{**series_a, "segment": "session"}]})
        q("chart-revenue-sum", "chart.chart", {**report, "chartType": "linear",
                                                "series": [{**series_c, "segment": "property_sum", "property": "revenue"}]})
        q("chart-formula", "chart.chart", {**report, "chartType": "linear", "series": [
            series_a, series_b, {"type": "formula", "id": "F", "formula": "B/A*100"}]})
        q("chart-hourly", "chart.chart", {**report, "chartType": "linear", "interval": "hour", "series": [series_a]})
        q("chart-aggregate", "chart.aggregate", {**report, "chartType": "bar", "series": [series_a, series_c]})
        q("chart-histogram", "chart.chart", {**report, "chartType": "histogram", "series": [series_a]})
        q("chart-map", "chart.chart", {**report, "chartType": "map", "series": [series_a],
                                        "breakdowns": [{"id": "A", "name": "country"}]})
        funnel = {**report, "chartType": "funnel", "series": [
            {**series_a, "filters": [{"id": "f", "name": "path", "operator": "is", "value": ["/"]}]},
            {**series_b}, {**series_c}], "options": {"type": "funnel", "funnelWindow": 24}}
        q("chart-funnel", "chart.funnel", funnel)
        q("chart-funnel-session", "chart.funnel", {**funnel, "options": {"type": "funnel", "funnelGroup": "session_id"}})
        q("chart-funnel-breakdown", "chart.funnel", {**funnel, "breakdowns": [{"id": "A", "name": "browser"}]})
        q("chart-conversion", "chart.conversion", {**report, "chartType": "conversion", "series": [series_a, series_c]})
        q("chart-sankey", "chart.sankey", {**report, "chartType": "sankey", "series": [series_a],
                                            "options": {"type": "sankey", "mode": "after", "steps": 4, "exclude": [],
                                                        "include": []}})
        for criteria in ("on_or_after", "on"):
            q(f"chart-retention-{criteria}", "chart.cohort", {
                "projectId": pid, "firstEvent": ["sign_up"], "secondEvent": ["screen_view"],
                "criteria": criteria, "interval": "day", **rng})
        q("chart-events", "chart.events", {"projectId": pid})
        q("chart-properties", "chart.properties", {"projectId": pid})
        q("chart-values", "chart.values", {"projectId": pid, "event": "screen_view", "property": "path"})
        # Events, profiles and sessions pages.
        q("event-list", "event.events", {"projectId": pid, "filters": []})
        q("event-conversion-names", "event.conversionNames", {"projectId": pid})
        q("profile-list", "profile.list", {"projectId": pid, "take": 50})
        q("profile-list-identified", "profile.list", {"projectId": pid, "take": 50, "isExternal": True})
        q("profile-power-users", "profile.powerUsers", {"projectId": pid, "take": 50})
        profile = "he0-user-000"
        for proc in ("byId", "metrics", "activity", "mostEvents", "popularRoutes"):
            q(f"profile-{proc}", f"profile.{proc}", {"projectId": pid, "profileId": profile})
        q("profile-properties", "profile.properties", {"projectId": pid})
        q("session-list", "session.list", {"projectId": pid, "take": 50})
        # The realtime page.
        for proc in ("activeSessions", "paths", "referrals", "geo", "coordinates"):
            q(f"realtime-{proc}", f"realtime.{proc}", {"projectId": pid})


def main() -> int:
    d = Driver()
    d.wait_healthy()
    d.sign_up()
    client = d.project()
    print(f"driver: project {client['projectId']}", flush=True)
    marker = OUT / "ingested.json"
    if not marker.exists():
        sent = d.ingest(client)
        marker.write_text(json.dumps({"sent": sent}))
    sent = json.loads(marker.read_text())["sent"]
    visible = d.wait_ingested(client, sent)
    for ua, ip, profile in d.increments:
        d.track(client, ua, ip, {"type": "increment", "payload": {
            "profileId": profile, "property": "purchases", "value": 1}})
    if d.increments:
        d.wait_profile_property(client, d.increments[-1][2], "purchases")
    d.reads(client)
    summary = {"sent": sent, "visible": visible, "calls": d.calls, "errors": d.errors}
    (OUT / "driver-summary.json").write_text(json.dumps(summary, indent=1))
    print(f"driver: {sent} events sent, {visible} visible, {d.calls} reads, {len(d.errors)} errors")
    for e in d.errors:
        print(f"driver: error {e}")
    return 1 if d.errors or visible < sent else 0


if __name__ == "__main__":
    sys.exit(main())
