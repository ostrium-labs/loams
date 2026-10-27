#!/usr/bin/env python3
"""The DX1 differential (plan DX1 Task 10): each history the gateway wrote
for a corpus scenario, against the corpus's history of that scenario, after
the corpus's own normalisation (../corpus/normalize.py) plus the gateway's
equivalences (plan rulings X1.R12, X1.R20):

  - a sticky task queue reads as the normal queue it stands for (the gateway
    has no sticky queues in DX1, Q1156);
  - a WorkflowTaskCompleted's `sdkMetadata` keeps only its flag sets: an SDK
    without a sticky cache replays from the start and names itself
    (`sdkName`, `sdkVersion`) again in later tasks;
  - a parallel scenario (normalize.ORDER_FREE) compares the multiset of its
    events other than workflow tasks: how many completions share a workflow
    task is timing (#1225).

One JUnit case per language and scenario: gateway_dx1_differential_<lang>_<scenario>.
A language whose corpus has no history of the scenario yet (a new SDK leg
before its first compat-mode generation, D2152) reports `skip` with the
reason, never `pass`.

  compare.py --corpus DIR --gateway DIR --scenarios a,b [--langs go,ts] --junit FILE
Stdlib only.
"""

from __future__ import annotations

import argparse
import copy
import json
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent / "corpus"))
import normalize  # noqa: E402


def gateway_equivalent(h: dict) -> dict:
    def walk(v):
        if isinstance(v, dict):
            if v.get("kind") == "TASK_QUEUE_KIND_STICKY":
                return {"name": v.get("normalName"), "kind": "TASK_QUEUE_KIND_NORMAL"}
            out = {k: walk(x) for k, x in v.items()}
            meta = out.get("sdkMetadata")
            if isinstance(meta, dict):
                out["sdkMetadata"] = {k: x for k, x in meta.items() if k not in ("sdkName", "sdkVersion")}
            return out
        if isinstance(v, list):
            return [walk(x) for x in v]
        return v

    return walk(copy.deepcopy(h))


def without_workflow_tasks(h: dict) -> dict:
    """A parallel scenario's history as the multiset of its other events,
    event-ID references removed: how many completions share a workflow task
    depends on timing, on Temporal and the gateway alike (#1225)."""

    def strip(v):
        if isinstance(v, dict):
            return {k: strip(x) for k, x in v.items() if k != "eventId" and not k.endswith("EventId")}
        if isinstance(v, list):
            return [strip(x) for x in v]
        return v

    rest = [
        json.dumps(strip(e), sort_keys=True)
        for e in h["events"]
        if not str(e.get("eventType", "")).startswith("EVENT_TYPE_WORKFLOW_TASK_")
    ]
    return {"events": sorted(rest)}


def compare(corpus: Path, gateway: Path, scenario: str) -> list[str]:
    a = normalize.normalize(gateway_equivalent(json.loads(corpus.read_text())))
    b = normalize.normalize(gateway_equivalent(json.loads(gateway.read_text())))
    if scenario in normalize.ORDER_FREE:
        a, b = without_workflow_tasks(a), without_workflow_tasks(b)
    return normalize.compare(a, b)


def main() -> int:
    p = argparse.ArgumentParser()
    p.add_argument("--corpus", type=Path, required=True)
    p.add_argument("--gateway", type=Path, required=True)
    p.add_argument("--scenarios", required=True)
    p.add_argument("--langs", default="go,ts")
    p.add_argument("--junit", type=Path, required=True)
    a = p.parse_args()
    junit = HERE.parent.parent.parent.parent / "scripts" / "compute" / "junit.py"
    bad = 0
    for lang in a.langs.split(","):
        for s in a.scenarios.split(","):
            name = f"gateway_dx1_differential_{lang}_{s}"
            mine, theirs = a.gateway / lang / f"{s}.json", a.corpus / lang / f"{s}.json"
            if not theirs.is_file():
                status, msg = "skip", f"the corpus has no {lang}/{s} history yet (generate.sh --langs {lang})"
            elif not mine.is_file():
                status, msg = "fail", f"the gateway wrote no {lang}/{s} history"
            else:
                diff = compare(theirs, mine, s)
                status, msg = ("pass", "") if not diff else ("fail", "; ".join(diff[:5]))
            bad += status != "pass"
            subprocess.run(
                [sys.executable, "-I", str(junit), "add", str(a.junit), "temporal-gateway", name, status, msg],
                check=True,
            )
            print(f"{status:4} {name} {msg}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
