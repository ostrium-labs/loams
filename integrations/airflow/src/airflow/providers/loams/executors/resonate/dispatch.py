# SPDX-License-Identifier: Apache-2.0
"""The executor's core, with no Airflow imports (D2093).

:class:`Dispatcher` turns workloads into Resonate promises and promise states
back into *infrastructure outcomes*. It never decides whether a task succeeded
as far as Airflow is concerned: the task reports its own state through the
Execution API, exactly as on any executor. What the dispatcher reports is
what only Resonate knows:

- the try was claimed by a worker (``running``);
- the task process exited (``success`` for exit code 0, ``failed`` otherwise);
- no worker claimed it before ``claim_timeout`` (``failed``; the dispatcher
  cancels the promise first so a late worker cannot start it);
- the worker that ran it lost its lease, i.e. died (``failed``, settled by the
  next worker that was handed the try: a started try is never run twice);
- the worker could not start the process, or refused the workload (``failed``);
- the promise timed out (``failed``).

The executor maps these onto ``BaseExecutor.change_state``; the scheduler then
applies the task's retry policy, which creates a new try and a new promise.
"""

from __future__ import annotations

import logging
import time
from collections import OrderedDict
from collections.abc import Callable, Hashable
from dataclasses import dataclass, field
from typing import Any, Literal

from airflow.providers.loams.executors.resonate import ids
from airflow.providers.loams.executors.resonate.client import (
    PENDING,
    REJECTED,
    REJECTED_CANCELED,
    REJECTED_TIMEDOUT,
    RESOLVED,
    PromiseRecord,
    ResonateClient,
    ResonateError,
    Unavailable,
)
from airflow.providers.loams.executors.resonate.config import ExecutorSettings

logger = logging.getLogger(__name__)

#: Version of the promise param and settle value schemas below.
SCHEMA_VERSION = 1

#: ``param["kind"]`` values the worker accepts.
KIND_TASK = "airflow.execute_task"
KIND_CALLBACK = "airflow.execute_callback"

#: ``value["outcome"]`` values a worker settles with.
OUTCOME_EXITED = "exited"
OUTCOME_LEASE_LOST = "lease_lost"
OUTCOME_SPAWN_FAILED = "spawn_failed"
OUTCOME_REFUSED = "refused"
OUTCOME_TIMED_OUT = "timed_out"
#: ``value["outcome"]`` values the executor settles with.
OUTCOME_CLAIM_TIMEOUT = "claim_timeout"
OUTCOME_REVOKED = "revoked"
OUTCOME_TERMINATED = "scheduler_terminated"

#: Tag keys on every promise this executor creates.
TAG_TARGET = "resonate:target"
TAG_KIND = "loams:af.kind"
TAG_NAMESPACE = "loams:af.ns"
TAG_TEAM = "loams:af.team"
TAG_QUEUE = "loams:af.queue"
TAG_DAG = "loams:af.dag"
TAG_RUN = "loams:af.run"
TAG_TASK = "loams:af.task"
TAG_TI = "loams:af.ti"

State = Literal["running", "success", "failed"]


@dataclass(frozen=True)
class Submission:
    """One workload, ready for Resonate."""

    key: Hashable
    promise_id: str
    queue: str
    group: str
    param: dict[str, Any]
    tags: dict[str, str]
    timeout_at_ms: int


@dataclass(frozen=True)
class Outcome:
    key: Hashable
    state: State
    info: str


@dataclass
class Tracked:
    key: Hashable
    promise_id: str
    queue: str
    claim_deadline_ms: int
    claimed: bool = False
    adopted: bool = False


@dataclass
class SubmitResult:
    #: ``submitted``: the promise exists and is tracked. ``retry``: Resonate
    #: was unavailable; offer the workload again later. ``failed``: give up
    #: and report the workload failed with ``reason``.
    status: Literal["submitted", "retry", "failed"]
    reason: str = ""


@dataclass
class Dispatcher:
    client: ResonateClient
    settings: ExecutorSettings
    clock: Callable[[], float] = time.time
    _tracked: OrderedDict[Hashable, Tracked] = field(default_factory=OrderedDict)
    _ready: list[Outcome] = field(default_factory=list)
    _attempts: dict[Hashable, int] = field(default_factory=dict)
    _next_poll: float = 0.0

    # -- bookkeeping --------------------------------------------------------

    def now_ms(self) -> int:
        return int(self.clock() * 1000)

    def tracked(self, key: Hashable) -> Tracked | None:
        return self._tracked.get(key)

    def keys(self) -> list[Hashable]:
        return list(self._tracked)

    def in_flight(self, queue: str | None = None) -> int:
        if queue is None:
            return len(self._tracked)
        return sum(1 for t in self._tracked.values() if t.queue == queue)

    def has_capacity(self, queue: str, pending_for_queue: int = 0) -> bool:
        """Whether one more workload for ``queue`` fits its limit."""
        limits = self.settings.queue_limits
        limit = limits.get(queue, limits.get("*"))
        if limit is None:
            return True
        return self.in_flight(queue) + pending_for_queue < limit

    def _track(self, key: Hashable, promise_id: str, queue: str, created_ms: int) -> Tracked:
        tracked = Tracked(
            key=key,
            promise_id=promise_id,
            queue=queue,
            claim_deadline_ms=created_ms + int(self.settings.claim_timeout * 1000),
        )
        self._tracked[key] = tracked
        return tracked

    # -- submit -------------------------------------------------------------

    def submit(self, sub: Submission) -> SubmitResult:
        try:
            record = self.client.promise_create(sub.promise_id, sub.timeout_at_ms, sub.param, sub.tags)
        except Unavailable as exc:
            return self._submit_retry(sub.key, f"Resonate unavailable: {exc}")
        except ResonateError as exc:
            if exc.status >= 500:
                return self._submit_retry(sub.key, f"Resonate error: {exc}")
            self._attempts.pop(sub.key, None)
            return SubmitResult("failed", f"Resonate refused promise {sub.promise_id}: {exc}")
        self._attempts.pop(sub.key, None)
        tracked = self._track(sub.key, sub.promise_id, sub.queue, self.now_ms())
        if not record.pending:
            # The id is deterministic, so a settled promise is this very try,
            # already finished (a resubmission after a scheduler restart).
            outcome = self._settled_outcome(tracked, record, resubmitted=True)
            self._tracked.pop(sub.key, None)
            if outcome is not None:
                self._ready.append(outcome)
        return SubmitResult("submitted")

    def _submit_retry(self, key: Hashable, reason: str) -> SubmitResult:
        attempts = self._attempts.get(key, 0) + 1
        self._attempts[key] = attempts
        if attempts >= self.settings.submit_max_attempts:
            self._attempts.pop(key, None)
            return SubmitResult("failed", f"{reason} (after {attempts} attempts)")
        logger.warning("submit of %s failed (attempt %d): %s", key, attempts, reason)
        return SubmitResult("retry", reason)

    # -- adopt --------------------------------------------------------------

    def adopt(self, key: Hashable, promise_id: str, queue: str) -> bool:
        """Take over a try another scheduler submitted. ``False``: not ours.

        Adoption is by id: the id is a function of the task instance key, so
        no search is needed (D2095).
        """
        try:
            record = self.client.promise_get(promise_id)
        except (Unavailable, ResonateError) as exc:
            logger.warning("cannot read %s for adoption: %s", promise_id, exc)
            return False
        if record is None:
            return False
        tracked = self._track(key, promise_id, queue, record.created_at or self.now_ms())
        tracked.adopted = True
        if not record.pending:
            outcome = self._settled_outcome(tracked, record, resubmitted=True)
            self._tracked.pop(key, None)
            if outcome is not None:
                self._ready.append(outcome)
        return True

    # -- cancel -------------------------------------------------------------

    def cancel(self, promise_id: str, outcome: str, reason: str) -> None:
        """Stop delivery, then settle the promise as cancelled. Never raises."""
        try:
            self.client.task_halt(promise_id)
        except (Unavailable, ResonateError) as exc:
            logger.warning("task.halt %s failed: %s", promise_id, exc)
        try:
            self.client.promise_settle(
                promise_id,
                REJECTED_CANCELED,
                {"v": SCHEMA_VERSION, "outcome": outcome, "reason": reason},
            )
        except (Unavailable, ResonateError) as exc:
            logger.warning("cancel of %s failed: %s", promise_id, exc)

    def revoke(self, key: Hashable, reason: str = "revoked by the scheduler") -> bool:
        """Cancel a try and forget it, without reporting anything."""
        self._attempts.pop(key, None)
        tracked = self._tracked.pop(key, None)
        if tracked is None:
            return False
        self.cancel(tracked.promise_id, OUTCOME_REVOKED, reason)
        return True

    def cancel_all(self, reason: str) -> None:
        for key in list(self._tracked):
            tracked = self._tracked.pop(key)
            self.cancel(tracked.promise_id, OUTCOME_TERMINATED, reason)

    # -- poll ---------------------------------------------------------------

    def poll(self, *, force: bool = False) -> list[Outcome]:
        """Read a batch of in-flight promises and return what changed."""
        out, self._ready = self._ready, []
        now = self.clock()
        if not force and now < self._next_poll:
            return out
        self._next_poll = now + self.settings.sync_interval
        batch = list(self._tracked.values())[: self.settings.sync_batch_size]
        for tracked in batch:
            # Round robin: whatever is read goes to the back of the queue.
            self._tracked.move_to_end(tracked.key)
            try:
                outcome = self._poll_one(tracked)
            except (Unavailable, ResonateError) as exc:
                logger.warning("poll of %s failed: %s", tracked.promise_id, exc)
                continue
            if outcome is not None:
                out.append(outcome)
        return out

    def _poll_one(self, tracked: Tracked) -> Outcome | None:
        record = self.client.promise_get(tracked.promise_id)
        now_ms = self.now_ms()
        if record is None:
            self._tracked.pop(tracked.key, None)
            return Outcome(tracked.key, "failed", f"promise {tracked.promise_id} is gone")
        if record.state != PENDING:
            self._tracked.pop(tracked.key, None)
            return self._settled_outcome(tracked, record, resubmitted=False)
        if record.timeout_at and record.timeout_at <= now_ms:
            # The server projects an expired promise as rejected_timedout.
            self._tracked.pop(tracked.key, None)
            return Outcome(tracked.key, "failed", f"task timed out ({tracked.promise_id})")
        if tracked.claimed:
            return None
        marker = self.client.promise_get(ids.started_marker_id(tracked.promise_id))
        if marker is not None:
            tracked.claimed = True
            worker = (marker.param_json() or {}).get("worker", "")
            return Outcome(tracked.key, "running", worker or tracked.promise_id)
        if now_ms >= tracked.claim_deadline_ms:
            self._tracked.pop(tracked.key, None)
            reason = (
                f"no worker claimed {tracked.promise_id} within "
                f"{self.settings.claim_timeout:g}s (queue {tracked.queue!r})"
            )
            self.cancel(tracked.promise_id, OUTCOME_CLAIM_TIMEOUT, reason)
            return Outcome(tracked.key, "failed", reason)
        return None

    def _settled_outcome(
        self, tracked: Tracked, record: PromiseRecord, *, resubmitted: bool
    ) -> Outcome | None:
        try:
            value = record.value_json() or {}
        except ValueError:
            value = {}
        if not isinstance(value, dict):
            value = {}
        outcome = value.get("outcome", "")
        reason = str(value.get("reason") or "")
        key = tracked.key
        if record.state == RESOLVED:
            rc = value.get("returncode")
            if outcome == OUTCOME_EXITED and rc == 0:
                return Outcome(key, "success", tracked.promise_id)
            return Outcome(key, "failed", f"task process exited with code {rc}")
        if record.state == REJECTED_TIMEDOUT:
            return Outcome(key, "failed", f"task timed out ({tracked.promise_id})")
        if record.state == REJECTED_CANCELED:
            if outcome == OUTCOME_REVOKED and not resubmitted:
                return None
            return Outcome(key, "failed", f"cancelled ({outcome or 'externally'}): {reason}")
        if record.state == REJECTED:
            return Outcome(key, "failed", f"{outcome or 'rejected'}: {reason}".rstrip(": "))
        return Outcome(key, "failed", f"unexpected promise state {record.state!r}")


def settled_ok(record: PromiseRecord) -> bool:
    """Whether a settled promise says the process exited 0."""
    if record.state != RESOLVED:
        return False
    value = record.value_json() or {}
    return value.get("outcome") == OUTCOME_EXITED and value.get("returncode") == 0


__all__ = [
    "KIND_CALLBACK",
    "KIND_TASK",
    "REJECTED",
    "SCHEMA_VERSION",
    "Dispatcher",
    "Outcome",
    "Submission",
    "SubmitResult",
    "Tracked",
    "settled_ok",
]
