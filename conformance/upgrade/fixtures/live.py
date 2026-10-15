"""Loams Live. Its documents belong to a deployed Live app; the previous
release's stack deploys none, so the fixture checks that Live is served and
answers a query of an unknown function the same way before and after. Live's
own stores are read by `loams migrate verify`. A fixture with a deployed app
(tables, documents, an index) follows when the release bundles one."""

from lib import Skip

NAME = "live"


def probe(stack):
    status, reply = stack.http().connect("loams.live.v1.LiveService/Query", {"function": "up1fx:missing"})
    code = reply.get("code") if isinstance(reply, dict) else None
    return [status, code]


def write(stack):
    if not stack.serves("loams.live.v1"):
        raise Skip("the engine does not serve loams.live.v1")
    return {"probe": probe(stack)}


def check(stack, rec):
    got = probe(stack)
    return [] if got == rec["probe"] else [f"Live answered {got}, was {rec['probe']}"]


def write_new(stack):
    return {}


def check_new(stack, rec, present):
    return []
