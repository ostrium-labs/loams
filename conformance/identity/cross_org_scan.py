#!/usr/bin/env python3
"""no_cross_org_tuples (plan IA1, IA0 Task 2; design 76 D1612).

Reads OpenFGA tuples and fails if any tuple links objects rooted in two different orgs.

Every object is rooted by its parent links: `project#org@org:O`, `environment#project@project:P`,
`deployment|database|graph#environment@environment:E`, `team#org@org:O`, `agent#project@project:P`
and `service_account#project@project:P`. People (`user:*`) belong to no org and are skipped, as is
the install root `platform:*`, which every org hangs off.

  cross_org_scan.py tuples.json        # OpenFGA Read output ({"tuples": [{"key": ...}]}) or a list
  cross_org_scan.py --store URL STORE  # read every tuple from a running OpenFGA (HTTP API)
  cross_org_scan.py --self-test
"""
import json
import sys
import urllib.request

PARENT_RELATIONS = {"org", "project", "environment"}


def obj(ref):
    return ref.split("#", 1)[0]


def parents(tuples):
    out = {}
    for t in tuples:
        if t["relation"] in PARENT_RELATIONS and not t["user"].startswith("user:"):
            out.setdefault(t["object"], []).append(obj(t["user"]))
    return out


def roots(tuples):
    # An object with two parents (an environment with both `project` and `org`) is reported by
    # scan(); for rooting, the first one counts.
    parent = {o: ps[0] for o, ps in parents(tuples).items()}

    def root(o, seen=()):
        if o.startswith("org:"):
            return o
        if o in seen or o not in parent:
            return None
        return root(parent[o], seen + (o,))

    return root


def scan(tuples):
    root = roots(tuples)
    bad = []
    for o, ps in parents(tuples).items():
        if len(set(ps)) > 1:
            bad.append(f"{o} has {len(set(ps))} parents: {sorted(set(ps))}")
    for t in tuples:
        u = obj(t["user"])
        if u.startswith(("user:", "platform:")) or t["object"].startswith("platform:"):
            continue
        ru, ro = root(u), root(t["object"])
        if ru and ro and ru != ro:
            bad.append(f'{t["object"]}#{t["relation"]}@{t["user"]} links {ru} to {ro}')
    return bad


def load(path):
    data = json.load(open(path))
    items = data["tuples"] if isinstance(data, dict) else data
    return [i.get("key", i) for i in items]


def read_store(url, store):
    out, token = [], ""
    while True:
        body = json.dumps({"page_size": 100, "continuation_token": token}).encode()
        req = urllib.request.Request(f"{url}/stores/{store}/read", data=body, method="POST")
        res = json.load(urllib.request.urlopen(req))
        out += [t["key"] for t in res.get("tuples", [])]
        token = res.get("continuation_token", "")
        if not token:
            return out


def self_test():
    here = __file__.rsplit("/", 1)[0]
    clean = load(f"{here}/fixtures/two-orgs.json")
    assert scan(clean) == [], scan(clean)
    leak = clean + [{"user": "team:acme-eng#member", "relation": "editor", "object": "project:globex-p1"}]
    assert len(scan(leak)) == 1, scan(leak)
    leak = clean + [{"user": "agent:acme-bot", "relation": "editor", "object": "environment:globex-prod"}]
    assert len(scan(leak)) == 1, scan(leak)
    leak = clean + [{"user": "environment:acme-dev", "relation": "environment", "object": "database:globex-db"}]
    assert scan(leak), "a database re-parented across orgs is reported"
    leak = clean + [{"user": "org:globex", "relation": "org", "object": "environment:acme-dev"}]
    assert scan(leak), "an environment with a project and an org of its own is reported"
    print("cross_org_scan: self-test ok")


if __name__ == "__main__":
    if sys.argv[1:] == ["--self-test"]:
        self_test()
        sys.exit(0)
    tuples = read_store(sys.argv[2], sys.argv[3]) if sys.argv[1] == "--store" else load(sys.argv[1])
    bad = scan(tuples)
    for b in bad:
        print("cross-org tuple:", b)
    print(f"cross_org_scan: {len(tuples)} tuples, {len(bad)} cross-org")
    sys.exit(1 if bad else 0)
