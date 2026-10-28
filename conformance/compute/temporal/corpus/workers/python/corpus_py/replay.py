"""corpus_replays_python and gateway_histories_replay_python: every Python
history in a directory replays with the pinned SDK's Replayer and zero
nondeterminism errors."""

from __future__ import annotations

import asyncio
import json
import os
import re
import sys
from pathlib import Path

from corpus_py import LANG, PREFIX

RUN = re.compile(r"(\.run\d+)?\.json$")
DEFAULT = Path(__file__).resolve().parents[3] / "histories" / LANG


def workflow_id(file_name: str) -> str:
    """The workflow ID the generator used (child IDs derive from it)."""
    return PREFIX + RUN.sub("", file_name)


async def replay_all(directory: Path) -> list[str]:
    from temporalio.client import WorkflowHistory
    from temporalio.worker import Replayer

    from corpus_py.workflows import WORKFLOWS

    files = sorted(p for p in directory.glob("*.json"))
    if not files:
        return [f"no histories in {directory}"]
    replayer = Replayer(workflows=WORKFLOWS)
    failures = []
    for p in files:
        history = WorkflowHistory.from_json(workflow_id(p.name), json.loads(p.read_text(encoding="utf-8")))
        try:
            result = await replayer.replay_workflow(history, raise_on_replay_failure=False)
        except Exception as err:  # noqa: BLE001 - one bad file must not hide the rest
            failures.append(f"{p.name}: {type(err).__name__}: {err}")
            continue
        if result.replay_failure is not None:
            failures.append(f"{p.name}: {type(result.replay_failure).__name__}: {result.replay_failure}")
        else:
            print(f"replay ok {p.name}", flush=True)
    return failures


def replay_dir(directory: str | None) -> int:
    d = Path(directory or os.environ.get("CORPUS_HISTORIES") or DEFAULT)
    failures = asyncio.run(replay_all(d))
    for f in failures:
        print(f"replay FAIL {f}", file=sys.stderr)
    print(f"corpus_replays_{LANG}: {'FAIL' if failures else 'ok'} ({d})")
    return 1 if failures else 0
