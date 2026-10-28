"""The starter: one driver per scenario (signals, queries, cancels, updates),
mirroring drive() in workers/ts/src/generate.ts."""

from __future__ import annotations

import asyncio
import json
from typing import Any

from corpus_py import LANG, QUEUE


def _json(v: Any) -> str:
    # JSON.stringify's compact form, as the TypeScript generator writes.
    return json.dumps(v, separators=(",", ":"))


def observation_file(namespace: str, observations: list[dict[str, Any]]) -> str:
    return json.dumps({"namespace": namespace, "language": LANG, "scenarios": observations}, indent=2) + "\n"


async def wait_started(client: Any, wid: str) -> None:
    """Until the run's first workflow task completed, so a signal, query,
    cancel or update lands on a running workflow."""
    while True:
        d = await client.get_workflow_handle(wid).describe()
        if (d.history_length or 0) >= 4 and not d.raw_description.HasField("pending_workflow_task"):
            return
        await asyncio.sleep(0.2)


async def drive(client: Any, name: str, wid: str) -> dict[str, Any]:
    from temporalio.client import WorkflowFailureError, WorkflowUpdateFailedError
    from temporalio.common import WorkflowIDConflictPolicy, WorkflowIDReusePolicy
    from temporalio.exceptions import WorkflowAlreadyStartedError

    o: dict[str, Any] = {"scenario": name, "workflow_id": wid, "expected": []}

    async def start(*args: Any, **extra: Any) -> Any:
        return await client.start_workflow(name, args=list(args), id=wid, task_queue=QUEUE, **extra)

    async def settle(handle: Any) -> dict[str, Any]:
        o["result"] = _json(await handle.result())
        return o

    if name == "hello":
        return await settle(await start("corpus"))
    if name == "signal_then_complete":
        h = await start()
        await wait_started(client, wid)
        await h.signal("go", "s")
        return await settle(h)
    if name == "signal_with_start":
        return await settle(await start(start_signal="go", start_signal_args=["s"]))
    if name == "query_while_running":
        h = await start()
        await wait_started(client, wid)
        state = await h.query("state", result_type=str)
        if state != "waiting":
            raise RuntimeError(f"query answered {state}")
        o["expected"].append("query state=waiting")
        await h.signal("finish")
        return await settle(h)
    if name in ("cancel_requested", "terminated"):
        h = await start()
        await wait_started(client, wid)
        if name == "cancel_requested":
            await h.cancel()
        else:
            await h.terminate(reason="corpus terminates it")
        try:
            await h.result()
        except WorkflowFailureError as err:
            o["expected"].append(f"WorkflowFailureError: {err.cause}")
            return o
        raise RuntimeError(f"the workflow completed instead of closing as {name}")
    if name == "workflow_failed_application_error":
        h = await start()
        try:
            await h.result()
        except WorkflowFailureError as err:
            o["expected"].append(f"{type(err.cause).__name__}: {err.cause}")
            return o
        raise RuntimeError("want an application error")
    if name == "continue_as_new_3":
        return await settle(await start(0))
    if name == "id_reuse_allow_duplicate_failed_only":
        policy = WorkflowIDReusePolicy.ALLOW_DUPLICATE_FAILED_ONLY
        first = await start(True, id_reuse_policy=policy)
        try:
            await first.result()
        except WorkflowFailureError:
            pass
        else:
            raise RuntimeError("the first run should fail")
        second = await start(False, id_reuse_policy=policy)
        o["expected"].append("the first run failed; the second started with the same ID")
        return await settle(second)
    if name == "id_conflict_fail":
        policy = WorkflowIDConflictPolicy.FAIL
        h = await start(id_conflict_policy=policy)
        try:
            await start(id_conflict_policy=policy)
        except WorkflowAlreadyStartedError:
            o["expected"].append("the second start was refused: WorkflowExecutionAlreadyStarted")
        else:
            raise RuntimeError("the second start should be refused")
        await h.signal("finish")
        return await settle(h)
    if name == "memo":
        return await settle(await start(memo={"initial": "set at start"}))
    if name in ("update_accepted_completed", "update_rejected"):
        h = await start()
        await wait_started(client, wid)
        by = -1 if name == "update_rejected" else 2
        try:
            total = await h.execute_update("increment", by, id=f"{wid}-update", result_type=int)
            o["expected"].append(f"update returned {total}")
            if by < 0:
                raise RuntimeError("the validator should reject a negative increment")
        except WorkflowUpdateFailedError as err:
            if by >= 0:
                raise
            o["expected"].append(f"update rejected: {err.cause or err}")
        await h.signal("finish")
        return await settle(h)
    return await settle(await start())
