#!/usr/bin/env python3
"""PgDog per major: the gate on the committed inventories (NF2 Task 12; §51 §12.4,
§62 §7.2 condition 5).

Reads conformance/router/pgdog-pg<N>.tsv (written by scripts/nf1/pgdog-majors.sh)
and the [majors] table of deploy/pins/loams-postgres-images.toml, and fails when:

- a file is malformed: the header is not `component, check, result, notes`, a
  row has another number of fields, a result is not ok/differs/error, a
  component is unknown, or a check repeats;
- a supported major (or the default) has no inventory;
- a supported major (or the default) has an `error` or `differs` row in a
  component Loams uses (USED). A major is "supported through PgDog" only then.

A beta major's blocking rows are printed, not failed: they block its promotion
(NF2 Task 14), not the beta. Promotion moves the major into `supported`, and
then this check fails until the rows are gone.

    python3 -I scripts/nf1/check_pgdog_majors.py [repo]
"""

from __future__ import annotations

import re
import sys
import tomllib
from pathlib import Path

HEADER = ["component", "check", "result", "notes"]
RESULTS = {"ok", "differs", "error"}
# What pg-control renders PgDog for (crates/loams-pg-control/src/pgdog/render.rs):
# every component in the inventory is one Loams uses.
USED = {"protocol", "cancel", "auth", "tls", "query", "session", "read-only"}
COMPONENTS = USED
NAME = re.compile(r"^pgdog-pg(\d+)\.tsv$")


def parse(path: Path) -> tuple[list[dict], list[str]]:
    problems: list[str] = []
    rows: list[dict] = []
    lines = path.read_text(encoding="utf-8").split("\n")
    if lines and lines[-1] == "":
        lines.pop()
    if not lines or lines[0].split("\t") != HEADER:
        return rows, [f"{path.name}: header is not {HEADER}"]
    seen: set[tuple[str, str]] = set()
    for n, line in enumerate(lines[1:], start=2):
        fields = line.split("\t")
        if len(fields) != len(HEADER):
            problems.append(f"{path.name}:{n}: {len(fields)} fields, expected {len(HEADER)}")
            continue
        r = dict(zip(HEADER, fields))
        if r["component"] not in COMPONENTS:
            problems.append(f"{path.name}:{n}: unknown component {r['component']!r}")
        if r["result"] not in RESULTS:
            problems.append(f"{path.name}:{n}: result {r['result']!r} is not one of {sorted(RESULTS)}")
        key = (r["component"], r["check"])
        if key in seen:
            problems.append(f"{path.name}:{n}: check repeats: {r['check']}")
        seen.add(key)
        r["line"] = n
        rows.append(r)
    if not rows:
        problems.append(f"{path.name}: no rows")
    return rows, problems


def blocking(rows: list[dict]) -> list[dict]:
    return [r for r in rows if r["component"] in USED and r["result"] in ("error", "differs")]


def check(repo: Path) -> tuple[list[str], list[str]]:
    """Returns (failures, notices)."""
    failures: list[str] = []
    notices: list[str] = []
    majors = tomllib.loads((repo / "deploy/pins/loams-postgres-images.toml").read_text())["majors"]
    gated = set(majors.get("supported", [])) | {majors.get("default")}
    gated.discard(None)
    beta = set(majors.get("beta", []))
    inv_dir = repo / "conformance/router"
    found: dict[int, Path] = {}
    for p in sorted(inv_dir.glob("pgdog-pg*.tsv")):
        m = NAME.match(p.name)
        if m:
            found[int(m.group(1))] = p
    for major in sorted(gated):
        if major not in found:
            failures.append(f"supported major {major} has no conformance/router/pgdog-pg{major}.tsv")
    for major, path in sorted(found.items()):
        rows, problems = parse(path)
        failures.extend(problems)
        for r in blocking(rows):
            msg = f"{path.name}:{r['line']}: {r['result']} in {r['component']}: {r['check']}"
            if major in gated:
                failures.append(f"{msg} (major {major} is supported; §62 §7.2 condition 5)")
            else:
                status = "beta" if major in beta else "not offered"
                notices.append(f"{msg} (major {major} is {status}: blocks its promotion)")
    return failures, notices


def main(argv: list[str]) -> int:
    repo = Path(argv[0]) if argv else Path(__file__).resolve().parents[2]
    failures, notices = check(repo)
    for n in notices:
        print(f"pgdog-majors: notice: {n}")
    for f in failures:
        print(f"pgdog-majors: {f}", file=sys.stderr)
    if failures:
        return 1
    print("pgdog-majors: ok")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
