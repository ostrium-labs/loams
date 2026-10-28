# SPDX-License-Identifier: Apache-2.0
from __future__ import annotations

from unittest import mock

import pytest
from airflow.sdk.exceptions import TaskDeferred

from airflow.providers.loams.hooks.house import HouseResult, LoamsHouseHook
from airflow.providers.loams.operators.house import LoamsHouseQueryOperator, task_query_id
from airflow.providers.loams.sensors.house import LoamsHouseQuerySensor


def test_task_query_id(context) -> None:
    assert task_query_id(context) == "airflow:dag:task:manual__2026-10-27T00:00:00_00:00:-1:1"
    assert task_query_id(context, 2).endswith(":1:2")


def test_operator_runs_statements_in_order(loams_connection, context) -> None:
    op = LoamsHouseQueryOperator(
        task_id="h",
        sql=["CREATE TABLE x (a UInt8) ENGINE = Memory", "SELECT 1"],
        read_only=False,
        return_rows=False,
    )
    results = [HouseResult(query_id="a"), HouseResult(query_id="b", rows=[[1]], summary={"result_rows": 1})]
    with mock.patch.object(LoamsHouseHook, "run", side_effect=results) as run:
        out = op.execute(context)
    assert [c.args[0] for c in run.call_args_list] == ["CREATE TABLE x (a UInt8) ENGINE = Memory", "SELECT 1"]
    ids = [c.kwargs["query_id"] for c in run.call_args_list]
    assert ids[0] != ids[1]
    assert "rows" not in out and out["query_id"] == "b"


def test_operator_defers_the_last_statement(loams_connection, context) -> None:
    op = LoamsHouseQueryOperator(
        task_id="h", sql=["SET x", "INSERT INTO t SELECT 1"], deferrable=True, read_only=False
    )
    with (
        mock.patch.object(LoamsHouseHook, "run", return_value=HouseResult()) as run,
        pytest.raises(TaskDeferred) as d,
    ):
        op.execute(context)
    assert d.value.method_name == "execute_complete"
    assert run.call_count == 1
    trigger = d.value.trigger
    assert trigger.sql == "INSERT INTO t SELECT 1"
    assert trigger.query_id == task_query_id(context, 1)
    assert trigger.read_only is False
    # Resume with the trigger's event, as the triggerer would.
    resume = getattr(op, d.value.method_name)
    out = resume(context, {"status": "success", "query_id": trigger.query_id, "rows": [], "summary": {}})
    assert out["query_id"] == trigger.query_id
    with pytest.raises(RuntimeError, match="boom"):
        resume(context, {"status": "error", "query_id": trigger.query_id, "message": "boom"})


def test_execute_complete(context) -> None:
    op = LoamsHouseQueryOperator(task_id="h", sql="SELECT 1")
    out = op.execute_complete(context, {"status": "success", "query_id": "q", "rows": [[1]], "summary": {}})
    assert out["rows"] == [[1]]
    resumed = op.execute_complete(context, {"status": "success", "query_id": "q", "record": {"status": "x"}})
    assert resumed == {"query_id": "q", "record": {"status": "x"}}
    with pytest.raises(RuntimeError, match="failed"):
        op.execute_complete(context, {"status": "error", "query_id": "q", "message": "boom"})


def test_sensor_poke(loams_connection, context) -> None:
    sensor = LoamsHouseQuerySensor(task_id="s", query_id="q")
    with mock.patch.object(
        LoamsHouseHook,
        "get_query",
        side_effect=[
            None,
            {"status": "QUERY_STATUS_RUNNING"},
            {"status": "QUERY_STATUS_FINISHED"},
            {"status": "QUERY_STATUS_CANCELLED", "error_message": ""},
        ],
    ):
        assert sensor.poke(context) is False
        assert sensor.poke(context) is False
        assert sensor.poke(context) is True
        with pytest.raises(RuntimeError, match="CANCELLED"):
            sensor.poke(context)


def test_sensor_defers(loams_connection, context) -> None:
    sensor = LoamsHouseQuerySensor(task_id="s", query_id="q", deferrable=True, poke_interval=7)
    with mock.patch.object(LoamsHouseHook, "get_query", return_value=None), pytest.raises(TaskDeferred) as d:
        sensor.execute(context)
    assert d.value.method_name == "execute_complete"
    assert d.value.trigger.sql is None
    assert d.value.trigger.poll_interval == 7.0
    assert d.value.trigger.not_found_timeout is None
    resume = getattr(sensor, d.value.method_name)
    record = {"status": "QUERY_STATUS_FINISHED", "query_id": "q"}
    assert resume(context, {"status": "success", "query_id": "q", "record": record}) == record
    with pytest.raises(RuntimeError, match="failed"):
        resume(context, {"status": "error", "query_id": "q", "message": "query failed"})
