# SPDX-License-Identifier: Apache-2.0
from __future__ import annotations

import argparse
import threading
from concurrent.futures import ThreadPoolExecutor
from typing import Any

import pytest

from airflow.providers.loams.executors.resonate import ids
from airflow.providers.loams.executors.resonate.client import ExecuteMessage
from airflow.providers.loams.executors.resonate.dispatch import KIND_TASK, SCHEMA_VERSION
from airflow.providers.loams.executors.resonate.worker import (
    Worker,
    WorkerSettings,
    groups_from_args,
)
from tests.executors.fakes import FakeClock, FakeProc, FakeResonate, FakeRunner

GROUP = "af.default.default"
PID = ids.task_promise_id("default", ids.TaskRef("dag", "run", "task", 1))
API = "http://airflow-api-server:8080/execution/"


def param(**overrides: Any) -> dict[str, Any]:
    base: dict[str, Any] = {
        "v": SCHEMA_VERSION,
        "kind": KIND_TASK,
        "workload": {"type": "ExecuteTask", "token": "secret", "ti": {"task_id": "task"}},
        "execution_api_url": API,
        "display_name": "ti-1",
    }
    base.update(overrides)
    return base


def make(
    *procs: FakeProc,
    runner: FakeRunner | None = None,
    sleep_hook: Any = None,
    **settings: Any,
) -> tuple[Worker, FakeResonate, FakeClock, FakeRunner]:
    clock = FakeClock()
    fake = FakeResonate(clock=clock)
    runner = runner or FakeRunner(*procs)
    opts: dict[str, Any] = {
        "groups": (GROUP,),
        "pid": "w1",
        "ttl_ms": 6_000,
        "kill_grace": 1.0,
        "tick": 0.5,
    }
    opts.update(settings)

    def sleep(seconds: float) -> None:
        clock.sleep(seconds)
        if sleep_hook is not None:
            sleep_hook(clock.now)

    worker = Worker(client=fake, settings=WorkerSettings(**opts), runner=runner, clock=clock, sleep=sleep)
    return worker, fake, clock, runner


def submit(fake: FakeResonate, clock: FakeClock, timeout_s: float = 3600, **p: Any) -> ExecuteMessage:
    fake.promise_create(
        PID,
        int((clock() + timeout_s) * 1000),
        param(**p),
        {"resonate:target": ids.anycast_address(GROUP)},
    )
    return fake.take(GROUP)[0]


def test_runs_heartbeats_and_resolves_with_the_exit_code() -> None:
    worker, fake, clock, runner = make(FakeProc(rc=0, polls=10))
    msg = submit(fake, clock)
    assert worker.handle(msg) == "exited"
    rec = fake.promises[PID].record()
    assert rec.state == "resolved"
    assert rec.value_json()["outcome"] == "exited"
    assert rec.value_json()["returncode"] == 0
    assert len(fake.heartbeats) >= 2
    assert all(hb == ("w1", PID, 1) for hb in fake.heartbeats)
    [(workload, server, title)] = runner.started
    assert workload["token"] == "secret" and server == API and title == "ti-1"
    marker = fake.promises[ids.started_marker_id(PID)].record()
    assert marker.param_json()["worker"].endswith("/w1")
    assert "resonate:target" not in marker.tags  # a marker is never delivered


def test_nonzero_exit_is_resolved_with_its_code() -> None:
    worker, fake, clock, _ = make(FakeProc(rc=3, polls=1))
    assert worker.handle(submit(fake, clock)) == "exited"
    assert fake.value_of(PID)["returncode"] == 3


def test_a_lost_race_does_nothing() -> None:
    worker, fake, clock, runner = make(FakeProc(rc=0, polls=0))
    msg = submit(fake, clock)
    assert worker.handle(msg) == "exited"
    assert worker.handle(msg) == "lost"
    assert len(runner.started) == 1


def test_a_bad_param_is_refused_not_redelivered() -> None:
    worker, fake, clock, runner = make()
    msg = submit(fake, clock, v=99)
    assert worker.handle(msg) == "refused"
    rec = fake.promises[PID].record()
    assert rec.state == "rejected"
    assert rec.value_json()["outcome"] == "refused"
    assert runner.started == []


def test_a_started_try_is_never_run_twice() -> None:
    worker, fake, clock, runner = make()
    msg = submit(fake, clock)
    # Worker "dead" claimed it, wrote the marker, started the process, died.
    got = fake.task_acquire(msg.task_id, msg.version, "dead", 6_000)
    assert got is not None
    fake.promise_create(ids.started_marker_id(PID), got.promise.timeout_at, {"worker": "h/dead"}, {})
    redelivered = fake.expire_lease(PID)
    fake.take(GROUP)
    assert worker.handle(redelivered) == "lease_lost"
    rec = fake.promises[PID].record()
    assert rec.state == "rejected"
    assert rec.value_json()["outcome"] == "lease_lost"
    assert "h/dead" in rec.value_json()["reason"]
    assert runner.started == []


def test_a_claim_released_before_start_runs_normally() -> None:
    worker, fake, clock, runner = make(FakeProc(rc=0, polls=0))
    msg = submit(fake, clock)
    got = fake.task_acquire(msg.task_id, msg.version, "other", 6_000)
    assert got is not None
    redelivered = fake.expire_lease(PID)  # no marker: it never started
    assert worker.handle(redelivered) == "exited"
    assert len(runner.started) == 1


def test_spawn_failure_rejects() -> None:
    worker, fake, clock, _ = make(runner=FakeRunner(error=OSError("no python")))
    assert worker.handle(submit(fake, clock)) == "spawn_failed"
    value = fake.value_of(PID)
    assert value["outcome"] == "spawn_failed" and "no python" in value["reason"]


def test_revocation_stops_the_process() -> None:
    proc = FakeProc(polls=None)
    holder: dict[str, FakeResonate] = {}

    def revoke_at_3s(now: float) -> None:
        fake = holder["fake"]
        if now >= start + 3 and fake.promises[PID].state == "pending":
            fake.task_halt(PID)
            fake.promise_settle(PID, "rejected_canceled", {"outcome": "revoked"})

    worker, fake, clock, _ = make(proc, sleep_hook=revoke_at_3s)
    holder["fake"] = fake
    start = clock()
    assert worker.handle(submit(fake, clock)) == "cancelled"
    assert proc.terminated and not proc.killed
    assert fake.value_of(PID)["outcome"] == "revoked"


def test_the_deadline_terminates_then_kills() -> None:
    proc = FakeProc(polls=None, ignores_term=True)
    worker, fake, clock, _ = make(proc)
    assert worker.handle(submit(fake, clock, timeout_s=5)) == "timed_out"
    assert proc.terminated and proc.killed
    # The server timed the promise out at its deadline.
    assert fake.promises[PID].record().state == "rejected_timedout"


def test_the_execution_api_url_can_be_overridden() -> None:
    worker, fake, clock, runner = make(FakeProc(polls=0), execution_api_url="http://api.internal/execution/")
    worker.handle(submit(fake, clock))
    assert runner.started[0][1] == "http://api.internal/execution/"


def test_second_stop_signal_abandons_the_task() -> None:
    proc = FakeProc(polls=None)

    def stop_twice(now: float) -> None:
        if now >= start + 2 and not worker._stopping.is_set():
            worker.request_stop()
            worker.request_stop()

    worker, fake, clock, _ = make(proc, sleep_hook=stop_twice)
    start = clock()
    assert worker.handle(submit(fake, clock)) == "abandoned"
    assert proc.terminated
    # Not settled: the lease lapses and the next worker settles it lease_lost.
    assert fake.promises[PID].state == "pending"


def test_unavailable_resonate_while_claiming() -> None:
    worker, fake, clock, runner = make()
    msg = submit(fake, clock)
    fake.unavailable.add("task.acquire")
    assert worker.handle(msg) == "acquire_failed"
    fake.unavailable.clear()
    fake.unavailable.add("promise.get")
    assert worker.handle(msg) == "released"
    assert fake.tasks[PID].state == "pending"
    assert runner.started == []


def test_offer_respects_concurrency_and_draining() -> None:
    worker, _, _, _ = make(concurrency=1)
    gate = threading.Event()
    ran: list[str] = []

    def blocking_handle(msg: ExecuteMessage) -> str:
        gate.wait(5)
        ran.append(msg.task_id)
        return "exited"

    worker.handle = blocking_handle  # type: ignore[method-assign]
    pool = ThreadPoolExecutor(max_workers=2)
    try:
        assert worker.offer(ExecuteMessage("a", 0), pool)
        assert not worker.offer(ExecuteMessage("b", 0), pool)
        gate.set()
    finally:
        pool.shutdown(wait=True)
    assert ran == ["a"]
    assert worker.offer(ExecuteMessage("c", 0), ThreadPoolExecutor(max_workers=1))
    worker.request_stop()
    assert not worker.offer(ExecuteMessage("d", 0), ThreadPoolExecutor(max_workers=1))


def test_settings_validation() -> None:
    with pytest.raises(ValueError):
        WorkerSettings(groups=("Bad_Group",))
    with pytest.raises(ValueError):
        WorkerSettings(ttl_ms=1000)
    assert WorkerSettings().pid.startswith("af-")


def test_groups_from_args() -> None:
    args = argparse.Namespace(groups="", queues="default, gpu", namespace="default", team=None)
    assert groups_from_args(args) == ("af.default.default", "af.default.gpu")
    args.team = "team-a"
    assert groups_from_args(args) == ("af.default.team-a.default", "af.default.team-a.gpu")
    args.groups = "af.x.y"
    assert groups_from_args(args) == ("af.x.y",)
