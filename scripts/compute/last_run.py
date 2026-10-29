#!/usr/bin/env python3
"""The Temporal gateway's combined last run (coordinator ruling 2026-10-28).

The guide's matrix (matrix.py temporal-gateway) reads one JUnit file and one
run label. The SDK legs run in different jobs and on different days, so each
leg's JUnit is committed on its own under
conformance/compute/temporal/gateway/last-run/ and listed, with its run ID,
date and mode, in last-run.toml. This combines them:

  last_run.py            # write last-run.junit.xml and last-run.txt
  last_run.py --check    # fail when the committed pair differs from a fresh combine

Only a run's own SDKs' cases are kept (a name ending in `_<sdk>` or containing
`_<sdk>_`), so a case shared by every leg, such as the profile's
`gateway_profile_serving`, cannot hide one run under another, and no SDK
appears in two runs. last-run.txt has one line per run: the SDKs, then the
run's label. Stdlib only.
"""

from __future__ import annotations

import re
import sys
import tomllib
import xml.etree.ElementTree as ET
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import junit  # noqa: E402

GATEWAY = HERE.parent.parent / "conformance/compute/temporal/gateway"
SUITE = "temporal-gateway"


def owned(name: str, sdk: str) -> bool:
    """Whether a case belongs to `sdk`: `gateway_hello_go`, `gateway_dx1_differential_go_hello`."""
    return re.search(rf"_{re.escape(sdk)}(_|$)", name) is not None


def combine(gateway: Path) -> tuple[str, str]:
    """The combined JUnit text and the run lines."""
    spec = tomllib.loads((gateway / "last-run.toml").read_text(encoding="utf-8"))
    names = tomllib.loads((gateway / "methods.toml").read_text(encoding="utf-8"))["sdks"]
    root = ET.Element("testsuites", name=SUITE)
    lines: list[str] = []
    seen: dict[str, str] = {}
    for r in spec["run"]:
        sdks: list[str] = r["sdks"]
        for sdk in sdks:
            if sdk not in names:
                raise SystemExit(f"last_run.py: {r['junit']} names SDK {sdk!r}, which methods.toml does not list")
            if sdk in seen:
                raise SystemExit(f"last_run.py: SDK {sdk!r} is in {seen[sdk]} and {r['junit']}")
            seen[sdk] = r["junit"]
        cases = [c for c in junit.read(gateway / r["junit"]) if any(owned(c.name, s) for s in sdks)]
        if not cases:
            raise SystemExit(f"last_run.py: {r['junit']} has no case of {sdks}")
        suite = ET.SubElement(root, "testsuite", name=SUITE, tests=str(len(cases)),
                              failures=str(sum(c.status == "fail" for c in cases)),
                              errors=str(sum(c.status == "error" for c in cases)),
                              skipped=str(sum(c.status == "skip" for c in cases)))
        props = ET.SubElement(suite, "properties")
        ET.SubElement(props, "property", name="run", value=r["run"])
        ET.SubElement(props, "property", name="source", value=r["junit"])
        for c in cases:
            tc = ET.SubElement(suite, "testcase", classname=c.suite or SUITE, name=c.name)
            tag = junit.STATUSES[c.status]
            if tag:
                ET.SubElement(tc, tag, message=c.message)
        lines.append(", ".join(names[s] for s in sdks) + ": " + r["run"])
    ET.indent(root)
    return ET.tostring(root, encoding="unicode") + "\n", "\n".join(lines) + "\n"


def main(argv: list[str]) -> int:
    gateway = GATEWAY
    xml_text, txt = combine(gateway)
    out_xml, out_txt = gateway / "last-run.junit.xml", gateway / "last-run.txt"
    if argv == ["--check"]:
        stale = [p.name for p, want in ((out_xml, xml_text), (out_txt, txt))
                 if not p.exists() or p.read_text(encoding="utf-8") != want]
        if stale:
            print(f"last_run.py: {', '.join(stale)} stale; run scripts/compute/last_run.py", file=sys.stderr)
            return 1
        return 0
    if argv:
        print(__doc__, file=sys.stderr)
        return 2
    out_xml.write_text(xml_text, encoding="utf-8")
    out_txt.write_text(txt, encoding="utf-8")
    print(f"last_run.py: {txt.count(chr(10))} runs combined into {out_xml.name}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
