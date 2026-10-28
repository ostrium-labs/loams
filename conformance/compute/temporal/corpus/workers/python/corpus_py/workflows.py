"""The corpus's Python workflows: one class per scenario, registered under the
scenario's name, mirroring workers/ts/src/workflows.ts and
workers/go/scenarios/scenarios.go. A change here changes the histories:
regenerate the corpus in the same PR.

Python, like TypeScript, has no SideEffect (workflow code runs in a
deterministic sandbox), so side_effect_and_version_marker records the
version marker only.
"""

from __future__ import annotations

import asyncio
from datetime import timedelta

from temporalio import workflow
from temporalio.common import RetryPolicy, SearchAttributeKey
from temporalio.exceptions import (
    ActivityError,
    ApplicationError,
    TimeoutError,
    is_cancelled_exception,
)

with workflow.unsafe.imports_passed_through():
    from corpus_py import activities

ONCE = RetryPolicy(maximum_attempts=1)
THIRTY = timedelta(seconds=30)
SEARCH_ATTRIBUTE = SearchAttributeKey.for_keyword("CorpusKeyword")


async def _greet(name: str) -> str:
    return await workflow.execute_activity(
        activities.greet, name, start_to_close_timeout=THIRTY, retry_policy=ONCE
    )


def _timeout_kind(err: BaseException) -> str:
    if isinstance(err, ActivityError) and isinstance(err.cause, TimeoutError):
        kind = err.cause.type
        return f"TIMEOUT_TYPE_{kind.name if kind is not None else 'UNSPECIFIED'}"
    return f"not a timeout: {err}"


# --- DX1 ----------------------------------------------------------------------


@workflow.defn(name="hello")
class Hello:
    @workflow.run
    async def run(self, name: str) -> str:
        return await _greet(name)


@workflow.defn(name="two_activities_sequential")
class TwoActivitiesSequential:
    @workflow.run
    async def run(self) -> str:
        a = await _greet("one")
        b = await _greet("two")
        return f"{a}, {b}"


@workflow.defn(name="activity_parallel_fanout_10")
class ActivityParallelFanout10:
    @workflow.run
    async def run(self) -> list[str]:
        return list(await asyncio.gather(*(_greet(f"n{i}") for i in range(10))))


@workflow.defn(name="signal_then_complete")
class SignalThenComplete:
    def __init__(self) -> None:
        self._v: str | None = None

    @workflow.signal(name="go")
    def go(self, v: str) -> None:
        self._v = v

    @workflow.run
    async def run(self) -> str:
        await workflow.wait_condition(lambda: self._v is not None)
        return f"got {self._v}"


@workflow.defn(name="query_while_running")
class QueryWhileRunning:
    def __init__(self) -> None:
        self._state = "waiting"
        self._done = False

    @workflow.signal(name="finish")
    def finish(self) -> None:
        self._done = True

    @workflow.query(name="state")
    def state(self) -> str:
        return self._state

    @workflow.run
    async def run(self) -> str:
        await workflow.wait_condition(lambda: self._done)
        self._state = "done"
        return self._state


@workflow.defn(name="cancel_requested")
class CancelRequested:
    @workflow.run
    async def run(self) -> None:
        await asyncio.sleep(3600)


@workflow.defn(name="terminated")
class Terminated:
    def __init__(self) -> None:
        self._done = False

    @workflow.signal(name="finish")
    def finish(self) -> None:
        self._done = True

    @workflow.run
    async def run(self) -> None:
        await workflow.wait_condition(lambda: self._done)


@workflow.defn(name="workflow_failed_application_error")
class WorkflowFailedApplicationError:
    @workflow.run
    async def run(self) -> None:
        raise ApplicationError("boom", "detail", type="CorpusBoom", non_retryable=True)


@workflow.defn(name="activity_heartbeat_details")
class ActivityHeartbeatDetails:
    @workflow.run
    async def run(self) -> int:
        return await workflow.execute_activity(
            activities.heartbeat_thrice,
            start_to_close_timeout=THIRTY,
            heartbeat_timeout=timedelta(seconds=10),
            retry_policy=ONCE,
        )


# --- DX2 ----------------------------------------------------------------------


@workflow.defn(name="timer_short")
class TimerShort:
    @workflow.run
    async def run(self) -> None:
        await asyncio.sleep(1)


@workflow.defn(name="timer_cancelled")
class TimerCancelled:
    @workflow.run
    async def run(self) -> str:
        long = asyncio.create_task(asyncio.sleep(3600))
        await asyncio.sleep(0.1)
        long.cancel()
        try:
            await long
        except asyncio.CancelledError:
            return "cancelled"
        raise ApplicationError("the long timer was not cancelled")


@workflow.defn(name="activity_retry_backoff_3_attempts")
class ActivityRetryBackoff3Attempts:
    @workflow.run
    async def run(self) -> int:
        return await workflow.execute_activity(
            activities.fail_twice,
            start_to_close_timeout=timedelta(seconds=10),
            retry_policy=RetryPolicy(
                initial_interval=timedelta(seconds=1), backoff_coefficient=2.0, maximum_attempts=3
            ),
        )


@workflow.defn(name="activity_start_to_close_timeout")
class ActivityStartToCloseTimeout:
    @workflow.run
    async def run(self) -> str:
        try:
            await workflow.execute_activity(
                activities.sleep_for,
                5000,
                start_to_close_timeout=timedelta(seconds=1),
                retry_policy=ONCE,
            )
            return "no timeout"
        except ActivityError as err:
            return _timeout_kind(err)


@workflow.defn(name="activity_schedule_to_start_timeout")
class ActivityScheduleToStartTimeout:
    @workflow.run
    async def run(self) -> str:
        try:
            await workflow.execute_activity(
                activities.greet,
                "nobody",
                task_queue="corpus-no-worker",
                schedule_to_start_timeout=timedelta(seconds=1),
                start_to_close_timeout=timedelta(seconds=1),
                retry_policy=ONCE,
            )
            return "no timeout"
        except ActivityError as err:
            return _timeout_kind(err)


@workflow.defn(name="activity_heartbeat_timeout")
class ActivityHeartbeatTimeout:
    @workflow.run
    async def run(self) -> str:
        try:
            await workflow.execute_activity(
                activities.sleep_for,
                5000,
                start_to_close_timeout=THIRTY,
                heartbeat_timeout=timedelta(seconds=1),
                retry_policy=ONCE,
            )
            return "no timeout"
        except ActivityError as err:
            return _timeout_kind(err)


@workflow.defn(name="signal_with_start")
class SignalWithStart:
    def __init__(self) -> None:
        self._v: str | None = None

    @workflow.signal(name="go")
    def go(self, v: str) -> None:
        self._v = v

    @workflow.run
    async def run(self) -> str:
        await workflow.wait_condition(lambda: self._v is not None)
        return f"started with {self._v}"


@workflow.defn(name="child_greeter")
class ChildGreeter:
    @workflow.run
    async def run(self, name: str) -> str:
        return await _greet(name)


@workflow.defn(name="child_sleeper")
class ChildSleeper:
    @workflow.run
    async def run(self) -> None:
        await asyncio.sleep(3600)


@workflow.defn(name="child_workflow")
class ChildWorkflow:
    @workflow.run
    async def run(self) -> str:
        return await workflow.execute_child_workflow(
            ChildGreeter.run, "child", id=f"{workflow.info().workflow_id}-child"
        )


@workflow.defn(name="child_workflow_cancelled")
class ChildWorkflowCancelled:
    @workflow.run
    async def run(self) -> str:
        child = await workflow.start_child_workflow(
            ChildSleeper.run, id=f"{workflow.info().workflow_id}-child"
        )
        child.cancel()
        try:
            await child
        except BaseException as err:  # noqa: BLE001 - asyncio.CancelledError is a BaseException
            if is_cancelled_exception(err) or isinstance(err, asyncio.CancelledError):
                return "child cancelled"
            raise
        raise ApplicationError("the child was not cancelled")


@workflow.defn(name="continue_as_new_3")
class ContinueAsNew3:
    @workflow.run
    async def run(self, n: int) -> int:
        if n < 3:
            workflow.continue_as_new(n + 1)
        return n


@workflow.defn(name="id_reuse_allow_duplicate_failed_only")
class IdReuseAllowDuplicateFailedOnly:
    @workflow.run
    async def run(self, fail: bool) -> str:
        if fail:
            raise ApplicationError("first run fails", type="CorpusFirstRun", non_retryable=True)
        return "second run"


@workflow.defn(name="id_conflict_fail")
class IdConflictFail:
    def __init__(self) -> None:
        self._done = False

    @workflow.signal(name="finish")
    def finish(self) -> None:
        self._done = True

    @workflow.run
    async def run(self) -> None:
        await workflow.wait_condition(lambda: self._done)


@workflow.defn(name="local_activity")
class LocalActivity:
    @workflow.run
    async def run(self) -> str:
        return await workflow.execute_local_activity(
            activities.greet, "local", start_to_close_timeout=timedelta(seconds=10)
        )


@workflow.defn(name="side_effect_and_version_marker")
class SideEffectAndVersionMarker:
    @workflow.run
    async def run(self) -> str:
        v = 1 if workflow.patched("corpus-change") else 0
        return f"version {v}"


# --- DX3 ----------------------------------------------------------------------


@workflow.defn(name="search_attributes_upsert")
class SearchAttributesUpsert:
    @workflow.run
    async def run(self) -> None:
        workflow.upsert_search_attributes([SEARCH_ATTRIBUTE.value_set("first")])
        await asyncio.sleep(0.1)
        workflow.upsert_search_attributes([SEARCH_ATTRIBUTE.value_set("second")])


@workflow.defn(name="memo")
class Memo:
    @workflow.run
    async def run(self) -> None:
        workflow.upsert_memo({"step": "upserted", "n": 2})


@workflow.defn(name="large_payload_1mb")
class LargePayload1MB:
    @workflow.run
    async def run(self) -> int:
        s = await workflow.execute_activity(
            activities.large, 1 << 20, start_to_close_timeout=THIRTY, retry_policy=ONCE
        )
        return len(s)


# --- DX4 ----------------------------------------------------------------------


@workflow.defn(name="update_accepted_completed")
class UpdateAcceptedCompleted:
    def __init__(self) -> None:
        self._total = 0
        self._done = False

    @workflow.signal(name="finish")
    def finish(self) -> None:
        self._done = True

    @workflow.update(name="increment")
    def increment(self, by: int) -> int:
        self._total += by
        return self._total

    @increment.validator
    def validate_increment(self, by: int) -> None:
        if by < 0:
            raise ApplicationError("increment must not be negative")

    @workflow.run
    async def run(self) -> str:
        await workflow.wait_condition(lambda: self._done)
        return f"total {self._total}"


@workflow.defn(name="update_rejected")
class UpdateRejected:
    def __init__(self) -> None:
        self._total = 0
        self._done = False

    @workflow.signal(name="finish")
    def finish(self) -> None:
        self._done = True

    @workflow.update(name="increment")
    def increment(self, by: int) -> int:
        self._total += by
        return self._total

    @increment.validator
    def validate_increment(self, by: int) -> None:
        if by < 0:
            raise ApplicationError("increment must not be negative")

    @workflow.run
    async def run(self) -> str:
        await workflow.wait_condition(lambda: self._done)
        return f"total {self._total}"


WORKFLOWS = [
    Hello,
    TwoActivitiesSequential,
    ActivityParallelFanout10,
    SignalThenComplete,
    QueryWhileRunning,
    CancelRequested,
    Terminated,
    WorkflowFailedApplicationError,
    ActivityHeartbeatDetails,
    TimerShort,
    TimerCancelled,
    ActivityRetryBackoff3Attempts,
    ActivityStartToCloseTimeout,
    ActivityScheduleToStartTimeout,
    ActivityHeartbeatTimeout,
    SignalWithStart,
    ChildWorkflow,
    ChildWorkflowCancelled,
    ContinueAsNew3,
    IdReuseAllowDuplicateFailedOnly,
    IdConflictFail,
    LocalActivity,
    SideEffectAndVersionMarker,
    SearchAttributesUpsert,
    Memo,
    LargePayload1MB,
    UpdateAcceptedCompleted,
    UpdateRejected,
    # The children the scenarios start.
    ChildGreeter,
    ChildSleeper,
]
