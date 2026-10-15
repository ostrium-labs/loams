"""A Qdrant collection with two points, through the Qdrant REST API."""

from lib import Skip, until

NAME = "qdrant"
POINTS = [
    {"id": 1, "vector": [1.0, 0.0, 0.0], "payload": {"name": "ada"}},
    {"id": 2, "vector": [0.0, 1.0, 0.0], "payload": {"name": "bob"}},
]


def api(stack):
    http = stack.http(stack.qdrant)
    if not http.reachable():
        raise Skip(f"no Qdrant API at {stack.qdrant}")
    return http


def point(http, pid):
    status, reply = http.get(f"/collections/up1fx/points/{pid}")
    return reply.get("result") if status == 200 and reply else None


def write(stack):
    http = api(stack)
    http.ok("PUT", "/collections/up1fx", {"vectors": {"size": 3, "distance": "Cosine"}}, accept=(200, 201, 409))
    http.ok("PUT", "/collections/up1fx/points?wait=true", {"points": POINTS})
    until("the points read", lambda: point(http, 2))
    return {"points": {str(p["id"]): p["payload"] for p in POINTS}}


def check(stack, rec):
    http = api(stack)
    problems = []
    for pid, payload in rec["points"].items():
        got = point(http, pid)
        if not got or got.get("payload") != payload:
            problems.append(f"point {pid}: {got}, wrote payload {payload}")
    return problems


def write_new(stack):
    http = api(stack)
    http.ok("PUT", "/collections/up1fx/points?wait=true",
            {"points": [{"id": 100, "vector": [0.0, 0.0, 1.0], "payload": {"name": "new"}}]})
    until("the new point reads", lambda: point(http, 100))
    return {"id": 100}


def check_new(stack, rec, present):
    there = point(api(stack), rec["id"]) is not None
    return [] if there == present else [f"point {rec['id']} is {'missing' if present else 'still there'}"]
