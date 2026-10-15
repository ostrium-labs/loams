"""Collections through the native REST API: a text and keyword schema, two
documents, a strong get and a search (§54 §14 step 2)."""

from lib import NS, until

NAME = "collections"
SCHEMA = {
    "fields": [
        {"name": "body", "source_path": "body",
         "kind": {"text": {"analyzer": "standard", "positions": True}}, "indexed": True, "fast": False},
        {"name": "tenant", "source_path": "tenant", "kind": "keyword", "indexed": True, "fast": True},
    ],
    "vectors": [],
    "sparse_vectors": [],
    "dynamic": "ignore",
    "max_fields": 1000,
}
DOCS = {1: {"body": "ada lovelace wrote the first program", "tenant": "t1"},
        2: {"body": "bob kept the books", "tenant": "t2"}}
BASE = f"/v1/namespaces/{NS}/collections/docs"


def namespace(stack):
    status, reply = stack.http().post("/v1/namespaces", {"name": NS})
    if status not in (200, 201, 409):
        raise AssertionError(f"create namespace: {status} {reply}")


def upsert(stack, docs):
    ops = [{"upsert": {"id": i, "source": src}} for i, src in docs.items()]
    stack.http().ok("POST", f"{BASE}/documents", {"ops": ops})


def get(stack, ids):
    reply = stack.http().ok("POST", f"{BASE}/documents/get", {"ids": ids})
    # A strong get answers one entry per id, `null` for one that is missing.
    return {d["id"]: d.get("source") for d in (reply or {}).get("documents", []) if d}


def write(stack):
    namespace(stack)
    stack.http().ok(
        "POST", f"/v1/namespaces/{NS}/collections",
        {"name": "docs", "schema": SCHEMA, "partitions": 1}, accept=(200, 201, 409),
    )
    upsert(stack, DOCS)
    until("the documents read", lambda: len(get(stack, list(DOCS))) == len(DOCS))
    return {"documents": {str(k): v for k, v in DOCS.items()}, "search": {"text": "lovelace", "ids": [1]}}


def search(stack, text):
    reply = stack.http().ok("POST", f"/v1/namespaces/{NS}/query", {
        "collection": "docs",
        "retrievers": [{"text": {"query": {"match": {"field": "body", "text": text}}, "k": 10}}],
        "limit": 10,
    })
    return sorted(h.get("pk") for h in reply.get("hits", []))


def check(stack, rec):
    problems = []
    want = {int(k): v for k, v in rec["documents"].items()}
    got = get(stack, list(want))
    for k, v in want.items():
        if got.get(k) != v:
            problems.append(f"document {k}: {got.get(k)!r}, wrote {v!r}")
    try:
        ids = until("the search answers", lambda: search(stack, rec["search"]["text"]) or None, within=10)
    except AssertionError as e:
        ids = str(e)
    if ids != rec["search"]["ids"]:
        problems.append(f"search {rec['search']['text']!r}: {ids}, expected {rec['search']['ids']}")
    return problems


NEW = {100: {"body": "written after the upgrade", "tenant": "t9"}}


def write_new(stack):
    upsert(stack, NEW)
    until("the new document reads", lambda: 100 in get(stack, [100]))
    return {"id": 100}


def check_new(stack, rec, present):
    there = rec["id"] in get(stack, [rec["id"]])
    return [] if there == present else [f"document {rec['id']} is {'missing' if present else 'still there'}"]
