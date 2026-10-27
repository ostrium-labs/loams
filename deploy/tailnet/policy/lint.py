#!/usr/bin/env python3
"""Offline sanity check of policy.hujson (no provider needed).

Strips HuJSON comments and trailing commas, parses the result, and checks the
subset both Tailscale and Headscale accept: only the sections `groups`,
`tagOwners`, `grants`, `tests`; every tag used is declared in `tagOwners`;
every test names a src. This does NOT evaluate the tests: that is the
provider's job (gitops-acl-action `test`, or headscale/policy/check.sh).

Usage: lint.py [POLICY_FILE]
"""
import json
import re
import sys
from pathlib import Path

path = Path(sys.argv[1]) if len(sys.argv) > 1 else Path(__file__).with_name("policy.hujson")
text = path.read_text(encoding="utf-8")


def strip(src: str) -> str:
    out, i, in_str = [], 0, False
    while i < len(src):
        c = src[i]
        if in_str:
            out.append(c)
            if c == "\\":
                out.append(src[i + 1])
                i += 1
            elif c == '"':
                in_str = False
        elif c == '"':
            in_str = True
            out.append(c)
        elif src.startswith("//", i):
            while i < len(src) and src[i] != "\n":
                i += 1
            continue
        elif src.startswith("/*", i):
            i = src.index("*/", i) + 2
            continue
        else:
            out.append(c)
        i += 1
    return re.sub(r",(\s*[\]}])", r"\1", "".join(out))


doc = json.loads(strip(text))
allowed = {"groups", "tagOwners", "grants", "tests"}
extra = set(doc) - allowed
if extra:
    sys.exit(f"sections outside the shared subset: {sorted(extra)}")
if not doc.get("grants"):
    sys.exit("grants is empty: that would be deny-all or allow-all depending on provider")
declared = set(doc["tagOwners"])
used = set(re.findall(r"tag:[a-z0-9][a-z0-9-]*", json.dumps({k: doc[k] for k in ("grants", "tests")})))
if used - declared:
    sys.exit(f"tags used but not in tagOwners: {sorted(used - declared)}")
for i, t in enumerate(doc.get("tests", [])):
    if "src" not in t or not (t.get("accept") or t.get("deny")):
        sys.exit(f"test #{i} needs src and accept/deny")
print(f"policy lint ok: {len(doc['grants'])} grants, {len(doc.get('tests', []))} tests, {len(declared)} tags")
