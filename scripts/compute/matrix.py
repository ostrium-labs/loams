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
    `scope` says what the run does not cover; an SDK of `[sdks]` with no
    case at all in the JUnit, such as a leg added since the committed run,
    reads "not yet run" in every cell and is named under the table). Every other method of the vendored `WorkflowService` is
    listed as not implemented (it answers UNIMPLEMENTED with this matrix's
    link, plan ruling X1.R18).
  - "Durable execution" (DX0 Task 7): not rendered yet. DX0 Task 7 adds its
    renderer to SECTIONS below (Temporal compat mode on 16 and 17, the
    corpus, DBOS per SDK, Airflow with Edge).

  matrix.py temporal-gateway --junit RUN.xml [--xfail xfail.toml]
            [--methods methods.toml] [--service service.proto]
            [--run "label"] [--out FILE | --splice FILE | --check FILE]

`--splice FILE` replaces the text between FILE's markers
`<!-- matrix:<section>:begin -->` and `<!-- matrix:<section>:end -->`;
`--check FILE` fails when that text differs from a fresh render. Stdlib only.
"""

from __future__ import annotations

import argparse
import re
import sys
import tomllib
from collections.abc import Callable
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
    "unrun": "not yet run",
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

    # An SDK none of whose cases is in the run: its leg was not part of it.
    unrun = [
        sdk for sdk in sdks
        if not any(t.format(sdk=sdk) in states for m in methods for t in m.get("cases", []))
    ]
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
        cells = [
            CELL["unrun"] if sdk in unrun else method_cell(templates if sdk in only else [], sdk, states)
            for sdk in sdks
        ]
        for k in m.get("known", []):
            unknown_sdks = sorted(set(k.get("sdks", sdks)) - set(sdks))
            if unknown_sdks:
                raise SystemExit(f"matrix.py: {m['name']}'s known issue names unknown SDKs: {unknown_sdks}")
            known.append(
                f"{len(known) + 1}. `{m['name']}`, "
                + ", ".join(sdks[sdk] for sdk in sdks if sdk in k.get("sdks", sdks) and sdk not in unrun)
                + f": {k['text'].strip()}"
            )
            for i, sdk in enumerate(sdks):
                if sdk in k.get("sdks", sdks) and sdk not in unrun:
                    cells[i] += f", known issue {len(known)}"
        sdk_part = "<sdk>" if len(only) != 1 else only[0]
        by = ", ".join(f"`{t.format(sdk=sdk_part)}`" for t in templates)
        if m.get("note"):
            by = f"{by}; {m['note']}" if by else m["note"]
        lines.append(f"| `{m['name']}` | " + " | ".join(cells) + f" | {by} |")
    if unrun:
        lines += [
            "",
            "Not yet run: "
            + ", ".join(sdks[sdk] for sdk in unrun)
            + ". The leg exists, but this run did not include it, so its cells say nothing yet.",
        ]
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


# The sections, in page order. DX0 Task 7 adds ("durable", "Durable execution",
# its renderer and its subcommand's arguments) here.
SECTIONS: dict[str, Callable[[argparse.Namespace], str]] = {
    "temporal-gateway": render_temporal_gateway,
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
    for p in (tg,):
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
