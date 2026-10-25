#!/usr/bin/env python3
"""Store, model, 50 orgs x (admin, device) tuples, 100 checks, list-objects,
a revoke and a re-check against an OpenFGA server. Prints one JSON line."""

import json
import sys
import time
import urllib.request

BASE = sys.argv[1]


def call(path, body):
    req = urllib.request.Request(
        BASE + path, data=json.dumps(body).encode(), method="POST", headers={"content-type": "application/json"}
    )
    t0 = time.perf_counter()
    out = json.load(urllib.request.urlopen(req))
    return out, (time.perf_counter() - t0) * 1000.0


store = call("/stores", {"name": "lc0-fga"})[0]["id"]
model = call(
    f"/stores/{store}/authorization-models",
    {
        "schema_version": "1.1",
        "type_definitions": [
            {"type": "user"},
            {
                "type": "org",
                "relations": {"member": {"this": {}}, "admin": {"this": {}}},
                "metadata": {
                    "relations": {
                        "member": {"directly_related_user_types": [{"type": "user"}]},
                        "admin": {"directly_related_user_types": [{"type": "user"}]},
                    }
                },
            },
            {
                "type": "device",
                "relations": {
                    "org": {"this": {}},
                    "owner": {"tupleToUserset": {"tupleset": {"relation": "org"}, "computedUserset": {"relation": "admin"}}},
                },
                "metadata": {"relations": {"org": {"directly_related_user_types": [{"type": "org"}]}}},
            },
        ],
    },
)[0]["authorization_model_id"]
S = f"/stores/{store}"
writes, checks, ok = [], [], True
for i in range(50):
    _, ms = call(
        f"{S}/write",
        {
            "authorization_model_id": model,
            "writes": {
                "tuple_keys": [
                    {"user": f"user:u{i}", "relation": "admin", "object": f"org:o{i}"},
                    {"user": f"org:o{i}", "relation": "org", "object": f"device:d{i}"},
                ]
            },
        },
    )
    writes.append(ms)
for i in range(50):
    r, ms = call(f"{S}/check", {"authorization_model_id": model, "tuple_key": {"user": f"user:u{i}", "relation": "owner", "object": f"device:d{i}"}})
    checks.append(ms)
    ok &= r["allowed"]
    r, ms = call(f"{S}/check", {"authorization_model_id": model, "tuple_key": {"user": f"user:u{i}", "relation": "owner", "object": f"device:d{(i + 1) % 50}"}})
    checks.append(ms)
    ok &= not r["allowed"]
listed = call(f"{S}/list-objects", {"authorization_model_id": model, "type": "device", "relation": "owner", "user": "user:u7"})[0]
call(f"{S}/write", {"authorization_model_id": model, "deletes": {"tuple_keys": [{"user": "user:u7", "relation": "admin", "object": "org:o7"}]}})
after = call(f"{S}/check", {"authorization_model_id": model, "tuple_key": {"user": "user:u7", "relation": "owner", "object": "device:d7"}})[0]


def p(xs, q):
    xs = sorted(xs)
    return round(xs[min(len(xs) - 1, int(q * len(xs)))], 1)


print(
    json.dumps(
        {
            "checks_correct": ok,
            "write_p50_p95_ms": [p(writes, 0.5), p(writes, 0.95)],
            "check_p50_p95_ms": [p(checks, 0.5), p(checks, 0.95)],
            "list_objects": listed,
            "revoked_check_allowed": after["allowed"],
        }
    )
)
