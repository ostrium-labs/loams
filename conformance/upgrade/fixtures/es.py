"""An index with two documents, through the Elasticsearch REST API."""

from lib import Skip, until

NAME = "es"
DOCS = {"1": {"title": "ada", "n": 1}, "2": {"title": "bob", "n": 2}}


def api(stack):
    http = stack.http(stack.es)
    if not http.reachable():
        raise Skip(f"no Elasticsearch API at {stack.es}")
    return http


def doc(http, did):
    status, reply = http.get(f"/up1fx/_doc/{did}")
    return reply.get("_source") if status == 200 and reply and reply.get("found") else None


def write(stack):
    http = api(stack)
    http.ok("PUT", "/up1fx", {"mappings": {"properties": {"title": {"type": "text"}, "n": {"type": "long"}}}},
            accept=(200, 201, 400))
    for did, src in DOCS.items():
        http.ok("PUT", f"/up1fx/_doc/{did}?refresh=true", src)
    until("the documents read", lambda: doc(http, "2"))
    return {"documents": DOCS}


def check(stack, rec):
    http = api(stack)
    return [
        f"document {did}: {doc(http, did)}, wrote {src}"
        for did, src in rec["documents"].items()
        if doc(http, did) != src
    ]


def write_new(stack):
    http = api(stack)
    http.ok("PUT", "/up1fx/_doc/100?refresh=true", {"title": "new", "n": 100})
    until("the new document reads", lambda: doc(http, "100"))
    return {"id": "100"}


def check_new(stack, rec, present):
    there = doc(api(stack), rec["id"]) is not None
    return [] if there == present else [f"document {rec['id']} is {'missing' if present else 'still there'}"]
