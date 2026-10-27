# SPDX-License-Identifier: Apache-2.0
"""ResonateExecutor against a fake Resonate, with real Airflow 3.3 workloads."""

from __future__ import annotations

import uuid
from pathlib import Path
from types import SimpleNamespace
from typing import Any

import pytest

pytest.importorskip("airflow.executors.workloads")

from airflow.executors.workloads import BundleInfo, ExecuteTask, TaskInstanceDTO
from airflow.models.taskinstancekey import TaskInstanceKey
from airflow.utils.state import TaskInstanceState

from airflow.providers.loams.executors.resonate import ids
from airflow.providers.loams.executors.resonate_executor import LoamsResonateExecutor, ResonateExecutor
from tests.executors.fakes import FakeClock, FakeResonate

RUN = "manual__2026-10-27T10:00:00+00:00"


@pytest.fixture
def fake(monkeypatch: pytest.MonkeyPatch) -> FakeResonate:
    clock = FakeClock()
    resonate = FakeResonate(clock=clock)
    monkeypatch.setattr(ResonateExecutor, "client_factory", lambda settings: resonate)
    monkeypatch.setattr(ResonateExecutor, "clock", staticmethod(clock))
    return resonate


def executor() -> ResonateExecutor:
    ex = ResonateExecutor(parallelism=8)
    ex.start()
    return ex


def workload(
    task_id: str = "load",
    try_number: int = 1,
    queue: str = "default",
    executor_config: dict[str, Any] | None = None,
) -> ExecuteTask:
    ti = TaskInstanceDTO(
        id=uuid.uuid4(),
        task_id=task_id,
        dag_id="etl",
        run_id=RUN,
        try_number=try_number,
        dag_version_id=uuid.uuid4(),
        map_index=-1,
        queue=queue,
        pool_slots=1,
        priority_weight=1,
        executor_config=executor_config,
    )
    return ExecuteTask(
        ti=ti,
        dag_rel_path=Path("etl.py"),
        token="workload-token",
        bundle_info=BundleInfo(name="dags-folder", version=None),
        log_path="dag_id=etl/load.log",
    )


def promise_id(task_id: str = "load", try_number: int = 1) -> str:
    return ids.task_promise_id("default", ids.TaskRef("etl", RUN, task_id, try_number))


def claim_and_finish(fake: FakeResonate, pid: str, rc: int | None) -> int:
    group = ids.target_group("default", "default")
    queue = fake.queues[group]
    msg = next(m for m in queue if m.task_id == pid)
    queue.remove(msg)
    got = fake.task_acquire(pid, msg.version, "w1", 60_000)
    assert got is not None
    fake.promise_create(ids.started_marker_id(pid), got.promise.timeout_at, {"worker": "h/w1"}, {})
    if rc is not None:
        fake.task_fulfill(pid, got.task.version, "resolved", {"outcome": "exited", "returncode": rc})
    return got.task.version


def tick(ex: ResonateExecutor, fake: FakeResonate, seconds: float = 5.0) -> dict:
    fake.clock.advance(seconds)  # type: ignore[attr-defined]
    ex.heartbeat()
    return ex.get_event_buffer()


def test_alias() -> None:
    assert LoamsResonateExecutor is ResonateExecutor


def test_a_queued_workload_becomes_a_promise(fake: FakeResonate) -> None:
    ex = executor()
    w = workload()
    ex.queue_workload(w, session=None)
    ex.heartbeat()
    pid = promise_id()
    assert pid in fake.promises
    rec = fake.promises[pid].record()
    assert rec.tags["resonate:target"] == "poll://any@af.default.default"
    assert rec.tags["loams:af.dag"] == "etl" and rec.tags["loams:af.ti"] == str(w.ti.id)
    param = rec.param_json()
    assert param["kind"] == "airflow.execute_task"
    assert param["workload"]["token"] == "workload-token"
    assert param["workload"]["ti"]["task_id"] == "load"
    assert param["execution_api_url"].endswith("/execution/")
    assert w.ti.key in ex.running
    assert ex.queued_tasks == {}
    assert ex.get_event_buffer() == {w.ti.key: (TaskInstanceState.QUEUED, pid)}


def test_running_then_success(fake: FakeResonate) -> None:
    ex = executor()
    w = workload()
    ex.queue_workload(w, session=None)
    ex.heartbeat()
    ex.get_event_buffer()
    pid = promise_id()
    version = claim_and_finish(fake, pid, rc=None)
    assert tick(ex, fake) == {w.ti.key: (TaskInstanceState.RUNNING, "h/w1")}
    assert w.ti.key in ex.running
    fake.task_fulfill(pid, version, "resolved", {"outcome": "exited", "returncode": 0})
    assert tick(ex, fake) == {w.ti.key: (TaskInstanceState.SUCCESS, pid)}
    assert w.ti.key not in ex.running


def test_failure_outcomes_reach_the_scheduler(fake: FakeResonate) -> None:
    ex = executor()
    w = workload()
    ex.queue_workload(w, session=None)
    ex.heartbeat()
    ex.get_event_buffer()
    claim_and_finish(fake, promise_id(), rc=2)
    events = tick(ex, fake)
    state, info = events[w.ti.key]
    assert state == TaskInstanceState.FAILED and "code 2" in info


def test_claim_timeout_fails_the_try(fake: FakeResonate, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("AIRFLOW__LOAMS_RESONATE__CLAIM_TIMEOUT", "30")
    ex = executor()
    w = workload()
    ex.queue_workload(w, session=None)
    ex.heartbeat()
    ex.get_event_buffer()
    events = tick(ex, fake, 31)
    state, info = events[w.ti.key]
    assert state == TaskInstanceState.FAILED and "no worker claimed" in info
    assert fake.promises[promise_id()].state == "rejected_canceled"


def test_queue_routing_and_overrides(fake: FakeResonate, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("AIRFLOW__LOAMS_RESONATE__QUEUE_GROUPS", '{"gpu": "af.default.gpu-pool"}')
    ex = executor()
    gpu = workload("train", queue="gpu")
    special = workload(
        "special", executor_config={"loams_resonate": {"group": "af.default.big", "timeout": 60}}
    )
    plain = workload("other", queue="reports")
    for w in (gpu, special, plain):
        ex.queue_workload(w, session=None)
    ex.heartbeat()
    target = {t: fake.promises[promise_id(t)].tags["resonate:target"] for t in ("train", "special", "other")}
    assert target == {
        "train": "poll://any@af.default.gpu-pool",
        "special": "poll://any@af.default.big",
        "other": "poll://any@af.default.reports",
    }
    now_ms = int(fake.clock() * 1000)  # type: ignore[operator]
    claim_s = ex.settings.claim_timeout
    assert fake.promises[promise_id("special")].timeout_at == now_ms + int((claim_s + 60) * 1000)


def test_team_executors_use_team_groups(fake: FakeResonate) -> None:
    ex = executor()
    ex.team_name = "team-a"
    sub = ex.build_submission(workload())
    assert sub.group == "af.default.team-a.default"
    assert sub.tags["loams:af.team"] == "team-a"
    assert sub.promise_id == promise_id()  # ids do not depend on the team


def test_queue_limits_hold_work_back(fake: FakeResonate, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("AIRFLOW__LOAMS_RESONATE__QUEUE_LIMITS", '{"default": 1}')
    ex = executor()
    a, b = workload("a"), workload("b")
    ex.queue_workload(a, session=None)
    ex.queue_workload(b, session=None)
    ex.heartbeat()
    assert len(ex.running) == 1 and len(ex.queued_tasks) == 1


def test_resonate_outage_keeps_work_queued_then_fails(
    fake: FakeResonate, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("AIRFLOW__LOAMS_RESONATE__SUBMIT_MAX_ATTEMPTS", "2")
    ex = executor()
    w = workload()
    fake.unavailable.add("promise.create")
    ex.queue_workload(w, session=None)
    ex.heartbeat()
    assert w.ti.key in ex.queued_tasks and ex.get_event_buffer() == {}
    ex.heartbeat()
    assert w.ti.key not in ex.queued_tasks
    state, info = ex.get_event_buffer()[w.ti.key]
    assert state == TaskInstanceState.FAILED and "2 attempts" in info


def ti_like(task_id: str = "load", try_number: int = 1, external: str | None = None) -> Any:
    key = TaskInstanceKey(dag_id="etl", task_id=task_id, run_id=RUN, try_number=try_number)
    return SimpleNamespace(
        key=key,
        dag_id="etl",
        run_id=RUN,
        task_id=task_id,
        try_number=try_number,
        map_index=-1,
        queue="default",
        external_executor_id=external,
    )


def test_adoption_after_a_scheduler_restart(fake: FakeResonate) -> None:
    old = executor()
    old.queue_workload(workload(), session=None)
    old.heartbeat()
    claim_and_finish(fake, promise_id(), rc=None)

    new = executor()
    mine, lost = ti_like(), ti_like("never-submitted")
    assert list(new.try_adopt_task_instances([mine, lost])) == [lost]
    assert mine.key in new.running
    events = tick(new, fake)
    assert events[mine.key] == (TaskInstanceState.RUNNING, "h/w1")


def test_adoption_uses_the_external_executor_id(fake: FakeResonate) -> None:
    ex = executor()
    pid = promise_id()
    fake.promise_create(pid, 2_000_000_000_000, {"v": 1}, {})
    ti = ti_like(try_number=7, external=pid)  # try 7 would compute another id
    assert list(ex.try_adopt_task_instances([ti])) == []


def test_revoke_cancels_quietly(fake: FakeResonate) -> None:
    ex = executor()
    w = workload()
    ex.queue_workload(w, session=None)
    ex.heartbeat()
    ex.get_event_buffer()
    ex.revoke_task(ti=ti_like())
    assert w.ti.key not in ex.running
    assert fake.promises[promise_id()].state == "rejected_canceled"
    assert tick(ex, fake) == {}
    # Revoking something unknown does not raise.
    ex.revoke_task(ti=ti_like("unknown"))


def test_terminate_leaves_work_adoptable(fake: FakeResonate) -> None:
    ex = executor()
    ex.queue_workload(workload(), session=None)
    ex.heartbeat()
    ex.terminate()
    assert fake.closed
    assert fake.promises[promise_id()].state == "pending"


def test_terminate_can_cancel(fake: FakeResonate, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("AIRFLOW__LOAMS_RESONATE__CANCEL_ON_TERMINATE", "true")
    ex = executor()
    ex.queue_workload(workload(), session=None)
    ex.heartbeat()
    ex.terminate()
    assert fake.promises[promise_id()].state == "rejected_canceled"


def test_the_provider_registers_the_executor() -> None:
    from airflow.executors.base_executor import BaseExecutor

    from airflow.providers.loams.get_provider_info import get_provider_info

    [path] = get_provider_info()["executors"]
    module, _, name = path.rpartition(".")
    import importlib

    cls = getattr(importlib.import_module(module), name)
    assert issubclass(cls, BaseExecutor)
    assert cls is LoamsResonateExecutor
