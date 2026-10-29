#!/usr/bin/env python3
"""Normalise a Temporal history (protojson) for comparison (DX0 Task 4).

Two generations of the same scenario differ in what the server or the
worker picks at run time; this masks exactly that and keeps the rest:

  masked  event times and every other timestamp (keys ending in `Time` or
          `Timestamp`, and RFC 3339 times inside strings, such as a
          schedule's workflow ID); run IDs (keys ending in `RunId`, and any
          UUID inside a string); identities; task tokens; request IDs; the
          worker's binary checksum and build ID; the namespace and its ID;
          the server's task IDs; `historySizeBytes` (it follows the sizes of
          the masked fields); stack traces (addresses); a continued run's
          `firstWorkflowTaskBackoff`; and times and UUIDs inside payloads
          (a local activity marker's replay time, a schedule's action time),
          which are shown decoded when masked
  kept    event IDs, event types, their order, and every other attribute
          (payloads, timeouts, retry policies, failures, markers, search
          attributes, memo, SDK metadata)

The History message carries no batch boundaries, so there are none to keep.

  normalize.py <history.json>               # the normalised JSON on stdout
  normalize.py --compare <a.json> <b.json>  # exit 0 when equal after masking
  normalize.py --self-test                  # normalize_selftest
Stdlib only.
"""

from __future__ import annotations

import base64
import binascii
import json
import re
import sys
from pathlib import Path

MASK_KEYS = {
    "identity": "<identity>",
    "requestId": "<request-id>",
    "taskToken": "<task-token>",
    "binaryChecksum": "<binary-checksum>",
    "buildId": "<build-id>",
    "namespace": "<namespace>",
    "namespaceId": "<namespace-id>",
    "parentWorkflowNamespace": "<namespace>",
    "parentWorkflowNamespaceId": "<namespace-id>",
    "taskId": "<task-id>",
    "historySizeBytes": "<size>",
    "stackTrace": "<stack-trace>",
    # The delay before a continued run's first task: how long the previous
    # task took to complete, not a configured value.
    "firstWorkflowTaskBackoff": "<backoff>",
}
UUID = re.compile(r"[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}")
RFC3339 = re.compile(r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(\.\d+)?(Z|[+-]\d{2}:\d{2})")


def _mask(key: str | None, v):
    if key is not None and key.endswith(("Time", "Timestamp", "_time")) and isinstance(v, (str, dict)):
        return "<time>"
    if isinstance(v, dict):
        out = {k: _mask(k, x) for k, x in v.items()}
        # A worker's sticky queue is named after its host and process.
        if out.get("kind") == "TASK_QUEUE_KIND_STICKY" and "name" in out:
            out["name"] = "<sticky-queue>"
        return out
    if isinstance(v, list):
        out = [_mask(key, x) for x in v]
        # SDK flag sets, written in map order.
        if key in ("langUsedFlags", "coreUsedFlags"):
            out = sorted(out)
        return out
    if key is not None:
        if key in MASK_KEYS:
            return MASK_KEYS[key]
        if key.endswith("RunId") or key == "runId":
            return "<run-id>"
        if key.endswith(("Time", "Timestamp", "_time")) and isinstance(v, (str, dict)):
            return "<time>"
    if isinstance(v, str):
        if key == "data":
            text = _b64_text(v)
            if text is not None:
                masked = _mask_text(text)
                try:
                    # A JSON payload (an SDK's marker data): mask by key too,
                    # such as the TypeScript SDK's complete_time.
                    doc = json.loads(text)
                    remasked = json.dumps(_mask(None, doc), sort_keys=True)
                    if remasked != json.dumps(doc, sort_keys=True):
                        masked = remasked
                except ValueError:
                    pass
                # Kept as base64 when nothing in it is masked, so the file
                # still reads as the server wrote it.
                return v if masked == text else "<decoded> " + masked
        return _mask_text(v)
    return v


def _mask_text(s: str) -> str:
    return RFC3339.sub("<time>", UUID.sub("<uuid>", s))


def _b64_text(v: str) -> str | None:
    try:
        raw = base64.b64decode(v, validate=True)
        return raw.decode("utf-8")
    except (binascii.Error, UnicodeDecodeError, ValueError):
        return None


def normalize(history: dict) -> dict:
    if "events" not in history or not isinstance(history["events"], list):
        raise ValueError("not a History: no `events` list")
    return _mask(None, history)


def load(path: Path) -> dict:
    return normalize(json.loads(path.read_text(encoding="utf-8")))


def dumps(h: dict) -> str:
    return json.dumps(h, indent=2, sort_keys=True) + "\n"


# Scenarios whose activities run in parallel: the server, not the code,
# decides the order in which they start and complete, and so the event IDs
# of those events. They compare with that order free (`activity_order_free`).
ORDER_FREE = {"activity_parallel_fanout_10"}
_ACTIVITY_DONE = {"EVENT_TYPE_ACTIVITY_TASK_STARTED", "EVENT_TYPE_ACTIVITY_TASK_COMPLETED"}


def _without_ids(e: dict) -> str:
    def strip(v):
        if isinstance(v, dict):
            return {k: strip(x) for k, x in v.items() if k not in ("eventId", "startedEventId")}
        if isinstance(v, list):
            return [strip(x) for x in v]
        return v
    return json.dumps(strip(e), sort_keys=True)


def activity_order_free(h: dict) -> dict:
    """The history with its activity started/completed events as a multiset:
    the sequence of event types stays, their attributes are sorted."""
    events = h["events"]
    done = sorted(_without_ids(e) for e in events if e.get("eventType") in _ACTIVITY_DONE)
    rest = [e for e in events if e.get("eventType") not in _ACTIVITY_DONE]
    return dict(h, events=rest, activityEvents=done, eventTypes=[e.get("eventType") for e in events])


def compare(a: dict, b: dict) -> list[str]:
    """Differences after masking, as readable lines (empty when equal)."""
    ea, eb = a["events"], b["events"]
    out = []
    if len(ea) != len(eb):
        out.append(f"{len(ea)} events vs {len(eb)}")
    for i, (x, y) in enumerate(zip(ea, eb)):
        if x != y:
            out.append(
                f"event {i + 1}: {x.get('eventType')} #{x.get('eventId')} differs from "
                f"{y.get('eventType')} #{y.get('eventId')}"
            )
            if len(out) > 20:
                break
    rest_a = {k: v for k, v in a.items() if k != "events"}
    rest_b = {k: v for k, v in b.items() if k != "events"}
    if rest_a != rest_b:
        out.append("non-event fields differ")
    return out


def self_test() -> int:
    def ev(i, t, attrs, when, run):
        return {
            "eventId": str(i), "eventTime": when, "eventType": t, "taskId": str(1000 + i * 7),
            "workflowExecutionStartedEventAttributes" if t == "EVENT_TYPE_WORKFLOW_EXECUTION_STARTED" else "attrs":
                dict(attrs, originalExecutionRunId=run, identity=f"{i}@host-{run[:4]}", firstExecutionRunId=run),
        }

    def hist(run, when, order=(1, 2)):
        events = {
            1: ev(1, "EVENT_TYPE_WORKFLOW_EXECUTION_STARTED", {"workflowType": {"name": "hello"}}, when, run),
            2: ev(2, "EVENT_TYPE_WORKFLOW_TASK_SCHEDULED", {"taskQueue": {"name": "corpus-go"}}, when, run),
        }
        return {"events": [events[i] for i in order]}

    r1, r2 = "0b6f3c8a-1111-4a2b-9c3d-123456789abc", "9e8d7c6b-2222-4f5e-8d7c-cba987654321"
    a = normalize(hist(r1, "2026-10-24T10:00:00.123Z"))
    b = normalize(hist(r2, "2026-10-25T11:30:00Z"))
    c = normalize(hist(r1, "2026-10-24T10:00:00.123Z", order=(2, 1)))
    d = normalize({"events": [dict(hist(r1, "2026-10-24T10:00:00Z")["events"][0], eventType="EVENT_TYPE_OTHER")]
                   + hist(r1, "2026-10-24T10:00:00Z")["events"][1:]})
    s = normalize({"events": [{"eventId": "1", "result": "corpus-go-schedule_triggered-wf-2026-10-24T10:00:00Z"}]})
    checks = {
        "masked fields compare equal": compare(a, b) == [],
        "a reordered pair does not": compare(a, c) != [],
        "a changed event type does not": compare(a, d) != [],
        "event ids and types are kept": [e["eventId"] for e in a["events"]] == ["1", "2"],
        "times inside strings are masked": s["events"][0]["result"] == "corpus-go-schedule_triggered-wf-<time>",
        "SDK flag sets are sorted": normalize({"events": [{"sdkMetadata": {"langUsedFlags": [7, 3]}}]})["events"][0]["sdkMetadata"]["langUsedFlags"] == [3, 7],
    }
    bad = [k for k, ok in checks.items() if not ok]
    for k, ok in checks.items():
        print(f"{'ok  ' if ok else 'FAIL'} normalize_selftest: {k}")
    return 1 if bad else 0


def main(argv: list[str]) -> int:
    if argv == ["--self-test"]:
        return self_test()
    if len(argv) == 3 and argv[0] == "--compare":
        a, b = load(Path(argv[1])), load(Path(argv[2]))
        if Path(argv[1]).name.split(".")[0] in ORDER_FREE:
            a, b = activity_order_free(a), activity_order_free(b)
        diff = compare(a, b)
        for line in diff:
            print(f"{argv[1]} vs {argv[2]}: {line}")
        return 1 if diff else 0
    if len(argv) == 1:
        sys.stdout.write(dumps(load(Path(argv[0]))))
        return 0
    print(__doc__, file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
