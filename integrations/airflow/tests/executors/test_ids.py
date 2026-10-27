# SPDX-License-Identifier: Apache-2.0
from __future__ import annotations

import pytest

from airflow.providers.loams.executors.resonate import ids

RUN = "manual__2026-10-27T10:00:00+00:00"


def ref(**kw: object) -> ids.TaskRef:
    base: dict[str, object] = {
        "dag_id": "etl.daily",
        "run_id": RUN,
        "task_id": "group.load",
        "try_number": 1,
        "map_index": -1,
    }
    base.update(kw)
    return ids.TaskRef(**base)  # type: ignore[arg-type]


def test_task_promise_id_is_deterministic_and_readable() -> None:
    pid = ids.task_promise_id("default", ref())
    assert pid == ids.task_promise_id("default", ref())
    assert pid == ("af.default.etl%2Edaily.manual__2026-10-27T10%3A00%3A00%2B00%3A00:group%2Eload.-1.1")


def test_origin_is_the_dag_run_and_holds_no_colon() -> None:
    pid = ids.task_promise_id("default", ref())
    origin = ids.origin_of(pid)
    assert ":" not in origin
    assert origin == ids.origin_of(ids.task_promise_id("default", ref(task_id="other")))
    assert origin != ids.origin_of(ids.task_promise_id("default", ref(run_id="r2")))


@pytest.mark.parametrize(
    "change",
    [
        {"try_number": 2},
        {"map_index": 0},
        {"task_id": "group.load2"},
        {"dag_id": "etl.weekly"},
        {"run_id": "scheduled__2026-10-27"},
    ],
)
def test_every_key_field_changes_the_id(change: dict[str, object]) -> None:
    assert ids.task_promise_id("default", ref()) != ids.task_promise_id("default", ref(**change))


def test_escaping_is_injective_across_separators() -> None:
    # "a.b" + "c" must not collide with "a" + "b.c".
    one = ids.task_promise_id("ns", ref(dag_id="a.b", run_id="c"))
    two = ids.task_promise_id("ns", ref(dag_id="a", run_id="b.c"))
    assert one != two


def test_escape_round_trips() -> None:
    for text in ["plain", "a.b:c", "100%", "ünïcode/✓", "--__--", ""]:
        assert ids.unescape(ids.escape(text)) == text
        assert all(ch.isalnum() or ch in "_-%" for ch in ids.escape(text))


def test_long_ids_use_the_hashed_form() -> None:
    long = ref(dag_id="d" * 200, task_id="t" * 200)
    pid = ids.task_promise_id("default", long)
    assert len(pid.encode()) <= ids.MAX_ID_BYTES
    assert pid.startswith("af.default.h")
    assert pid.endswith(".-1.1")
    assert pid == ids.task_promise_id("default", long)
    assert pid != ids.task_promise_id("default", ref(dag_id="d" * 200, task_id="t" * 199))


def test_started_marker_is_below_the_task_in_the_same_origin() -> None:
    pid = ids.task_promise_id("default", ref())
    marker = ids.started_marker_id(pid)
    assert marker == pid + ".started"
    assert ids.origin_of(marker) == ids.origin_of(pid)


def test_callback_ids() -> None:
    cid = ids.callback_promise_id("default", "0192f0d0-aaaa-7bbb-8ccc-123456789abc")
    assert cid == "af.default.cb:0192f0d0-aaaa-7bbb-8ccc-123456789abc"
    assert ":" not in ids.origin_of(cid)


def test_target_groups() -> None:
    assert ids.target_group("default", "default") == "af.default.default"
    assert ids.target_group("default", "gpu", "team-a") == "af.default.team-a.gpu"
    lossy = ids.target_group("default", "GPU_Pool")
    assert lossy.startswith("af.default.gpu-pool-")
    assert lossy != ids.target_group("default", "gpu-pool")
    assert ids.target_group("default", "GPU_Pool") == lossy
    for group in [lossy, ids.target_group("ns.with.dots", "q:1")]:
        assert ids.validate_group(group) == group
    assert ids.anycast_address("af.default.default") == "poll://any@af.default.default"


def test_validate_group_rejects_bad_labels() -> None:
    for bad in ["", "af..x", "AF.default", "af.default.under_score", "af.-x"]:
        with pytest.raises(ValueError):
            ids.validate_group(bad)
