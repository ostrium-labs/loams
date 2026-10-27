# SPDX-License-Identifier: Apache-2.0
from __future__ import annotations

import datetime as dt
import decimal
from unittest import mock

import pytest
from loams.graph.v1 import graph_pb2 as pb

from airflow.providers.loams.hooks.graph import LoamsGraphHook, from_value, rows_as_list, to_value
from airflow.providers.loams.operators.graph import LoamsGraphQueryOperator


@pytest.mark.parametrize(
    ("value", "expected"),
    [
        (None, None),
        (True, True),
        (42, 42),
        (2**63, 2**63),
        (1.5, 1.5),
        ("x", "x"),
        (b"\x00", b"\x00"),
        ([1, "a"], [1, "a"]),
        ({"k": [1]}, {"k": [1]}),
        (decimal.Decimal("1.10"), "1.10"),
        (dt.date(2026, 10, 27), "2026-10-27"),
        (dt.datetime(2026, 10, 27, 1, 2, 3), "2026-10-27T01:02:03"),
        (
            dt.datetime(2026, 10, 27, 1, 2, 3, 500000, tzinfo=dt.timezone(dt.timedelta(hours=5, minutes=30))),
            "2026-10-27T01:02:03.5+05:30",
        ),
        (dt.time(4, 5, 6), "04:05:06"),
    ],
)
def test_value_round_trip(value, expected) -> None:
    assert from_value(to_value(value)) == expected


def test_to_value_refuses() -> None:
    with pytest.raises(ValueError):
        to_value(2**64)
    with pytest.raises(TypeError):
        to_value(object())


def test_from_value_graph_kinds() -> None:
    node = pb.Node(id=1, labels=["Person"], properties={"name": pb.Value(string="Ada")})
    rel = pb.Relationship(id=2, type="KNOWS", src=1, dst=3, properties={})
    assert from_value(pb.Value(node=node)) == {"id": 1, "labels": ["Person"], "properties": {"name": "Ada"}}
    assert from_value(pb.Value(relationship=rel))["type"] == "KNOWS"
    path = from_value(pb.Value(path=pb.Path(nodes=[node], relationships=[rel])))
    assert len(path["nodes"]) == 1 and len(path["relationships"]) == 1
    assert from_value(pb.Value(duration=pb.Duration(months=1, days=2, nanos=3))) == {
        "months": 1,
        "days": 2,
        "nanos": 3,
    }
    assert from_value(pb.Value(counter=pb.Counter(positive={"a": 5}, negative={"a": 2}))) == 3
    assert from_value(pb.Value(vector=pb.Vector(values=[0.5]))) == [0.5]


def test_build_request(loams_connection) -> None:
    request = LoamsGraphHook().build_request(
        "MATCH (n) WHERE n.x = $x RETURN n",
        parameters={"x": 1},
        language="cypher",
        read_only=True,
        max_rows=5,
    )
    assert request.namespace == "acme"
    assert request.graph == "social"
    assert request.language == pb.QUERY_LANGUAGE_CYPHER
    assert request.parameters["x"].int64 == 1
    assert request.read_only and request.max_rows == 5
    with pytest.raises(ValueError, match="language"):
        LoamsGraphHook().build_request("x", language="sparql")


def test_query_converts_rows(loams_connection) -> None:
    response = pb.ExecuteResponse(
        rows=pb.RowSet(
            columns=["name", "n"],
            column_types=["STRING", "INT64"],
            rows=[pb.Row(values=[pb.Value(string="Ada"), pb.Value(int64=2)])],
        ),
        truncated=True,
        consistency_token="tok",
    )
    client = mock.MagicMock()
    client.__enter__.return_value.graph.query.return_value = response
    hook = LoamsGraphHook()
    with mock.patch.object(hook, "get_conn", return_value=client):
        result = hook.query("MATCH ...")
    assert result.rows == [{"name": "Ada", "n": 2}]
    assert result.truncated and result.consistency_token == "tok"
    assert rows_as_list(result) == [["Ada", 2]]


def test_operator_sets_idempotency_key_for_writes(loams_connection, context) -> None:
    op = LoamsGraphQueryOperator(task_id="g", statement="INSERT (:A)")
    with mock.patch.object(LoamsGraphHook, "query") as query:
        query.return_value.rows = []
        query.return_value.truncated = False
        query.return_value.to_dict.return_value = {"rows": []}
        assert op.execute(context) == {"rows": []}
    assert query.call_args.kwargs["idempotency_key"].startswith("airflow:dag:task:")
    op = LoamsGraphQueryOperator(task_id="g2", statement="MATCH (n) RETURN n", read_only=True)
    with mock.patch.object(LoamsGraphHook, "query") as query:
        query.return_value.rows = []
        query.return_value.truncated = False
        op.execute(context)
    assert query.call_args.kwargs["idempotency_key"] is None
