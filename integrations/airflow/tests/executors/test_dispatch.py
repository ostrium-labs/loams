# SPDX-License-Identifier: Apache-2.0
from __future__ import annotations

from dataclasses import replace

from airflow.providers.loams.executors.resonate import ids
from airflow.providers.loams.executors.resonate.config import ExecutorSettings
from airflow.providers.loams.executors.resonate.dispatch import (
    KIND_TASK,
    SCHEMA_VERSION,
    Dispatcher,
    Submission,
)
from tests.executors.fakes import FakeClock, FakeResonate

GROUP = "af.default.default"
SETTINGS = ExecutorSettings(claim_timeout=60, sync_interval=5, submit_max_attempts=3)


def setup(settings: ExecutorSettings = SETTINGS) -> tuple[Dispatcher, FakeResonate, FakeClock]:
    clock = FakeClock()
    fake = FakeResonate(clock=clock)
    return Dispatcher(client=fake, settings=settings, clock=clock), fake, clock


def submission(key: str = "k1", try_number: int = 1, queue: str = "default") -> Submission:
    pid = ids.task_promise_id("default", ids.TaskRef("dag", "run", key, try_number))
    group = ids.target_group("default", queue)
    return Submission(
        key=key,
        promise_id=pid,
        queue=queue,
        group=group,
        param={"v": SCHEMA_VERSION, "kind": KIND_TASK, "workload": {"ti": {}}},
        tags={"resonate:target": ids.anycast_address(group)},
        timeout_at_ms=2_000_000_000_000,
    )


def claim(fake: FakeResonate, pid: str, worker: str = "w1") -> int:
    """What a worker does: acquire, then write the started marker."""
    queue = fake.queues[GROUP]
    msg = next(m for m in queue if m.task_id == pid)
    queue.remove(msg)
    got = fake.task_acquire(msg.task_id, msg.version, worker, 60_000)
    assert got is not None
    fake.promise_create(ids.started_marker_id(pid), got.promise.timeout_at, {"worker": worker}, {})
    return got.task.version


def test_submit_creates_one_promise_with_a_task() -> None:
    d, fake, _ = setup()
    sub = submission()
    assert d.submit(sub).status == "submitted"
    assert d.submit(sub).status == "submitted"  # idempotent
    assert list(fake.promises) == [sub.promise_id]
    assert fake.tasks[sub.promise_id].state == "pending"
    assert [m.task_id for m in fake.queues[GROUP]] == [sub.promise_id]
    assert d.in_flight() == 1


def test_claim_then_exit_zero_is_running_then_success() -> None:
    d, fake, _ = setup()
    sub = submission()
    d.submit(sub)
    assert d.poll(force=True) == []
    version = claim(fake, sub.promise_id)
    [running] = d.poll(force=True)
    assert (running.key, running.state, running.info) == ("k1", "running", "w1")
    assert d.poll(force=True) == []  # reported once
    fake.task_fulfill(sub.promise_id, version, "resolved", {"outcome": "exited", "returncode": 0})
    [done] = d.poll(force=True)
    assert (done.key, done.state) == ("k1", "success")
    assert d.in_flight() == 0


def test_nonzero_exit_and_worker_rejections_are_failures() -> None:
    d, fake, _ = setup()
    a, b = submission("a"), submission("b")
    d.submit(a)
    d.submit(b)
    va = claim(fake, a.promise_id)
    fake.task_fulfill(a.promise_id, va, "resolved", {"outcome": "exited", "returncode": 1})
    vb = claim(fake, b.promise_id, "w2")
    fake.task_fulfill(b.promise_id, vb, "rejected", {"outcome": "lease_lost", "reason": "died"})
    outcomes = {o.key: o for o in d.poll(force=True) if o.state != "running"}
    assert outcomes["a"].state == "failed" and "code 1" in outcomes["a"].info
    assert outcomes["b"].state == "failed" and outcomes["b"].info == "lease_lost: died"


def test_claim_timeout_cancels_and_fails() -> None:
    d, fake, clock = setup()
    sub = submission()
    d.submit(sub)
    clock.advance(59)
    assert d.poll(force=True) == []
    clock.advance(2)
    [out] = d.poll(force=True)
    assert out.state == "failed" and "no worker claimed" in out.info
    assert fake.promises[sub.promise_id].state == "rejected_canceled"
    assert fake.value_of(sub.promise_id)["outcome"] == "claim_timeout"
    assert fake.tasks[sub.promise_id].state == "fulfilled"
    # A late worker cannot start it any more.
    msg = fake.take(GROUP)[0]
    assert fake.task_acquire(msg.task_id, msg.version, "late", 60_000) is None


def test_claimed_tasks_never_hit_the_claim_timeout() -> None:
    d, fake, clock = setup()
    sub = submission()
    d.submit(sub)
    claim(fake, sub.promise_id)
    d.poll(force=True)
    clock.advance(3600)
    assert d.poll(force=True) == []
    assert fake.promises[sub.promise_id].state == "pending"


def test_promise_timeout_is_a_failure() -> None:
    d, fake, clock = setup()
    sub = replace(submission(), timeout_at_ms=int(clock() * 1000) + 10_000)
    d.submit(sub)
    claim(fake, sub.promise_id)
    d.poll(force=True)
    clock.advance(11)
    [out] = d.poll(force=True)
    assert out.state == "failed" and "timed out" in out.info


def test_unavailable_submit_retries_then_fails() -> None:
    d, fake, _ = setup()
    fake.unavailable.add("promise.create")
    assert d.submit(submission()).status == "retry"
    assert d.submit(submission()).status == "retry"
    result = d.submit(submission())
    assert result.status == "failed" and "3 attempts" in result.reason
    fake.unavailable.clear()
    assert d.submit(submission()).status == "submitted"


def test_poll_is_rate_limited_and_survives_outages() -> None:
    d, fake, clock = setup()
    d.submit(submission())
    d.poll()
    reads = fake.count("promise.get")
    d.poll()
    assert fake.count("promise.get") == reads  # within sync_interval
    clock.advance(5)
    fake.unavailable.add("promise.get")
    assert d.poll() == []
    assert d.in_flight() == 1


def test_batches_are_round_robin() -> None:
    d, fake, _ = setup(replace(SETTINGS, sync_batch_size=2))
    for key in ("a", "b", "c"):
        d.submit(submission(key))
    d.poll(force=True)
    first = [arg for name, arg in fake.calls if name == "promise.get"]
    fake.calls.clear()
    d.poll(force=True)
    second = [arg for name, arg in fake.calls if name == "promise.get"]
    seen = {p for p in first + second if not p.endswith(".started")}
    assert len(seen) == 3


def test_revoke_cancels_without_an_outcome() -> None:
    d, fake, _ = setup()
    sub = submission()
    d.submit(sub)
    assert d.revoke("k1")
    assert d.poll(force=True) == []
    assert fake.promises[sub.promise_id].state == "rejected_canceled"
    assert fake.tasks[sub.promise_id].state in ("halted", "fulfilled")
    assert not d.revoke("k1")


def test_adopt_by_id() -> None:
    d, fake, _ = setup()
    sub = submission()
    d.submit(sub)
    claim(fake, sub.promise_id)

    d2 = Dispatcher(client=fake, settings=SETTINGS, clock=d.clock)
    assert d2.adopt("k1", sub.promise_id, "default")
    assert not d2.adopt("k2", "af.default.missing:x.-1.1", "default")
    tracked = d2.tracked("k1")
    assert tracked is not None and tracked.adopted
    [running] = d2.poll(force=True)
    assert running.state == "running"


def test_adopting_a_finished_try_reports_it() -> None:
    d, fake, _ = setup()
    sub = submission()
    d.submit(sub)
    version = claim(fake, sub.promise_id)
    fake.task_fulfill(sub.promise_id, version, "resolved", {"outcome": "exited", "returncode": 0})
    d2 = Dispatcher(client=fake, settings=SETTINGS, clock=d.clock)
    assert d2.adopt("k1", sub.promise_id, "default")
    [out] = d2.poll()
    assert out.state == "success"


def test_resubmitting_a_settled_try_reports_it() -> None:
    d, fake, _ = setup()
    sub = submission()
    d.submit(sub)
    d.revoke("k1")
    d2 = Dispatcher(client=fake, settings=SETTINGS, clock=d.clock)
    assert d2.submit(sub).status == "submitted"
    [out] = d2.poll()
    assert out.state == "failed" and "revoked" in out.info


def test_queue_limits() -> None:
    d, _, _ = setup(replace(SETTINGS, queue_limits={"gpu": 1, "*": 2}))
    assert d.has_capacity("gpu")
    d.submit(submission("g1", queue="gpu"))
    assert not d.has_capacity("gpu")
    assert d.has_capacity("default")
    d.submit(submission("d1"))
    d.submit(submission("d2"))
    assert not d.has_capacity("default")


def test_cancel_all() -> None:
    d, fake, _ = setup()
    a, b = submission("a"), submission("b")
    d.submit(a)
    d.submit(b)
    d.cancel_all("bye")
    assert d.in_flight() == 0
    assert fake.value_of(a.promise_id)["outcome"] == "scheduler_terminated"
    assert fake.promises[b.promise_id].state == "rejected_canceled"
