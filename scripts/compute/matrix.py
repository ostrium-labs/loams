#!/usr/bin/env python3
"""The compute compatibility matrix (OC1 OC0 Task 11, DX1 DX0 Task 7, DX1 Task 11).

Renders Markdown from conformance runs (JUnit, read through junit.py) and
their strict expected-failure lists (xfail.toml, read through
xfail_check.py). One section per kind of run:

  - "Temporal API (gateway)" (plan DX1 Task 11): one row per
    `WorkflowService` method of the DX1 set and one column per SDK, from the
    SDK legs' JUnit (conformance/compute/temporal/gateway/run.sh). Which
    cases exercise which method is conformance/compute/temporal/gateway/
    methods.toml (`cases`, with `{sdk}` for the SDK's key; `only` limits a
    method to some SDKs; a method without cases is "not exercised";
    `[[method.known]]` records a known issue the legs do not show, such as
    one seen only under load, and marks those SDKs' cells; a top-level
    `scope` says what the run does not cover). Every other method of the vendored `WorkflowService` is
    listed as not implemented (it answers UNIMPLEMENTED with this matrix's
    link, plan ruling X1.R18).
  - "Durable execution" (DX0 Task 7): one row per leg of the compute nightly
    (.github/workflows/compute-compat.yml): the databases and endpoint modes,
    Temporal compat mode on 16 and 17, the corpus replays, DBOS per SDK, and
    Airflow with Edge and with the LocalExecutor. Each leg's JUnit files are
    found by name under --junit-dir and checked strictly against its
    xfail.toml; a leg without results reads "not run", a product whose
    harness is absent "not merged".

  matrix.py temporal-gateway --junit RUN.xml [--xfail xfail.toml]
            [--methods methods.toml] [--service service.proto]
            [--run "label"] [--out FILE | --splice FILE | --check FILE]
  matrix.py durable [--junit-dir DIR] [--compute-dir DIR] [--date D]
            [--loams-version V] [--commit SHA] [--out FILE | --splice FILE | --check FILE]

`--splice FILE` replaces the text between FILE's markers
`<!-- matrix:<section>:begin -->` and `<!-- matrix:<section>:end -->`;
`--check FILE` fails when that text differs from a fresh render. Stdlib only.
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import re
import sys
import tomllib
from collections.abc import Callable
from dataclasses import dataclass, field
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent.parent
sys.path.insert(0, str(HERE))
import junit  # noqa: E402
import xfail_check  # noqa: E402

GATEWAY = ROOT / "conformance/compute/temporal/gateway"
SERVICE_PROTO = ROOT / "crates/loams-temporal-proto/proto/temporal/api/workflowservice/v1/service.proto"

# A cell's text per state. "xfail" cells also carry the entry's issue.
CELL = {
    "pass": "pass",
    "fail": "**fail**",
    "xfail": "expected failure",
    "skip": "skipped",
    "missing": "not run",
}


def service_methods(proto: Path) -> list[str]:
    """The RPCs of the vendored `WorkflowService`, in file order."""
    return re.findall(r"^\s*rpc\s+(\w+)\s*\(", proto.read_text(encoding="utf-8"), re.M)


def case_states(cases: list[junit.Case], xfail: Path | None) -> dict[str, tuple[str, str]]:
    """Each case's name -> (state, note); a listed failure is "xfail"."""
    entries = xfail_check.load_entries(xfail)[0] if xfail else []
    out: dict[str, tuple[str, str]] = {}
    for c in cases:
        state = {"pass": "pass", "skip": "skip"}.get(c.status, "fail")
        note = ""
        if state == "fail":
            full = f"{c.suite}::{c.name}" if c.suite else c.name
            probe = xfail_check.Case(full, c.name, "failed")
            hit = next((e for e in entries if any(xfail_check.matches(t, probe) for t in e.tests)), None)
            if hit is not None:
                state, note = "xfail", hit.issue
        out[c.name] = (state, note)
    return out


def method_cell(templates: list[str], sdk: str, states: dict[str, tuple[str, str]]) -> str:
    """One method and SDK: the worst state of the cases that exercise it."""
    if not templates:
        return "not exercised"
    order = ["fail", "xfail", "missing", "skip", "pass"]
    got = [states.get(t.format(sdk=sdk), ("missing", "")) for t in templates]
    worst = min(got, key=lambda s: order.index(s[0]))
    text = CELL[worst[0]]
    return f"{text} ({worst[1]})" if worst[1] else text


def render_temporal_gateway(args: argparse.Namespace) -> str:
    spec = tomllib.loads(Path(args.methods).read_text(encoding="utf-8"))
    sdks: dict[str, str] = spec["sdks"]
    methods: list[dict] = spec["method"]
    states = case_states(junit.read(Path(args.junit)), Path(args.xfail) if args.xfail else None)
    all_methods = service_methods(Path(args.service))
    listed = [m["name"] for m in methods]
    unknown = sorted(set(listed) - set(all_methods))
    if unknown:
        raise SystemExit(f"matrix.py: methods.toml names methods the service does not have: {unknown}")

    passed = sum(1 for s, _ in states.values() if s == "pass")
    lines = ["### Temporal API (gateway)", ""]
    if args.run:
        lines += [f"Run: {args.run}.", ""]
    if spec.get("scope"):
        lines += [f"Not covered by this run: {spec['scope'].strip()}", ""]
    lines += [
        f"SDK legs: {passed} of {len(states)} cases passed. "
        f"{len(listed)} of the {len(all_methods)} `WorkflowService` methods are implemented (design 74 §5.8, DX1).",
        "",
        "| Method | " + " | ".join(sdks.values()) + " | Exercised by |",
        "|---|" + "---|" * len(sdks) + "---|",
    ]
    known: list[str] = []
    for m in methods:
        templates = m.get("cases", [])
        only = m.get("only", list(sdks))
        cells = [method_cell(templates if sdk in only else [], sdk, states) for sdk in sdks]
        for k in m.get("known", []):
            unknown_sdks = sorted(set(k.get("sdks", sdks)) - set(sdks))
            if unknown_sdks:
                raise SystemExit(f"matrix.py: {m['name']}'s known issue names unknown SDKs: {unknown_sdks}")
            known.append(
                f"{len(known) + 1}. `{m['name']}`, "
                + ", ".join(sdks[sdk] for sdk in sdks if sdk in k.get("sdks", sdks))
                + f": {k['text'].strip()}"
            )
            for i, sdk in enumerate(sdks):
                if sdk in k.get("sdks", sdks):
                    cells[i] += f", known issue {len(known)}"
        sdk_part = "<sdk>" if len(only) != 1 else only[0]
        by = ", ".join(f"`{t.format(sdk=sdk_part)}`" for t in templates)
        if m.get("note"):
            by = f"{by}; {m['note']}" if by else m["note"]
        lines.append(f"| `{m['name']}` | " + " | ".join(cells) + f" | {by} |")
    if known:
        lines += ["", "Known issues (not shown by the cases above):", "", *known]
    rest = [n for n in all_methods if n not in listed]
    lines += [
        "",
        f"Not implemented ({len(rest)}; each answers `UNIMPLEMENTED` with a link to this page): "
        + ", ".join(f"`{n}`" for n in rest)
        + ".",
    ]
    return "\n".join(lines) + "\n"


# --- Durable execution (DX0 Task 7) ---------------------------------------------

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


DURABLE_ROWS: tuple[Row, ...] = (
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


def durable_find_junit(junit_dir: Path | None, names: tuple[str, ...]) -> list[Path]:
    if junit_dir is None or not junit_dir.is_dir():
        return []
    found: list[Path] = []
    for n in names:
        found.extend(sorted(p for p in junit_dir.rglob(n) if p.is_file()))
    return found


def durable_evaluate(row: Row, compute_dir: Path, junit_dir: Path | None, pins: dict[str, str]) -> Result:
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
    files = durable_find_junit(junit_dir, row.junit)
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



def durable_cell(s: str) -> str:
    return s.replace("|", "\\|").replace("\n", " ")


def render_durable_results(results: list[Result], *, date: str, loams_version: str, commit: str, corpus: str) -> str:
    out: list[str] = []
    out.append(f"Run: {date}; Loams {loams_version}" + (f" (commit `{commit}`)" if commit else "") + ".")
    out.append("")
    out.append("| Product | Version | Endpoint mode | Verdict | Passed | Failed (expected) | Skipped | Expected failures |")
    out.append("|---|---|---|---|---|---|---|---|")
    for r in results:
        name = r.row.name + (f" ({r.row.note})" if r.row.note else "")
        if r.verdict in ("not run", "not merged"):
            counts = ("–", "–", "–")
        else:
            counts = (str(r.passed), f"{r.failed} ({r.expected})", str(r.skipped))
        xf = "; ".join(f"`{e.id}` ({e.issue})" for e in r.entries) or "none"
        if r.verdict == "not merged":
            xf = "–"
        out.append(
            f"| {durable_cell(name)} | {durable_cell(r.version)} | {durable_cell(r.row.mode)} | **{r.verdict}** | "
            f"{counts[0]} | {counts[1]} | {counts[2]} | {durable_cell(xf)} |"
        )
    if corpus:
        out.append("")
        out.append(f"The corpus: {corpus}.")
    problems = [(r, p) for r in results for p in r.problems]
    if problems:
        out.append("")
        out.append("Problems:")
        out.append("")
        for r, p in problems:
            out.append(f"- {r.row.name}: {p}")
    out.append("")
    return "\n".join(out)


def render_durable(args: argparse.Namespace) -> str:
    """DX0 Task 7: one row per leg of the compute nightly (DX1 ruling T7.R2).
    Verdicts: green, red, not run (no JUnit for the leg) and not merged (the
    product's harness is not on this branch)."""
    compute_dir = Path(args.compute_dir)
    junit_dir = Path(args.junit_dir) if args.junit_dir else None
    pins = read_pins(compute_dir / "pins.env")
    results = [durable_evaluate(r, compute_dir, junit_dir, pins) for r in DURABLE_ROWS]
    return render_durable_results(
        results, date=args.date, loams_version=args.loams_version, commit=args.commit, corpus=corpus_summary(compute_dir)
    )


# The sections, in page order.
SECTIONS: dict[str, Callable[[argparse.Namespace], str]] = {
    "temporal-gateway": render_temporal_gateway,
    "durable": render_durable,
}


def splice(text: str, section: str, body: str) -> str:
    begin, end = f"<!-- matrix:{section}:begin -->", f"<!-- matrix:{section}:end -->"
    i, j = text.find(begin), text.find(end)
    if i < 0 or j < i:
        raise SystemExit(f"matrix.py: no {begin} ... {end} markers")
    return text[: i + len(begin)] + "\n" + body + text[j:]


def main(argv: list[str]) -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="section", required=True)
    tg = sub.add_parser("temporal-gateway", help="the Temporal gateway's SDK legs")
    tg.add_argument("--junit", required=True)
    tg.add_argument("--xfail", default=str(GATEWAY / "xfail.toml"))
    tg.add_argument("--methods", default=str(GATEWAY / "methods.toml"))
    tg.add_argument("--service", default=str(SERVICE_PROTO))
    tg.add_argument("--run", default="", help="a line naming the run (date, revision, Resonate)")
    du = sub.add_parser("durable", help="the compute nightly's durable-execution legs (DX0 Task 7)")
    du.add_argument("--junit-dir", help="the downloaded artifacts; each leg's JUnit files are found by name")
    du.add_argument("--compute-dir", default=str(ROOT / "conformance/compute"))
    du.add_argument("--date", default=dt.date.today().isoformat())
    du.add_argument("--loams-version", default="dev")
    du.add_argument("--commit", default="")
    for p in (tg, du):
        out = p.add_mutually_exclusive_group()
        out.add_argument("--out")
        out.add_argument("--splice")
        out.add_argument("--check")
    args = ap.parse_args(argv)

    body = SECTIONS[args.section](args)
    if args.out:
        Path(args.out).write_text(body, encoding="utf-8")
    elif args.splice or args.check:
        path = Path(args.splice or args.check)
        old = path.read_text(encoding="utf-8")
        new = splice(old, args.section, body)
        if args.check:
            if new != old:
                print(f"matrix.py: {path}'s {args.section} section is stale; run with --splice", file=sys.stderr)
                return 1
        elif new != old:
            path.write_text(new, encoding="utf-8")
    else:
        sys.stdout.write(body)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
