"""An owned graph with two nodes and an edge, through `loams.graph.v1`."""

from lib import NS, Skip, until
from fixtures.collections import namespace

NAME = "graph"
MATCH = "MATCH (a:Person)-[:KNOWS]->(b:Person) RETURN a.name, b.name ORDER BY a.name"
COUNT = "MATCH (p:Person) RETURN count(p)"


def execute(stack, statement):
    status, reply = stack.http().connect(
        "loams.graph.v1.GraphService/Execute", {"namespace": NS, "graph": "social", "statement": statement}
    )
    if status != 200:
        raise AssertionError(f"Execute {statement!r}: {status} {reply}")
    rows = (reply.get("rows") or {}).get("rows", [])

    def value(v):
        return next(iter(v.values())) if isinstance(v, dict) and v else v

    return [[value(v) for v in row.get("values", [])] for row in rows]


def write(stack):
    if not stack.serves("loams.graph.v1"):
        raise Skip("the engine does not serve loams.graph.v1")
    namespace(stack)
    status, reply = stack.http().connect(
        "loams.graph.v1.GraphAdminService/CreateGraph",
        {"namespace": NS, "name": "social", "idempotencyKey": "up1fx-social"},
    )
    if status != 200:
        raise AssertionError(f"CreateGraph: {status} {reply}")
    execute(stack, "INSERT (:Person {name: 'Ada'})-[:KNOWS]->(:Person {name: 'Bob'})")
    want = [["Ada", "Bob"]]
    until("the graph reads", lambda: execute(stack, MATCH) == want)
    return {"match": want}


def check(stack, rec):
    got = execute(stack, MATCH)
    return [] if got == rec["match"] else [f"graph social: {got}, wrote {rec['match']}"]


def count(stack):
    rows = execute(stack, COUNT)
    return int(rows[0][0]) if rows else 0


def write_new(stack):
    execute(stack, "INSERT (:Person {name: 'Cy'})")
    until("the new node reads", lambda: count(stack) == 3)
    return {"persons": 3}


def check_new(stack, rec, present):
    n = count(stack)
    want = rec["persons"] if present else rec["persons"] - 1
    return [] if n == want else [f"graph social has {n} persons, expected {want}"]
