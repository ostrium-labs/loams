"""Durable promises in each state (pending, resolved), through the Durable
protocol on its listener."""

from lib import Skip

NAME = "durable"
VERSION = "2026-04-01"
FAR = (2**63 - 1) // 2


def call(stack, kind, data):
    http = stack.http(stack.durable)
    if not http.reachable():
        raise Skip(f"no Durable listener at {stack.durable}")
    status, reply = http.post("/", {"kind": kind, "head": {"corrId": f"up1fx-{kind}", "version": VERSION},
                                    "data": data})
    return status, reply


def state(stack, pid):
    status, reply = call(stack, "promise.get", {"id": pid})
    if status >= 300 or not reply:
        return None
    return ((reply.get("data") or {}).get("promise") or {}).get("state")


def create(stack, pid):
    status, reply = call(stack, "promise.create", {"id": pid, "timeoutAt": FAR, "param": {}, "tags": {}})
    if status >= 300:
        raise AssertionError(f"promise.create {pid}: {status} {reply}")


def write(stack):
    create(stack, "up1fx-p1")
    create(stack, "up1fx-p2")
    status, reply = call(stack, "promise.settle",
                         {"id": "up1fx-p2", "state": "resolved", "value": {"data": "done"}})
    if status >= 300:
        raise AssertionError(f"promise.settle: {status} {reply}")
    return {"states": {"up1fx-p1": state(stack, "up1fx-p1"), "up1fx-p2": state(stack, "up1fx-p2")}}


def check(stack, rec):
    return [
        f"promise {pid}: {state(stack, pid)}, was {want}"
        for pid, want in rec["states"].items()
        if state(stack, pid) != want
    ]


def write_new(stack):
    create(stack, "up1fx-p3")
    return {"id": "up1fx-p3"}


def check_new(stack, rec, present):
    there = state(stack, rec["id"]) is not None
    return [] if there == present else [f"promise {rec['id']} is {'missing' if present else 'still there'}"]
