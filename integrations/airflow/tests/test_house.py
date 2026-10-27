# SPDX-License-Identifier: Apache-2.0
from __future__ import annotations

import base64
from unittest import mock

import pytest

from airflow.providers.loams._connect import LoamsApiError
from airflow.providers.loams.hooks.house import (
    HouseQueryStatus,
    HouseResultBuilder,
    LoamsHouseHook,
    parse_json_rows,
    query_record,
    split_statements,
)


def rows(*lines: str) -> str:
    return base64.b64encode("\n".join(lines).encode() + b"\n").decode()


FRAMES = [
    {
        "header": {
            "columns": [
                {"name": "kind", "clickhouseType": "String"},
                {"name": "n", "clickhouseType": "UInt64"},
            ],
            "queryId": "q1",
            "format": "RESULT_FORMAT_JSON_ROWS",
        }
    },
    {"progress": {"readRows": "10"}},
    {"batch": {"rowsJson": rows('["a","1"]', '["b","2"]'), "rows": "2"}},
    {"batch": {"rowsJson": rows('["c","3"]'), "rows": "1"}},
    {
        "summary": {
            "resultRows": "3",
            "readRows": "10",
            "elapsedMs": "4",
            "truncated": False,
            "queryId": "q1",
            "snapshotIds": {"db.t": "7"},
        }
    },
]


def test_builder_folds_frames() -> None:
    builder = HouseResultBuilder()
    for f in FRAMES:
        builder.feed(f)
    result = builder.result
    assert builder.done
    assert result.query_id == "q1"
    assert result.column_names == ["kind", "n"]
    assert result.rows == [["a", "1"], ["b", "2"], ["c", "3"]]
    assert result.records()[0] == {"kind": "a", "n": "1"}
    assert result.summary["result_rows"] == 3
    assert result.summary["snapshot_ids"] == {"db.t": 7}


def test_builder_refuses_other_formats() -> None:
    with pytest.raises(LoamsApiError, match="format"):
        HouseResultBuilder().feed({"header": {"format": "RESULT_FORMAT_ARROW_IPC"}})


def test_parse_json_rows_skips_blank_lines() -> None:
    assert parse_json_rows(b"[1]\n\n[2]\n") == [[1], [2]]


def test_build_request(loams_connection) -> None:
    hook = LoamsHouseHook(database="analytics")
    request = hook.build_request(
        "SELECT {d:Date};  ",
        params={"d": "2026-10-27", "flag": True},
        settings={"max_threads": 2},
        read_only=False,
        max_rows=10,
        max_bytes=1024,
        timeout_ms=1000,
        query_id="q",
    )
    assert request == {
        "sql": "SELECT {d:Date}",
        "database": "analytics",
        "format": "RESULT_FORMAT_JSON_ROWS",
        "readOnly": False,
        "params": {"d": "2026-10-27", "flag": "1"},
        "settings": {"max_threads": "2"},
        "maxRows": 10,
        "maxBytes": "1024",
        "timeoutMs": 1000,
        "queryId": "q",
    }
    with pytest.raises(ValueError, match="empty"):
        hook.build_request(" ; ")


def test_run_streams_execute_query(loams_connection) -> None:
    with mock.patch(
        "airflow.providers.loams._connect.ConnectClient.stream", return_value=iter(FRAMES)
    ) as stream:
        result = LoamsHouseHook().run("SELECT 1")
    service, method, request = stream.call_args.args
    assert (service, method) == ("loams.house.v1.HouseService", "ExecuteQuery")
    assert request["readOnly"] is True
    assert len(result.rows) == 3


def test_run_without_summary_fails(loams_connection) -> None:
    with (
        mock.patch("airflow.providers.loams._connect.ConnectClient.stream", return_value=iter(FRAMES[:2])),
        pytest.raises(LoamsApiError, match="without a summary"),
    ):
        LoamsHouseHook().run("SELECT 1")


def test_house_endpoint_override(loams_connection) -> None:
    loams_connection({"house_endpoint": "https://house:8443"})
    assert LoamsHouseHook()._client().base_url == "https://house:8443"


def test_get_query(loams_connection) -> None:
    answer = {
        "query": {"queryId": "q1", "status": "QUERY_STATUS_FAILED", "errorCode": 62, "errorMessage": "bad"}
    }
    with mock.patch("airflow.providers.loams._connect.ConnectClient.call", return_value=answer):
        record = LoamsHouseHook().get_query("q1")
    assert record["status"] == HouseQueryStatus.FAILED
    assert record["error_code"] == 62
    assert record["error_message"] == "bad"


def test_get_query_not_found_is_none(loams_connection) -> None:
    with mock.patch(
        "airflow.providers.loams._connect.ConnectClient.call",
        side_effect=LoamsApiError("no", code="not_found", reason="house_query_not_found"),
    ):
        assert LoamsHouseHook().get_query("q1") is None


def test_get_query_other_errors_raise(loams_connection) -> None:
    with (
        mock.patch(
            "airflow.providers.loams._connect.ConnectClient.call",
            side_effect=LoamsApiError("no", code="unauthenticated"),
        ),
        pytest.raises(LoamsApiError),
    ):
        LoamsHouseHook().get_query("q1")


def test_status_normalize() -> None:
    assert HouseQueryStatus.normalize(2) == HouseQueryStatus.FINISHED
    assert HouseQueryStatus.normalize("3") == HouseQueryStatus.FAILED
    assert HouseQueryStatus.normalize(None) == "QUERY_STATUS_UNSPECIFIED"
    assert query_record({})["status"] == "QUERY_STATUS_UNSPECIFIED"


def test_list_tables_pages(loams_connection) -> None:
    pages = [
        {"tables": [{"name": "a"}], "nextPageToken": "t2"},
        {"tables": [{"name": "b"}]},
    ]
    with mock.patch("airflow.providers.loams._connect.ConnectClient.call", side_effect=pages) as call:
        tables = LoamsHouseHook().list_tables("db")
    assert [t["name"] for t in tables] == ["a", "b"]
    assert call.call_args.args[2]["pageToken"] == "t2"


def test_split_statements() -> None:
    assert split_statements("SELECT 1; SELECT 2") == ["SELECT 1; SELECT 2"]
    assert split_statements(["a", " ", "b"]) == ["a", "b"]


@pytest.mark.asyncio
async def test_async_run(loams_connection) -> None:
    async def frames(*args, **kwargs):
        for f in FRAMES:
            yield f

    with mock.patch("airflow.providers.loams._connect.AsyncConnectClient.stream", side_effect=frames):
        result = await LoamsHouseHook().arun("SELECT 1")
    assert result.query_id == "q1"
