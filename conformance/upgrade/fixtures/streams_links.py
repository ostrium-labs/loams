"""A stream with two records and a counter link over it (§54 §14 step 2)."""

from lib import NS, b64, unb64, until
from fixtures.collections import namespace

NAME = "streams_links"
RECORDS = [("a", "5"), ("b", "10")]


def records(stack):
    reply = stack.http().ok(
        "GET", f"/v1/namespaces/{NS}/streams/events/partitions/0/records?offset=0&max_bytes=65536"
    )
    return [[unb64(r["key"]), unb64(r["value"])] for r in reply.get("records", [])]


def produce(stack, pairs):
    body = {"records": [{"key": b64(k), "value": b64(v)} for k, v in pairs]}
    stack.http().ok("POST", f"/v1/namespaces/{NS}/streams/events/partitions/0/records", body)


def counters(stack):
    return stack.http().ok("GET", f"/v1/namespaces/{NS}/links/counts").get("counters")


def write(stack):
    namespace(stack)
    stack.http().ok("POST", f"/v1/namespaces/{NS}/streams", {"name": "events", "partitions": 1},
                    accept=(200, 201, 409))
    stack.http().ok("POST", f"/v1/namespaces/{NS}/links", {"name": "counts", "source": "events"},
                    accept=(200, 201, 409))
    produce(stack, RECORDS)
    want = {"a": 5, "b": 10}
    until("the link applied the records", lambda: counters(stack) == want)
    return {"records": [list(p) for p in RECORDS], "counters": want}


def check(stack, rec):
    problems = []
    got = records(stack)
    if got[: len(rec["records"])] != rec["records"]:
        problems.append(f"stream events: {got}, wrote {rec['records']}")
    got = counters(stack)
    if got != rec["counters"] and not (isinstance(got, dict) and all(got.get(k) == v for k, v in rec["counters"].items())):
        problems.append(f"link counts: {got}, expected {rec['counters']}")
    return problems


def write_new(stack):
    produce(stack, [("c", "1")])
    until("the new record reads", lambda: ["c", "1"] in records(stack))
    return {"record": ["c", "1"]}


def check_new(stack, rec, present):
    there = rec["record"] in records(stack)
    return [] if there == present else [f"record {rec['record']} is {'missing' if present else 'still there'}"]
