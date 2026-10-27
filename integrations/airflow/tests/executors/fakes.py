# SPDX-License-Identifier: Apache-2.0
"""In-memory fakes: a Resonate server, a process runner and a clock.

``FakeResonate`` follows the protocol semantics the executor and the worker
rely on (spec/02-abstract/external.lean in the Resonate fork):

- ``promise.create`` is idempotent: an existing promise comes back unchanged;
- a promise with ``resonate:target`` gets a task in ``pending`` at version 0
  and an ``execute`` message for the target's group;
- ``task.acquire`` succeeds only on a pending task at the offered version and
  bumps the version; ``task.fulfill`` only at the acquired version;
- a lease expiry puts the task back to ``pending`` and re-sends ``execute``;
- ``task.halt`` stops delivery; ``promise.settle`` on a settled promise is a
  no-op that returns the record.
"""

from __future__ import annotations

from collections import defaultdict
from collections.abc import Callable, Iterator
from dataclasses import dataclass, field
from typing import Any

from airflow.providers.loams.executors.resonate.client import (
    PENDING,
    TASK_ACQUIRED,
    TASK_FULFILLED,
    TASK_HALTED,
    TASK_PENDING,
    Acquired,
    ExecuteMessage,
    PromiseRecord,
    TaskRecord,
    Unavailable,
    encode_value,
)


class FakeClock:
    def __init__(self, start: float = 1_800_000_000.0) -> None:
        self.now = start

    def __call__(self) -> float:
        return self.now

    def sleep(self, seconds: float) -> None:
        self.now += seconds

    def advance(self, seconds: float) -> None:
        self.now += seconds


@dataclass
class _Promise:
    id: str
    state: str
    param: dict[str, Any]
    value: dict[str, Any]
    tags: dict[str, str]
    timeout_at: int
    created_at: int
    settled_at: int | None = None

    def record(self) -> PromiseRecord:
        return PromiseRecord(
            id=self.id,
            state=self.state,
            param=dict(self.param),
            value=dict(self.value),
            tags=dict(self.tags),
            timeout_at=self.timeout_at,
            created_at=self.created_at,
            settled_at=self.settled_at,
        )


@dataclass
class _Task:
    id: str
    state: str = TASK_PENDING
    version: int = 0
    pid: str | None = None
    ttl: int | None = None

    def record(self) -> TaskRecord:
        return TaskRecord(id=self.id, state=self.state, version=self.version, pid=self.pid)


def _group_of(address: str) -> str:
    # poll://any@<group> or poll://uni@<group>/<pid>
    rest = address.split("@", 1)[1]
    return rest.split("/", 1)[0]


@dataclass
class FakeResonate:
    clock: Callable[[], float] = field(default_factory=FakeClock)
    promises: dict[str, _Promise] = field(default_factory=dict)
    tasks: dict[str, _Task] = field(default_factory=dict)
    queues: dict[str, list[ExecuteMessage]] = field(default_factory=lambda: defaultdict(list))
    calls: list[tuple[str, Any]] = field(default_factory=list)
    heartbeats: list[tuple[str, str, int]] = field(default_factory=list)
    #: Operation names that raise :class:`Unavailable`.
    unavailable: set[str] = field(default_factory=set)
    closed: bool = False

    def _now_ms(self) -> int:
        return int(self.clock() * 1000)

    def _op(self, name: str, arg: Any) -> None:
        self.calls.append((name, arg))
        if name in self.unavailable:
            raise Unavailable(f"{name}: fake outage")

    def _project(self, p: _Promise) -> None:
        if p.state == PENDING and p.timeout_at and p.timeout_at <= self._now_ms():
            p.state = "rejected_timedout"
            p.settled_at = p.timeout_at
            task = self.tasks.get(p.id)
            if task is not None:
                task.state = TASK_FULFILLED

    # -- promises -----------------------------------------------------------

    def promise_create(self, id: str, timeout_at: int, param: Any, tags: dict[str, str]) -> PromiseRecord:
        self._op("promise.create", id)
        existing = self.promises.get(id)
        if existing is not None:
            self._project(existing)
            return existing.record()
        p = _Promise(
            id=id,
            state=PENDING,
            param=encode_value(param),
            value={},
            tags=dict(tags),
            timeout_at=timeout_at,
            created_at=self._now_ms(),
        )
        self.promises[id] = p
        target = tags.get("resonate:target")
        if target:
            self.tasks[id] = _Task(id=id)
            self.queues[_group_of(target)].append(ExecuteMessage(task_id=id, version=0))
        return p.record()

    def promise_get(self, id: str) -> PromiseRecord | None:
        self._op("promise.get", id)
        p = self.promises.get(id)
        if p is None:
            return None
        self._project(p)
        return p.record()

    def promise_settle(self, id: str, state: str, value: Any) -> PromiseRecord | None:
        self._op("promise.settle", (id, state))
        p = self.promises.get(id)
        if p is None:
            return None
        self._project(p)
        if p.state == PENDING:
            p.state = state
            p.value = encode_value(value)
            p.settled_at = self._now_ms()
            task = self.tasks.get(id)
            if task is not None:
                task.state = TASK_FULFILLED
        return p.record()

    # -- tasks --------------------------------------------------------------

    def task_get(self, id: str) -> TaskRecord | None:
        self._op("task.get", id)
        task = self.tasks.get(id)
        return None if task is None else task.record()

    def task_acquire(self, id: str, version: int, pid: str, ttl_ms: int) -> Acquired | None:
        self._op("task.acquire", (id, version))
        task = self.tasks.get(id)
        p = self.promises.get(id)
        if task is None or p is None:
            return None
        self._project(p)
        if task.state != TASK_PENDING or task.version != version:
            return None
        task.state = TASK_ACQUIRED
        task.version += 1
        task.pid = pid
        task.ttl = ttl_ms
        return Acquired(task=task.record(), promise=p.record())

    def task_heartbeat(self, pid: str, task_id: str, version: int) -> None:
        self._op("task.heartbeat", task_id)
        self.heartbeats.append((pid, task_id, version))

    def task_fulfill(self, id: str, version: int, state: str, value: Any) -> bool:
        self._op("task.fulfill", (id, state))
        task = self.tasks.get(id)
        p = self.promises.get(id)
        if task is None or p is None:
            return False
        self._project(p)
        if task.state != TASK_ACQUIRED or task.version != version or p.state != PENDING:
            return False
        p.state = state
        p.value = encode_value(value)
        p.settled_at = self._now_ms()
        task.state = TASK_FULFILLED
        return True

    def task_release(self, id: str, version: int) -> bool:
        self._op("task.release", id)
        task = self.tasks.get(id)
        if task is None or task.state != TASK_ACQUIRED or task.version != version:
            return False
        task.state = TASK_PENDING
        task.pid = None
        return True

    def task_halt(self, id: str) -> bool:
        self._op("task.halt", id)
        task = self.tasks.get(id)
        if task is None or task.state == TASK_FULFILLED:
            return False
        task.state = TASK_HALTED
        task.pid = None
        return True

    # -- test controls ------------------------------------------------------

    def expire_lease(self, id: str) -> ExecuteMessage:
        """The holder died: back to pending, ``execute`` re-sent."""
        task = self.tasks[id]
        assert task.state == TASK_ACQUIRED
        task.state = TASK_PENDING
        task.pid = None
        msg = ExecuteMessage(task_id=id, version=task.version)
        target = self.promises[id].tags["resonate:target"]
        self.queues[_group_of(target)].append(msg)
        return msg

    def take(self, group: str) -> list[ExecuteMessage]:
        msgs, self.queues[group] = self.queues[group], []
        return msgs

    def value_of(self, id: str) -> Any:
        return self.promises[id].record().value_json()

    def count(self, op: str) -> int:
        return sum(1 for name, _ in self.calls if name == op)

    # -- push ---------------------------------------------------------------

    def poll(self, group: str, pid: str, stop: Callable[[], bool]) -> Iterator[ExecuteMessage]:
        self._op("poll", group)
        yield from self.take(group)

    def close(self) -> None:
        self.closed = True


class FakeProc:
    """A task process that exits with ``rc`` after ``polls`` polls."""

    _next_pid = 1000

    def __init__(self, rc: int = 0, polls: int | None = 0, ignores_term: bool = False) -> None:
        FakeProc._next_pid += 1
        self.pid = FakeProc._next_pid
        self.rc = rc
        self.polls_left = polls
        self.ignores_term = ignores_term
        self.returncode: int | None = None
        self.terminated = False
        self.killed = False

    def poll(self) -> int | None:
        if self.returncode is not None:
            return self.returncode
        if self.polls_left is not None and self.polls_left <= 0:
            self.returncode = self.rc
            return self.returncode
        if self.polls_left is not None:
            self.polls_left -= 1
        return None

    def terminate(self) -> None:
        self.terminated = True
        if not self.ignores_term:
            self.returncode = -15

    def kill(self) -> None:
        self.killed = True
        self.returncode = -9


class FakeRunner:
    def __init__(self, *procs: FakeProc, error: Exception | None = None) -> None:
        self.procs = list(procs)
        self.error = error
        self.started: list[tuple[dict[str, Any], str, str]] = []

    def start(self, workload: dict[str, Any], server: str, title: str) -> FakeProc:
        if self.error is not None:
            raise self.error
        self.started.append((workload, server, title))
        return self.procs.pop(0) if self.procs else FakeProc()
