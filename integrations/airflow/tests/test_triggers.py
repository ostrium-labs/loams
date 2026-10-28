# SPDX-License-Identifier: Apache-2.0
from __future__ import annotations

import importlib
from unittest import mock

import pytest

from airflow.providers.loams._connect import LoamsApiError
from airflow.providers.loams.hooks.house import HouseResult, LoamsHouseHook
from airflow.providers.loams.hooks.stream import LoamsStreamHook
from airflow.providers.loams.triggers.house import LoamsHouseQueryTrigger
from airflow.providers.loams.triggers.stream import LoamsStreamMessageTrigger, LoamsStreamOffsetTrigger
from tests.test_stream import FakeConsumer, FakeMessage


def roundtrip(trigger):
    classpath, kwargs = trigger.serialize()
    module, _, name = classpath.rpartition(".")
    again = getattr(importlib.import_module(module), name)(**kwargs)
    assert again.serialize() == (classpath, kwargs)
    return again


async def events(trigger) -> list[dict]:
    return [e.payload async for e in trigger.run()]


def test_serialize_round_trips() -> None:
    roundtrip(LoamsHouseQueryTrigger(query_id="q", sql="SELECT 1", params={"a": 1}, max_rows=5))
    roundtrip(LoamsStreamOffsetTrigger(topic="t", mode="offset", target_offsets={0: 3}))
    roundtrip(LoamsStreamMessageTrigger(topics="t", group_id="g", match={"type": "x"}))


@pytest.mark.asyncio
async def test_house_trigger_runs_an_unseen_query() -> None:
    trigger = LoamsHouseQueryTrigger(query_id="q", sql="INSERT 1", read_only=False)
    result = HouseResult(query_id="q", columns=[], rows=[[1]], summary={"written_rows": 1})
    with (
        mock.patch.object(LoamsHouseHook, "aget_query", mock.AsyncMock(return_value=None)),
        mock.patch.object(LoamsHouseHook, "arun", mock.AsyncMock(return_value=result)) as arun,
    ):
        out = await events(trigger)
    assert out == [
        {"status": "success", "query_id": "q", "summary": {"written_rows": 1}, "columns": [], "rows": [[1]]}
    ]
    assert arun.call_args.kwargs["query_id"] == "q"
    assert arun.call_args.kwargs["read_only"] is False


@pytest.mark.asyncio
async def test_house_trigger_follows_a_running_query_instead_of_rerunning() -> None:
    trigger = LoamsHouseQueryTrigger(query_id="q", sql="INSERT 1", poll_interval=0)
    records = [
        {"status": "QUERY_STATUS_RUNNING"},
        {"status": "QUERY_STATUS_RUNNING"},
        {"status": "QUERY_STATUS_FINISHED", "query_id": "q"},
    ]
    with (
        mock.patch.object(LoamsHouseHook, "aget_query", mock.AsyncMock(side_effect=records)),
        mock.patch.object(LoamsHouseHook, "arun", mock.AsyncMock()) as arun,
    ):
        out = await events(trigger)
    arun.assert_not_called()
    assert out[0]["status"] == "success"
    assert out[0]["record"]["status"] == "QUERY_STATUS_FINISHED"


@pytest.mark.asyncio
async def test_house_trigger_reports_failure() -> None:
    trigger = LoamsHouseQueryTrigger(query_id="q")
    record = {"status": "QUERY_STATUS_FAILED", "error_message": "Code: 60. Unknown table"}
    with mock.patch.object(LoamsHouseHook, "aget_query", mock.AsyncMock(return_value=record)):
        out = await events(trigger)
    assert out[0]["status"] == "error"
    assert "Unknown table" in out[0]["message"]


@pytest.mark.asyncio
async def test_house_trigger_gives_up_on_unknown_id() -> None:
    trigger = LoamsHouseQueryTrigger(query_id="q", poll_interval=0, not_found_timeout=0)
    with mock.patch.object(LoamsHouseHook, "aget_query", mock.AsyncMock(return_value=None)):
        out = await events(trigger)
    assert out[0]["status"] == "error"
    assert "no query q" in out[0]["message"]


@pytest.mark.asyncio
async def test_house_trigger_api_error_is_an_event() -> None:
    trigger = LoamsHouseQueryTrigger(query_id="q", sql="SELECT 1")
    with mock.patch.object(
        LoamsHouseHook, "aget_query", mock.AsyncMock(side_effect=LoamsApiError("x", code="unauthenticated"))
    ):
        out = await events(trigger)
    assert out[0]["status"] == "error" and out[0]["code"] == "unauthenticated"


@pytest.mark.asyncio
async def test_house_trigger_on_kill_cancels() -> None:
    with mock.patch.object(LoamsHouseHook, "acancel", mock.AsyncMock()) as cancel:
        await LoamsHouseQueryTrigger(query_id="q").on_kill()
    cancel.assert_awaited_once_with("q")


@pytest.mark.asyncio
async def test_offset_trigger_polls_until_done() -> None:
    trigger = LoamsStreamOffsetTrigger(topic="t", group_id="g", poll_interval=0)
    with mock.patch(
        "airflow.providers.loams.triggers.stream.check_offsets",
        side_effect=[(False, {"lag": {"0": 0}}), (True, {"lag": {"0": 4}})],
    ):
        out = await events(trigger)
    assert out == [{"status": "success", "topic": "t", "mode": "new_records", "lag": {"0": 4}}]


@pytest.mark.asyncio
async def test_offset_trigger_error_event() -> None:
    trigger = LoamsStreamOffsetTrigger(topic="t", group_id="g")
    with mock.patch(
        "airflow.providers.loams.triggers.stream.check_offsets", side_effect=ValueError("no topic")
    ):
        out = await events(trigger)
    assert out[0]["status"] == "error" and "no topic" in out[0]["message"]


@pytest.mark.asyncio
async def test_message_trigger_skips_non_matching_and_commits() -> None:
    consumer = FakeConsumer(
        [[FakeMessage(b'{"type": "other"}')], [], [FakeMessage(b'{"type": "commit", "n": 2}', offset=1)]]
    )
    trigger = LoamsStreamMessageTrigger(
        topics="orders", group_id="g", match={"type": "commit"}, poll_timeout=0
    )
    with mock.patch.object(LoamsStreamHook, "get_consumer", return_value=consumer):
        out = await events(trigger)
    assert len(out) == 1
    assert out[0]["value"] == {"type": "commit", "n": 2}
    assert out[0]["offset"] == 1
    assert consumer.committed_calls == 2  # the skipped record is committed too
    assert consumer.closed
