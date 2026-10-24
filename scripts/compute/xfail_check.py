#!/usr/bin/env python3
"""Strict expected-failure check for the compute conformance harness.

OC1 OC0 Task 1 (design 68 §12) and DX1 DX0 (design 74 §13): a product's run
writes JUnit; its `xfail.toml` lists the tests expected to fail, each with a
reason and a tracking issue. This fails when

  - a test failed (or errored) and no entry lists it (`unlisted failure`);
  - a listed test passed (`unexpected pass`: the entry hides a fix and must
    be removed);
  - a listed pattern matched no test in the run (`stale entry`);
  - an entry is malformed (no id, tests, reason or issue; a bad `since`).

Skipped tests are neither failures nor passes. A test is named
`<classname>::<name>` when the JUnit case has a classname, else `<name>`;
an entry's `tests` are exact names or `fnmatch` globs matched against both
forms.

  xfail_check.py <junit.xml> [<junit.xml> ...] --xfail <xfail.toml>
  xfail_check.py --validate <xfail.toml> [...]

Exit 0 when the run matches the list, 1 otherwise. Stdlib only (Python 3.11+).
"""

from __future__ import annotations

import argparse
import datetime as dt
import fnmatch
import re
import sys
import tomllib
import xml.etree.ElementTree as ET
from dataclasses import dataclass
from pathlib import Path

ISSUE_RE = re.compile(
    r"^(#[0-9]+|[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+#[0-9]+|https://github\.com/[^/\s]+/[^/\s]+/(issues|pull)/[0-9]+)$"
)
ID_RE = re.compile(r"^[a-z0-9][a-z0-9_.-]*$")
ENTRY_KEYS = {"id", "tests", "reason", "issue", "since"}


@dataclass(frozen=True)
class Case:
    name: str  # `<classname>::<name>` or `<name>`
    short: str  # `<name>`
    status: str  # "passed" | "failed" | "skipped"


@dataclass
class Entry:
    id: str
    tests: list[str]
    reason: str
    issue: str
    since: str


def load_entries(path: Path) -> tuple[list[Entry], list[str]]:
    """The entries of an xfail.toml and the problems found in it."""
    problems: list[str] = []
    try:
        data = tomllib.loads(path.read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError) as e:
        return [], [f"{path}: cannot read: {e}"]
    unknown = set(data) - {"product", "xfail"}
    if unknown:
        problems.append(f"{path}: unknown top-level keys {sorted(unknown)}")
    raw = data.get("xfail", [])
    if not isinstance(raw, list):
        return [], [f"{path}: `xfail` must be an array of tables ([[xfail]])"]
    entries: list[Entry] = []
    seen: set[str] = set()
    for i, e in enumerate(raw):
        where = f"{path}: entry {i + 1}"
        if not isinstance(e, dict):
            problems.append(f"{where}: not a table")
            continue
        extra = set(e) - ENTRY_KEYS
        if extra:
            problems.append(f"{where}: unknown keys {sorted(extra)}")
        eid = e.get("id")
        if not isinstance(eid, str) or not ID_RE.match(eid):
            problems.append(f"{where}: `id` must be a lower-case slug")
            eid = f"<entry {i + 1}>"
        elif eid in seen:
            problems.append(f"{where}: duplicate id {eid!r}")
        seen.add(eid)
        where = f"{path}: {eid}"
        tests = e.get("tests")
        if (
            not isinstance(tests, list)
            or not tests
            or not all(isinstance(t, str) and t.strip() for t in tests)
        ):
            problems.append(f"{where}: `tests` must be a non-empty list of names or globs")
            tests = []
        reason = e.get("reason")
        if not isinstance(reason, str) or len(reason.strip()) < 8:
            problems.append(f"{where}: `reason` is required (say why it fails)")
            reason = ""
        issue = e.get("issue")
        if not isinstance(issue, str) or not ISSUE_RE.match(issue.strip()):
            problems.append(
                f"{where}: `issue` is required: #N, owner/repo#N or a GitHub issue or PR URL"
            )
            issue = ""
        since = e.get("since")
        if isinstance(since, dt.date):
            since = since.isoformat()
        if not isinstance(since, str) or not re.match(r"^\d{4}-\d{2}-\d{2}$", since):
            problems.append(f"{where}: `since` must be a date (YYYY-MM-DD)")
            since = ""
        entries.append(Entry(eid, list(tests), reason, issue, since))
    return entries, problems


def load_cases(paths: list[Path]) -> tuple[list[Case], list[str]]:
    cases: list[Case] = []
    problems: list[str] = []
    for p in paths:
        try:
            root = ET.parse(p).getroot()
        except (OSError, ET.ParseError) as e:
            problems.append(f"{p}: cannot read JUnit: {e}")
            continue
        for tc in root.iter("testcase"):
            name = tc.get("name") or ""
            cls = tc.get("classname") or ""
            if not name:
                problems.append(f"{p}: a testcase has no name")
                continue
            if tc.find("failure") is not None or tc.find("error") is not None:
                status = "failed"
            elif tc.find("skipped") is not None:
                status = "skipped"
            else:
                status = "passed"
            full = f"{cls}::{name}" if cls else name
            cases.append(Case(full, name, status))
    return cases, problems


def matches(pattern: str, case: Case) -> bool:
    return any(
        fnmatch.fnmatchcase(n, pattern) for n in {case.name, case.short}
    )


def check(cases: list[Case], entries: list[Entry]) -> list[str]:
    problems: list[str] = []
    covered: dict[str, Entry] = {}
    for e in entries:
        for pat in e.tests:
            hit = [c for c in cases if matches(pat, c)]
            if not hit:
                problems.append(
                    f"stale entry: {e.id}: {pat!r} matched no test in this run (remove it, or fix the name)"
                )
            for c in hit:
                covered.setdefault(c.name, e)
    for c in cases:
        e = covered.get(c.name)
        if c.status == "failed" and e is None:
            problems.append(f"unlisted failure: {c.name}")
        elif c.status == "passed" and e is not None:
            problems.append(
                f"unexpected pass: {c.name} is listed by {e.id} ({e.issue}); remove the entry"
            )
    return problems


def main(argv: list[str]) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("junit", nargs="*", type=Path)
    ap.add_argument("--xfail", type=Path, help="the product's xfail.toml")
    ap.add_argument(
        "--validate", nargs="+", type=Path, metavar="XFAIL_TOML",
        help="only check the files' entries (reason, issue, since)",
    )
    args = ap.parse_args(argv)

    if args.validate:
        bad = []
        for p in args.validate:
            _, problems = load_entries(p)
            bad.extend(problems)
        for line in bad:
            print(f"xfail: {line}")
        return 1 if bad else 0

    if not args.junit or args.xfail is None:
        ap.error("give JUnit files and --xfail, or --validate")
    entries, problems = load_entries(args.xfail)
    cases, junit_problems = load_cases(args.junit)
    problems += junit_problems
    if not cases and not junit_problems:
        problems.append("the JUnit files hold no test cases (a run that ran nothing proves nothing)")
    problems += check(cases, entries)
    counts = {s: sum(1 for c in cases if c.status == s) for s in ("passed", "failed", "skipped")}
    xfailed = sum(
        1 for c in cases if c.status == "failed" and any(matches(p, c) for e in entries for p in e.tests)
    )
    for line in problems:
        print(f"xfail: {line}")
    print(
        f"xfail: {len(cases)} cases: {counts['passed']} passed, {counts['failed']} failed "
        f"({xfailed} expected), {counts['skipped']} skipped; {len(entries)} entries; "
        f"{'FAIL' if problems else 'ok'}"
    )
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
