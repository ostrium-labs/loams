# SPDX-License-Identifier: Apache-2.0
from __future__ import annotations

import json
import struct

import httpx
import pytest

from airflow.providers.loams._connect import (
    AsyncConnectClient,
    ConnectClient,
    EnvelopeReader,
    LoamsApiError,
    camel,
    envelope,
    field,
)


def frame(message: dict, flags: int = 0) -> bytes:
    payload = json.dumps(message).encode()
    return struct.pack(">BI", flags, len(payload)) + payload


def test_camel_and_field() -> None:
    assert camel("query_id") == "queryId"
    assert camel("x") == "x"
    assert field({"queryId": "q"}, "query_id") == "q"
    assert field({"query_id": "q"}, "query_id") == "q"
    assert field(None, "query_id", "d") == "d"


def test_envelope_round_trip_split_anywhere() -> None:
    data = envelope({"a": 1}) + frame({"b": 2}) + frame({}, flags=0x02)
    for cut in range(len(data) + 1):
        reader = EnvelopeReader(rpc="s/m")
        out = reader.feed(data[:cut]) + reader.feed(data[cut:])
        assert out == [{"a": 1}, {"b": 2}]
        reader.finish()


def test_end_stream_error_raises() -> None:
    reader = EnvelopeReader(rpc="s/m")
    reader.feed(
        frame(
            {
                "error": {
                    "code": "not_found",
                    "message": "no query",
                    "details": [
                        {"type": "loams.errors.v1.ErrorInfo", "debug": {"reason": "house_query_not_found"}}
                    ],
                }
            },
            flags=0x02,
        )
    )
    with pytest.raises(LoamsApiError) as err:
        reader.finish()
    assert err.value.code == "not_found"
    assert err.value.reason == "house_query_not_found"
    assert err.value.not_found


def test_stream_without_end_is_an_error() -> None:
    reader = EnvelopeReader(rpc="s/m")
    reader.feed(frame({"a": 1}))
    with pytest.raises(LoamsApiError, match="without an end-of-stream"):
        reader.finish()


def test_compressed_frame_is_refused() -> None:
    with pytest.raises(LoamsApiError, match="compressed"):
        EnvelopeReader(rpc="s/m").feed(frame({}, flags=0x01))


class _Client(ConnectClient):
    handler = None

    def _client(self) -> httpx.Client:
        return httpx.Client(transport=httpx.MockTransport(type(self).handler))


def test_unary_call_sends_bearer_and_json() -> None:
    seen = {}

    def handler(request: httpx.Request) -> httpx.Response:
        seen["url"] = str(request.url)
        seen["auth"] = request.headers["authorization"]
        seen["ct"] = request.headers["content-type"]
        seen["body"] = json.loads(request.content)
        return httpx.Response(200, json={"query": {"queryId": "q1"}})

    _Client.handler = staticmethod(handler)
    answer = _Client(base_url="http://h:1/", token="lk_x").call("pkg.S", "M", {"queryId": "q1"})
    assert seen == {
        "url": "http://h:1/pkg.S/M",
        "auth": "Bearer lk_x",
        "ct": "application/json",
        "body": {"queryId": "q1"},
    }
    assert answer == {"query": {"queryId": "q1"}}


def test_unary_error_maps_code_and_reason() -> None:
    def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(
            403,
            json={
                "code": "permission_denied",
                "message": "nope",
                "details": [{"debug": {"reason": "house_access_denied", "metadata": {"table": "t"}}}],
            },
        )

    _Client.handler = staticmethod(handler)
    with pytest.raises(LoamsApiError) as err:
        _Client(base_url="http://h").call("pkg.S", "M")
    assert err.value.code == "permission_denied"
    assert err.value.reason == "house_access_denied"
    assert err.value.metadata == {"table": "t"}
    assert err.value.http_status == 403


def test_unary_error_without_body_uses_http_status() -> None:
    _Client.handler = staticmethod(lambda request: httpx.Response(503, text="down"))
    with pytest.raises(LoamsApiError) as err:
        _Client(base_url="http://h").call("pkg.S", "M")
    assert err.value.code == "unavailable"


def test_server_stream() -> None:
    body = frame({"header": {}}) + frame({"summary": {}}) + frame({}, flags=0x02)

    def handler(request: httpx.Request) -> httpx.Response:
        assert request.headers["content-type"] == "application/connect+json"
        reader = EnvelopeReader(rpc="x")
        assert reader.feed(request.content) == [{"sql": "SELECT 1"}]
        return httpx.Response(200, content=body)

    _Client.handler = staticmethod(handler)
    frames = list(_Client(base_url="http://h").stream("pkg.S", "M", {"sql": "SELECT 1"}))
    assert frames == [{"header": {}}, {"summary": {}}]


class _AsyncClient(AsyncConnectClient):
    handler = None

    def _client(self) -> httpx.AsyncClient:
        return httpx.AsyncClient(transport=httpx.MockTransport(type(self).handler))


@pytest.mark.asyncio
async def test_async_call_and_stream() -> None:
    def handler(request: httpx.Request) -> httpx.Response:
        if request.url.path.endswith("/Unary"):
            return httpx.Response(200, json={"ok": True})
        return httpx.Response(200, content=frame({"n": 1}) + frame({}, flags=0x02))

    _AsyncClient.handler = staticmethod(handler)
    client = _AsyncClient(base_url="http://h")
    assert await client.call("pkg.S", "Unary") == {"ok": True}
    assert [m async for m in client.stream("pkg.S", "Stream")] == [{"n": 1}]
