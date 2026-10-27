#!/usr/bin/env python3
"""The compute compatibility matrix (OC1 OC0 Task 11, DX1 DX0 Task 7).

Merges a conformance run's JUnit files with each product's strict
`xfail.toml` and renders `docs/guides/compute/compatibility.md`: per product,
the version, the Loams Postgres endpoint mode, the verdict, the counts, the
expected failures with their issues, and the run's date. A product with no
JUnit file in the run reads "not run"; nothing is guessed.

  matrix.py render --junit-dir DIR [--out FILE] [--date YYYY-MM-DD]
                   [--loams-version V] [--commit SHA] [--compute-dir DIR]
  matrix.py --self-test        # matrix_from_fixture: the fixture renders as expected

The JUnit files are found by name anywhere under --junit-dir (the nightly
downloads one artifact directory per job). Stdlib only (Python 3.11+).
"""

from __future__ import annotations

import argparse
import datetime as dt
import difflib
import json
import sys
from dataclasses import dataclass, field
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(Path(__file__).resolve().parent))
import xfail_check  # noqa: E402  (a sibling script, not a package)

FIXTURE = ROOT / "conformance" / "compute" / "testdata" / "matrix"


@dataclass(frozen=True)
class Row:
    """One product leg of the matrix."""

    section: str
    name: str
    xfail: str  # path under the compute dir
    junit: tuple[str, ...]  # file names the leg's run writes
    mode: str
    version: str  # a format string over pins.env keys
    note: str = ""


ROWS: tuple[Row, ...] = (
    Row(
        "Durable execution",
        "Loams Postgres databases and endpoint modes (DX0 Task 2)",
        "durable/xfail.toml",
        ("durable.junit.xml",),
        "direct, txn",
        "Postgres client {PSQL_IMAGE_TAG}",
    ),
    Row(
        "Durable execution",
        "Temporal server (compat mode), Loams Postgres 16",
        "temporal/xfail.toml",
        ("temporal-pg16.junit.xml", "temporal-pg16-persistence.junit.xml"),
        "direct",
        "Temporal {TEMPORAL_VERSION}",
    ),
    Row(
        "Durable execution",
        "Temporal server (compat mode), Loams Postgres 17",
        "temporal/pg17/xfail.toml",
        ("temporal-pg17.junit.xml", "temporal-pg17-persistence.junit.xml"),
        "direct",
        "Temporal {TEMPORAL_VERSION}",
        "expected-failure leg: Temporal lists Postgres 13-16",
    ),
    Row(
        "Durable execution",
        "Temporal history corpus replays",
        "temporal/corpus/xfail.toml",
        ("corpus.junit.xml",),
        "none (files)",
        "Go SDK {TEMPORAL_SDK_GO}, TypeScript SDK {TEMPORAL_SDK_TS}",
    ),
    Row(
        "Durable execution",
        "DBOS Transact (Python)",
        "dbos-py/xfail.toml",
        ("dbos-py.junit.xml", "dbos-py-suite.junit.xml"),
        "direct",
        "dbos-transact-py {DBOS_PY}",
    ),
    Row(
        "Durable execution",
        "DBOS Transact (TypeScript)",
        "dbos-ts/xfail.toml",
        ("dbos-ts.junit.xml", "dbos-ts-suite.junit.xml"),
        "direct",
        "dbos-transact-ts {DBOS_TS}",
    ),
    Row(
        "Durable execution",
        "DBOS Transact (Go)",
        "dbos-go/xfail.toml",
        ("dbos-go.junit.xml", "dbos-go-suite.junit.xml"),
        "direct",
        "dbos-transact-golang {DBOS_GO}",
    ),
    Row(
        "Durable execution",
        "DBOS Transact (Java)",
        "dbos-java/xfail.toml",
        ("dbos-java.junit.xml", "dbos-java-suite.junit.xml"),
        "direct",
        "dbos-transact-java {DBOS_JAVA}",
    ),
    Row(
        "Durable execution",
        "Airflow with the Edge executor",
        "airflow/xfail.toml",
        ("airflow.junit.xml", "airflow-edge.junit.xml"),
        "direct",
        "Airflow {AIRFLOW_VERSION}, edge3 {AIRFLOW_EDGE3}",
    ),
    Row(
        "Durable execution",
        "Airflow with the LocalExecutor (comparison)",
        "airflow/local/xfail.toml",
        ("airflow-local.junit.xml",),
        "direct",
        "Airflow {AIRFLOW_VERSION}",
    ),
)


@dataclass
class Result:
    row: Row
    verdict: str
    version: str
    passed: int = 0
    failed: int = 0
    expected: int = 0
    skipped: int = 0
    entries: list[xfail_check.Entry] = field(default_factory=list)
    problems: list[str] = field(default_factory=list)


def read_pins(path: Path) -> dict[str, str]:
    pins: dict[str, str] = {}
    if not path.is_file():
        return pins
    for line in path.read_text(encoding="utf-8").splitlines():
        line = line.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        k, v = line.split("=", 1)
        pins[k] = v
    # Derived: an image's tag without registry, repository or digest.
    if "PSQL_IMAGE" in pins:
        pins["PSQL_IMAGE_TAG"] = pins["PSQL_IMAGE"].split("@")[0].rsplit("/", 1)[-1]
    return pins


def version_of(row: Row, pins: dict[str, str]) -> str:
    class Missing(dict):
        def __missing__(self, key: str) -> str:
            return "?"

    return row.version.format_map(Missing(pins))


def find_junit(junit_dir: Path | None, names: tuple[str, ...]) -> list[Path]:
    if junit_dir is None or not junit_dir.is_dir():
        return []
    found: list[Path] = []
    for n in names:
        found.extend(sorted(p for p in junit_dir.rglob(n) if p.is_file()))
    return found


def evaluate(row: Row, compute_dir: Path, junit_dir: Path | None, pins: dict[str, str]) -> Result:
    version = version_of(row, pins)
    xfail_path = compute_dir / row.xfail
    if not xfail_path.parent.is_dir():
        return Result(row, "not merged", version)
    entries: list[xfail_check.Entry] = []
    problems: list[str] = []
    if xfail_path.is_file():
        entries, problems = xfail_check.load_entries(xfail_path)
    else:
        problems.append(f"{row.xfail} is missing")
    files = find_junit(junit_dir, row.junit)
    if not files:
        return Result(row, "not run", version, entries=entries, problems=problems)
    cases, junit_problems = xfail_check.load_cases(files)
    problems += junit_problems
    if not cases and not junit_problems:
        problems.append("the JUnit files hold no test cases")
    problems += xfail_check.check(cases, entries)
    listed = lambda c: any(xfail_check.matches(p, c) for e in entries for p in e.tests)  # noqa: E731
    res = Result(
        row,
        "red" if problems else "green",
        version,
        passed=sum(1 for c in cases if c.status == "passed"),
        failed=sum(1 for c in cases if c.status == "failed"),
        expected=sum(1 for c in cases if c.status == "failed" and listed(c)),
        skipped=sum(1 for c in cases if c.status == "skipped"),
        entries=entries,
        problems=problems,
    )
    return res


def corpus_summary(compute_dir: Path) -> str:
    """Scenarios per phase and the history count, from the committed corpus."""
    corpus = compute_dir / "temporal" / "corpus"
    try:
        scenarios = json.loads((corpus / "scenarios.json").read_text(encoding="utf-8"))["scenarios"]
        histories = json.loads((corpus / "manifest.json").read_text(encoding="utf-8"))["histories"]
    except (OSError, KeyError, ValueError):
        return ""
    phases: dict[str, int] = {}
    for s in scenarios:
        phases[s["phase"]] = phases.get(s["phase"], 0) + 1
    langs: dict[str, int] = {}
    servers = sorted({h.get("loams_postgres_major", "?") for h in histories})
    for h in histories:
        langs[h["language"]] = langs.get(h["language"], 0) + 1
    per_phase = ", ".join(f"{p} {n}" for p, n in sorted(phases.items()))
    per_lang = ", ".join(f"{n} {lang}" for lang, n in sorted(langs.items()))
    return (
        f"{len(scenarios)} scenarios ({per_phase}); {len(histories)} histories ({per_lang}); "
        f"generated against: {', '.join(servers)}"
    )


def cell(s: str) -> str:
    return s.replace("|", "\\|").replace("\n", " ")


def render(results: list[Result], *, date: str, loams_version: str, commit: str, corpus: str) -> str:
    out: list[str] = []
    out.append("# Compute compatibility matrix")
    out.append("")
    out.append(
        "Generated by `scripts/compute/matrix.py` from a run of the compute conformance "
        "harness (`conformance/compute/`); do not edit by hand. Each product runs "
        "unmodified on Loams Postgres and is judged by its own suite plus the Loams "
        "cases, strictly against its `xfail.toml` (design 68 §12, design 74 §13)."
    )
    out.append("")
    out.append(f"- Run date: {date}")
    out.append(f"- Loams: {loams_version}" + (f" (commit `{commit}`)" if commit else ""))
    out.append(
        "- Verdicts: **green** (every failure is a listed expected failure), **red** "
        "(an unlisted failure, an unexpected pass or a stale entry), **not run** (no "
        "result in this run), **not merged** (the product's harness is not on this branch)."
    )
    sections: dict[str, list[Result]] = {}
    for r in results:
        sections.setdefault(r.row.section, []).append(r)
    for section, rows in sections.items():
        out.append("")
        out.append(f"## {section}")
        out.append("")
        out.append("| Product | Version | Endpoint mode | Verdict | Passed | Failed (expected) | Skipped | Expected failures |")
        out.append("|---|---|---|---|---|---|---|---|")
        for r in rows:
            name = r.row.name + (f" ({r.row.note})" if r.row.note else "")
            if r.verdict in ("not run", "not merged"):
                counts = ("–", "–", "–")
            else:
                counts = (str(r.passed), f"{r.failed} ({r.expected})", str(r.skipped))
            xf = "; ".join(f"`{e.id}` ({e.issue})" for e in r.entries) or "none"
            if r.verdict == "not merged":
                xf = "–"
            out.append(
                f"| {cell(name)} | {cell(r.version)} | {cell(r.row.mode)} | **{r.verdict}** | "
                f"{counts[0]} | {counts[1]} | {counts[2]} | {cell(xf)} |"
            )
        if section == "Durable execution" and corpus:
            out.append("")
            out.append(f"The corpus: {corpus}.")
        problems = [(r, p) for r in rows for p in r.problems]
        if problems:
            out.append("")
            out.append("Problems:")
            out.append("")
            for r, p in problems:
                out.append(f"- {r.row.name}: {p}")
    out.append("")
    return "\n".join(out)


def build(compute_dir: Path, junit_dir: Path | None, *, date: str, loams_version: str, commit: str) -> str:
    pins = read_pins(compute_dir / "pins.env")
    results = [evaluate(r, compute_dir, junit_dir, pins) for r in ROWS]
    return render(results, date=date, loams_version=loams_version, commit=commit, corpus=corpus_summary(compute_dir))


def self_test() -> int:
    """matrix_from_fixture: the fixture's JUnit set renders the expected page."""
    got = build(
        FIXTURE / "compute",
        FIXTURE / "junit",
        date="2026-10-27",
        loams_version="0.0.0-fixture",
        commit="0000000",
    )
    want = (FIXTURE / "expected.md").read_text(encoding="utf-8")
    if got != want:
        sys.stdout.writelines(
            difflib.unified_diff(want.splitlines(True), got.splitlines(True), "expected.md", "rendered")
        )
        print("matrix_from_fixture: FAIL")
        return 1
    print("matrix_from_fixture: ok")
    return 0


def main(argv: list[str]) -> int:
    if argv == ["--self-test"]:
        return self_test()
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    sub = ap.add_subparsers(dest="cmd", required=True)
    r = sub.add_parser("render")
    r.add_argument("--junit-dir", type=Path)
    r.add_argument("--out", type=Path)
    r.add_argument("--date", default=dt.date.today().isoformat())
    r.add_argument("--loams-version", default="dev")
    r.add_argument("--commit", default="")
    r.add_argument("--compute-dir", type=Path, default=ROOT / "conformance" / "compute")
    args = ap.parse_args(argv)
    page = build(args.compute_dir, args.junit_dir, date=args.date, loams_version=args.loams_version, commit=args.commit)
    if args.out:
        args.out.parent.mkdir(parents=True, exist_ok=True)
        args.out.write_text(page, encoding="utf-8")
    else:
        sys.stdout.write(page)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
