#!/usr/bin/env python3
"""Append one test case to a JUnit file (`oc_junit_case` in lib.sh).

  junit.py add <file.xml> <suite> <name> <pass|fail|error|skip> [message]

The file is rewritten whole on every call, so it is valid JUnit after each
case: a run that stops half-way still leaves the cases it reached. `suite`
becomes the case's classname and the <testsuite> name. Stdlib only.
"""

from __future__ import annotations

import sys
import xml.etree.ElementTree as ET
from pathlib import Path

STATUSES = {"pass": None, "fail": "failure", "error": "error", "skip": "skipped"}


def add(path: Path, suite: str, name: str, status: str, message: str) -> None:
    if status not in STATUSES:
        raise SystemExit(f"junit.py: status must be one of {sorted(STATUSES)}, not {status!r}")
    if path.exists() and path.stat().st_size > 0:
        root = ET.parse(path).getroot()
    else:
        root = ET.Element("testsuites")
    ts = next((s for s in root.findall("testsuite") if s.get("name") == suite), None)
    if ts is None:
        ts = ET.SubElement(root, "testsuite", name=suite)
    tc = ET.SubElement(ts, "testcase", classname=suite, name=name)
    child = STATUSES[status]
    if child is not None:
        el = ET.SubElement(tc, child)
        if message:
            el.set("message", message[:500])
            el.text = message
    for s in root.findall("testsuite"):
        cases = s.findall("testcase")
        s.set("tests", str(len(cases)))
        s.set("failures", str(sum(1 for c in cases if c.find("failure") is not None)))
        s.set("errors", str(sum(1 for c in cases if c.find("error") is not None)))
        s.set("skipped", str(sum(1 for c in cases if c.find("skipped") is not None)))
    ET.indent(root)
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_suffix(path.suffix + ".tmp")
    ET.ElementTree(root).write(tmp, encoding="utf-8", xml_declaration=True)
    tmp.replace(path)


def main(argv: list[str]) -> int:
    if len(argv) < 5 or argv[0] != "add":
        print(__doc__, file=sys.stderr)
        return 2
    _, file, suite, name, status, *rest = argv
    add(Path(file), suite, name, status, rest[0] if rest else "")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
