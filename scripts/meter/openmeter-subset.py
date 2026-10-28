#!/usr/bin/env python3
"""Extract the OpenMeter API subset Loams serves (design §63 §17.2; ME1
Task 15) from OpenMeter's `api/openapi.yaml` at a pinned commit.

  scripts/meter/openmeter-subset.py <openapi.yaml> <commit> > crates/loams-meter/tests/openmeter/openapi.subset.json

Keeps the operations below and every schema they reach through `$ref`, and
records where it came from (`x-loams-source`: repository, commit, SHA-256 of
the full file). OpenMeter is Apache-2.0; only the schemas are copied.
`--check FILE` regenerates and compares (the file must not be edited by hand).
"""

import hashlib
import json
import sys

import yaml

REPO = "openmeterio/openmeter"

# (path, method): what the meter role's OpenMeter-compatible listener serves.
OPERATIONS = [
    ("/api/v1/events", "post"),
    ("/api/v1/meters", "get"),
    ("/api/v1/meters/{meterIdOrSlug}", "get"),
    ("/api/v1/meters/{meterIdOrSlug}/query", "get"),
    ("/api/v1/subjects/{subjectIdOrKey}/entitlements/{entitlementIdOrFeatureKey}/value", "get"),
    ("/api/v1/subjects/{subjectIdOrKey}/entitlements/{entitlementIdOrFeatureKey}/grants", "get"),
    ("/api/v1/subjects/{subjectIdOrKey}/entitlements/{entitlementIdOrFeatureKey}/grants", "post"),
]

KEEP = ("operationId", "parameters", "requestBody", "responses")


def refs(node, out):
    if isinstance(node, dict):
        r = node.get("$ref")
        if isinstance(r, str) and r.startswith("#/components/schemas/"):
            out.add(r.rsplit("/", 1)[1])
        for v in node.values():
            refs(v, out)
    elif isinstance(node, list):
        for v in node:
            refs(v, out)


DOC_KEYS = ("description", "example", "examples", "summary", "title")


def strip_docs(node, names=False):
    """Drops descriptions and examples: the subset is for checking shapes.
    The keys of a `properties` map are property names, not annotations, so
    a property called `title` stays."""
    if isinstance(node, dict):
        return {
            k: strip_docs(v, names=(k == "properties"))
            for k, v in node.items()
            if names or k not in DOC_KEYS
        }
    if isinstance(node, list):
        return [strip_docs(v) for v in node]
    return node


def subset(raw: bytes, commit: str) -> dict:
    doc = yaml.safe_load(raw)
    paths = {}
    for path, method in OPERATIONS:
        op = doc["paths"][path][method]
        paths.setdefault(path, {})[method] = strip_docs({k: op[k] for k in KEEP if k in op})
    schemas = doc["components"]["schemas"]
    want, done = set(), set()
    refs(paths, want)
    while want - done:
        name = sorted(want - done)[0]
        done.add(name)
        refs(schemas[name], want)
    return {
        "openapi": doc["openapi"],
        "x-loams-source": {
            "repository": REPO,
            "commit": commit,
            "file": "api/openapi.yaml",
            "sha256": hashlib.sha256(raw).hexdigest(),
            "license": "Apache-2.0",
        },
        "paths": paths,
        "components": {"schemas": {n: strip_docs(schemas[n]) for n in sorted(done)}},
    }


def main(argv):
    if len(argv) < 3:
        print(__doc__, file=sys.stderr)
        return 2
    raw = open(argv[1], "rb").read()
    out = json.dumps(subset(raw, argv[2]), indent=1, sort_keys=True) + "\n"
    if len(argv) == 5 and argv[3] == "--check":
        if open(argv[4]).read() != out:
            print(f"{argv[4]} differs from a fresh extraction", file=sys.stderr)
            return 1
        return 0
    sys.stdout.write(out)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
