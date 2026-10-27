# SPDX-License-Identifier: Apache-2.0
"""`LoamsGraphHook`: GQL (and openCypher) on Loams Graph, through the Loams SDK.

The Python SDK's `graph` module wraps `loams.graph.v1.GraphService` and
`GraphAdminService` (marked unstable, as the SDK says). The hook builds the
`ExecuteRequest`, binds parameters as `$name` values (never interpolated into
the statement), and converts the answer's values to plain Python so a result
can go to XCom.
"""

from __future__ import annotations

import datetime as dt
import decimal
from collections.abc import Mapping, Sequence
from dataclasses import asdict, dataclass, field
from typing import Any

from airflow.providers.loams.hooks.base import LoamsHook

__all__ = ["GraphResult", "LoamsGraphHook", "from_value", "to_value"]

_LANGUAGES = {
    "gql": "QUERY_LANGUAGE_GQL",
    "cypher": "QUERY_LANGUAGE_CYPHER",
    "opencypher": "QUERY_LANGUAGE_CYPHER",
}

_INT64_MAX = 2**63 - 1
_INT64_MIN = -(2**63)


def _pb() -> Any:
    from loams.graph.v1 import graph_pb2

    return graph_pb2


def to_value(value: Any) -> Any:
    """A Python value as a `loams.graph.v1.Value`."""
    pb = _pb()
    if value is None:
        return pb.Value(null=0)
    if isinstance(value, bool):
        return pb.Value(boolean=value)
    if isinstance(value, int):
        if _INT64_MIN <= value <= _INT64_MAX:
            return pb.Value(int64=value)
        if 0 <= value < 2**64:
            return pb.Value(uint64=value)
        raise ValueError(f"integer {value} does not fit in 64 bits")
    if isinstance(value, float):
        return pb.Value(float64=value)
    if isinstance(value, str):
        return pb.Value(string=value)
    if isinstance(value, (bytes, bytearray)):
        return pb.Value(bytes=bytes(value))
    if isinstance(value, decimal.Decimal):
        return pb.Value(decimal=pb.Decimal(value=str(value)))
    if isinstance(value, dt.datetime):
        local = pb.LocalDateTime(
            date=pb.Date(year=value.year, month=value.month, day=value.day),
            time=pb.LocalTime(
                hour=value.hour, minute=value.minute, second=value.second, nanosecond=value.microsecond * 1000
            ),
        )
        offset = value.utcoffset()
        if offset is None:
            return pb.Value(local_datetime=local)
        return pb.Value(
            zoned_datetime=pb.ZonedDateTime(local=local, offset_seconds=int(offset.total_seconds()))
        )
    if isinstance(value, dt.date):
        return pb.Value(date=pb.Date(year=value.year, month=value.month, day=value.day))
    if isinstance(value, dt.time):
        local_time = pb.LocalTime(
            hour=value.hour, minute=value.minute, second=value.second, nanosecond=value.microsecond * 1000
        )
        offset = value.utcoffset()
        if offset is None:
            return pb.Value(local_time=local_time)
        return pb.Value(zoned_time=pb.ZonedTime(time=local_time, offset_seconds=int(offset.total_seconds())))
    if isinstance(value, Mapping):
        return pb.Value(map=pb.MapValue(entries={str(k): to_value(v) for k, v in value.items()}))
    if isinstance(value, (list, tuple)):
        return pb.Value(list=pb.ListValue(values=[to_value(v) for v in value]))
    raise TypeError(f"cannot bind a {type(value).__name__} as a graph parameter")


def _local_time(t: Any) -> str:
    text = f"{t.hour:02d}:{t.minute:02d}:{t.second:02d}"
    if t.nanosecond:
        text += f".{t.nanosecond:09d}".rstrip("0")
    return text


def _date(d: Any) -> str:
    return f"{d.year:04d}-{d.month:02d}-{d.day:02d}"


def _offset(seconds: int) -> str:
    sign = "+" if seconds >= 0 else "-"
    seconds = abs(seconds)
    return f"{sign}{seconds // 3600:02d}:{(seconds % 3600) // 60:02d}"


def _properties(props: Any) -> dict[str, Any]:
    return {k: from_value(v) for k, v in props.items()}


def _node(n: Any) -> dict[str, Any]:
    return {"id": int(n.id), "labels": list(n.labels), "properties": _properties(n.properties)}


def _relationship(r: Any) -> dict[str, Any]:
    return {
        "id": int(r.id),
        "type": r.type,
        "src": int(r.src),
        "dst": int(r.dst),
        "properties": _properties(r.properties),
    }


def from_value(value: Any) -> Any:
    """A `loams.graph.v1.Value` as JSON-safe Python.

    Temporal values become ISO 8601 text, nodes and relationships dicts, a
    path `{"nodes": [...], "relationships": [...]}`, a decimal its text, and
    bytes stay bytes (XCom's serializer handles them).
    """
    kind = value.WhichOneof("kind")
    if kind is None or kind == "null":
        return None
    if kind in ("boolean", "int64", "uint64", "float64", "string", "bytes"):
        return getattr(value, kind)
    if kind == "decimal":
        return value.decimal.value
    if kind == "date":
        return _date(value.date)
    if kind == "local_time":
        return _local_time(value.local_time)
    if kind == "zoned_time":
        return _local_time(value.zoned_time.time) + _offset(value.zoned_time.offset_seconds)
    if kind == "local_datetime":
        return f"{_date(value.local_datetime.date)}T{_local_time(value.local_datetime.time)}"
    if kind == "zoned_datetime":
        local = value.zoned_datetime.local
        return f"{_date(local.date)}T{_local_time(local.time)}{_offset(value.zoned_datetime.offset_seconds)}"
    if kind == "duration":
        d = value.duration
        return {"months": int(d.months), "days": int(d.days), "nanos": int(d.nanos)}
    if kind == "list":
        return [from_value(v) for v in value.list.values]
    if kind == "map":
        return {k: from_value(v) for k, v in value.map.entries.items()}
    if kind == "node":
        return _node(value.node)
    if kind == "relationship":
        return _relationship(value.relationship)
    if kind == "path":
        return {
            "nodes": [_node(n) for n in value.path.nodes],
            "relationships": [_relationship(r) for r in value.path.relationships],
        }
    if kind == "vector":
        return list(value.vector.values)
    if kind == "counter":
        c = value.counter
        return sum(c.positive.values()) - sum(c.negative.values())
    raise TypeError(f"unknown graph value kind {kind}")


@dataclass
class GraphResult:
    columns: list[str] = field(default_factory=list)
    column_types: list[str] = field(default_factory=list)
    rows: list[dict[str, Any]] = field(default_factory=list)
    truncated: bool = False
    consistency_token: str = ""
    commit_epoch: int = 0

    def to_dict(self) -> dict[str, Any]:
        return asdict(self)


class LoamsGraphHook(LoamsHook):
    """Statements on a Loams graph.

    :param loams_conn_id: the `loams` connection.
    :param graph: the default graph; the connection's extra `graph` by default.
    """

    hook_name = "Loams Graph"

    def __init__(
        self, loams_conn_id: str = LoamsHook.default_conn_name, *, graph: str | None = None, **kwargs: Any
    ) -> None:
        super().__init__(loams_conn_id=loams_conn_id, **kwargs)
        self._graph = graph

    def graph_for(self, graph: str | None) -> str:
        name = graph or self._graph or self.settings.graph
        if not name:
            raise ValueError("no graph: pass graph= or set the connection's extra `graph`")
        return name

    def build_request(
        self,
        statement: str,
        *,
        graph: str | None = None,
        parameters: Mapping[str, Any] | None = None,
        language: str = "gql",
        read_only: bool = False,
        max_rows: int = 0,
        timeout_ms: int = 0,
        idempotency_key: str | None = None,
    ) -> Any:
        pb = _pb()
        lang = _LANGUAGES.get(language.lower())
        if lang is None:
            raise ValueError(f"unknown graph language {language!r}: use one of {sorted(_LANGUAGES)}")
        return pb.ExecuteRequest(
            namespace=self.settings.require_namespace(),
            graph=self.graph_for(graph),
            statement=statement,
            language=pb.QueryLanguage.Value(lang),
            parameters={str(k): to_value(v) for k, v in (parameters or {}).items()},
            read_only=read_only,
            timeout_ms=int(timeout_ms),
            max_rows=int(max_rows),
            idempotency_key=idempotency_key or "",
        )

    def query(self, statement: str, **kwargs: Any) -> GraphResult:
        """Runs one statement (see `build_request`) and answers its rows as dicts."""
        request = self.build_request(statement, **kwargs)
        with self.get_conn() as client:
            response = client.graph.query(request)
        return self.to_result(response)

    @staticmethod
    def to_result(response: Any) -> GraphResult:
        rowset = response.rows
        columns = list(rowset.columns)
        rows = [dict(zip(columns, (from_value(v) for v in row.values), strict=False)) for row in rowset.rows]
        return GraphResult(
            columns=columns,
            column_types=list(rowset.column_types),
            rows=rows,
            truncated=bool(response.truncated),
            consistency_token=response.consistency_token,
            commit_epoch=int(response.commit_epoch),
        )

    def explain(
        self,
        statement: str,
        *,
        graph: str | None = None,
        parameters: Mapping[str, Any] | None = None,
        language: str = "gql",
        profile: bool = False,
        timeout_ms: int = 0,
    ) -> str:
        """The engine's plan for a statement, as text (`profile` runs it)."""
        pb = _pb()
        lang = _LANGUAGES.get(language.lower())
        if lang is None:
            raise ValueError(f"unknown graph language {language!r}: use one of {sorted(_LANGUAGES)}")
        request = pb.ExplainRequest(
            namespace=self.settings.require_namespace(),
            graph=self.graph_for(graph),
            statement=statement,
            language=pb.QueryLanguage.Value(lang),
            parameters={str(k): to_value(v) for k, v in (parameters or {}).items()},
            profile=profile,
            timeout_ms=int(timeout_ms),
        )
        with self.get_conn() as client:
            plan = client.graph.explain(request)
        return str(plan.text)

    def list_graphs(self) -> list[str]:
        pb = _pb()
        names: list[str] = []
        token = ""
        with self.get_conn() as client:
            while True:
                response = client.graph.list(
                    pb.ListGraphsRequest(namespace=self.settings.require_namespace(), page_token=token)
                )
                names.extend(g.name for g in response.graphs)
                token = response.next_page_token
                if not token:
                    return names

    def test_connection(self) -> tuple[bool, str]:
        try:
            graphs = self.list_graphs()
            return True, f"Graph answered with {len(graphs)} graphs"
        except Exception as exc:
            return False, str(exc)


def rows_as_list(result: GraphResult, columns: Sequence[str] | None = None) -> list[list[Any]]:
    """Rows as lists in column order (for a downstream that wants tuples)."""
    names = list(columns or result.columns)
    return [[row.get(c) for c in names] for row in result.rows]
