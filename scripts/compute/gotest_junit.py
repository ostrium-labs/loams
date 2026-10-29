#!/usr/bin/env python3
"""`go test -json` to JUnit, for the compute harness (DX0 Tasks 3 and 4).

  go test -json ./... | gotest_junit.py <suite> <out.xml>

Each test (subtests included, as `Parent/Sub`) becomes a case with classname
`<package>`; a test that never reported pass, fail or skip (the binary
crashed or timed out) is an error, and so is a package that failed outside
any test (a build failure, a panic in TestMain). Stdlib only.
"""

from __future__ import annotations

import json
import sys
import xml.etree.ElementTree as ET


def convert(lines, suite: str) -> ET.Element:
    tests: dict[tuple[str, str], dict] = {}
    pkg_fail: dict[str, list[str]] = {}
    pkg_out: dict[str, list[str]] = {}
    for raw in lines:
        raw = raw.strip()
        if not raw.startswith("{"):
            continue
        try:
            ev = json.loads(raw)
        except json.JSONDecodeError:
            continue
        pkg, test, action = ev.get("Package", ""), ev.get("Test"), ev.get("Action")
        if test is None:
            if action == "output":
                pkg_out.setdefault(pkg, []).append(ev.get("Output", ""))
            elif action == "fail":
                pkg_fail[pkg] = pkg_out.get(pkg, [])
            continue
        t = tests.setdefault((pkg, test), {"status": None, "output": [], "elapsed": 0.0})
        if action == "output":
            t["output"].append(ev.get("Output", ""))
        elif action in ("pass", "fail", "skip"):
            t["status"] = action
            t["elapsed"] = float(ev.get("Elapsed") or 0.0)

    root = ET.Element("testsuites")
    ts = ET.SubElement(root, "testsuite", name=suite)
    for (pkg, test), t in sorted(tests.items()):
        tc = ET.SubElement(ts, "testcase", classname=pkg, name=test, time=f"{t['elapsed']:.3f}")
        out = "".join(t["output"])[-20000:]
        if t["status"] == "fail":
            ET.SubElement(tc, "failure", message="failed").text = out
        elif t["status"] == "skip":
            ET.SubElement(tc, "skipped")
        elif t["status"] is None:
            ET.SubElement(tc, "error", message="no result (crash or timeout)").text = out
    for pkg, out in sorted(pkg_fail.items()):
        if not any(p == pkg and t["status"] == "fail" for (p, _), t in tests.items()):
            tc = ET.SubElement(ts, "testcase", classname=pkg, name="(package)")
            ET.SubElement(tc, "error", message="package failed outside a test").text = "".join(out)[-20000:]
    cases = ts.findall("testcase")
    ts.set("tests", str(len(cases)))
    ts.set("failures", str(sum(1 for c in cases if c.find("failure") is not None)))
    ts.set("errors", str(sum(1 for c in cases if c.find("error") is not None)))
    ts.set("skipped", str(sum(1 for c in cases if c.find("skipped") is not None)))
    return root


def main(argv: list[str]) -> int:
    if len(argv) != 2:
        print(__doc__, file=sys.stderr)
        return 2
    root = convert(sys.stdin, argv[0])
    ET.indent(root)
    ET.ElementTree(root).write(argv[1], encoding="utf-8", xml_declaration=True)
    ts = root.find("testsuite")
    print(f"gotest_junit: {ts.get('tests')} cases, {ts.get('failures')} failed, {ts.get('errors')} errors, {ts.get('skipped')} skipped")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
