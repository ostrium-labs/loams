# SPDX-License-Identifier: Apache-2.0
from __future__ import annotations

import json
from typing import Any

import httpx
import pytest

from airflow.providers.loams.executors.resonate.client import (
    PROTOCOL_VERSION,
    HttpResonateClient,
    ResonateError,
    SseFramer,
    Unavailable,
    decode_value,
    encode_value,
    parse_message,
)

PROMISE = {
    "id": "af.default.d.r:t.-1.1",
    "state": "pending",
    "param": encode_value({"v": 1}),
    "value": {},
    "tags": {"resonate:target": "poll://any@af.default.default"},
    "timeoutAt": 2_000,
    "createdAt": 1_000,
}


class Server:
    """Answers protocol envelopes with canned (status, data) per kind."""

    def __init__(self, answers: dict[str, tuple[int, Any]]) -> None:
        self.answers = answers
        self.requests: list[dict[str, Any]] = []
        self.headers: list[httpx.Headers] = []

    def __call__(self, request: httpx.Request) -> httpx.Response:
        self.headers.append(request.headers)
        if request.method == "GET":
            body = (
                'data: {"kind":"execute","head":{"serverUrl":"x"},'
                '"data":{"task":{"id":"t1","version":3}}}\n\n'
                'data: {"kind":"unblock","head":{},"data":{}}\n\n'
            )
            return httpx.Response(200, text=body, headers={"content-type": "text/event-stream"})
        envelope = json.loads(request.content)
        self.requests.append(envelope)
        status, data = self.answers[envelope["kind"]]
        head = {"corrId": envelope["head"]["corrId"], "status": status, "version": "2026-04-01"}
        return httpx.Response(200, json={"kind": envelope["kind"], "head": head, "data": data})


def make(answers: dict[str, tuple[int, Any]], token: str | None = None):
    server = Server(answers)
    client = HttpResonateClient("http://resonate:8001/", token=token, transport=httpx.MockTransport(server))
    return client, server


def test_values_round_trip() -> None:
    value = {"workload": {"ti": {"id": "x"}}, "n": [1, 2]}
    assert decode_value(encode_value(value)) == value
    assert decode_value({}) is None
    assert encode_value(None) == {}
    with pytest.raises(ValueError):
        decode_value({"data": "not base64 json!"})


def test_promise_create_sends_the_envelope() -> None:
    client, server = make({"promise.create": (200, {"promise": PROMISE})}, token="tok")
    rec = client.promise_create("af.default.d.r:t.-1.1", 2_000, {"v": 1}, {"a": "b"})
    assert rec.id == PROMISE["id"] and rec.pending
    assert rec.param_json() == {"v": 1}
    sent = server.requests[0]
    assert sent["kind"] == "promise.create"
    assert sent["head"]["version"] == PROTOCOL_VERSION
    assert sent["head"]["auth"] == "tok"
    assert sent["data"]["timeoutAt"] == 2_000
    assert decode_value(sent["data"]["param"]) == {"v": 1}
    assert server.headers[0]["authorization"] == "Bearer tok"


def test_not_found_and_conflict_are_normal_answers() -> None:
    client, _ = make(
        {
            "promise.get": (404, {}),
            "task.acquire": (409, {}),
            "task.fulfill": (409, {}),
            "task.halt": (409, {}),
            "task.get": (404, {}),
        }
    )
    assert client.promise_get("x") is None
    assert client.task_acquire("x", 0, "p", 60_000) is None
    assert client.task_fulfill("x", 1, "resolved", {"ok": True}) is False
    assert client.task_halt("x") is False
    assert client.task_get("x") is None


def test_task_fulfill_wraps_a_settle_action() -> None:
    client, server = make({"task.fulfill": (200, {"promise": {**PROMISE, "state": "resolved"}})})
    assert client.task_fulfill("t", 2, "resolved", {"outcome": "exited", "returncode": 0})
    data = server.requests[0]["data"]
    assert data["id"] == "t" and data["version"] == 2
    assert data["action"]["kind"] == "promise.settle"
    assert data["action"]["data"]["id"] == "t"
    assert decode_value(data["action"]["data"]["value"])["returncode"] == 0


def test_task_acquire_parses_task_and_promise() -> None:
    task = {"id": PROMISE["id"], "state": "acquired", "version": 1, "pid": "w", "ttl": 60000}
    client, _ = make({"task.acquire": (200, {"task": task, "promise": PROMISE, "preload": []})})
    got = client.task_acquire(PROMISE["id"], 0, "w", 60_000)
    assert got is not None
    assert got.task.version == 1 and got.promise.timeout_at == 2_000


def test_errors() -> None:
    client, _ = make({"promise.settle": (400, {"message": "bad state"})})
    with pytest.raises(ResonateError) as err:
        client.promise_settle("x", "resolved", None)
    assert err.value.status == 400 and "bad state" in str(err.value)

    def boom(request: httpx.Request) -> httpx.Response:
        raise httpx.ConnectError("refused", request=request)

    down = HttpResonateClient("http://resonate:8001", transport=httpx.MockTransport(boom))
    with pytest.raises(Unavailable):
        down.promise_get("x")

    def gateway(request: httpx.Request) -> httpx.Response:
        return httpx.Response(503, text="no upstream")

    proxied = HttpResonateClient("http://resonate:8001", transport=httpx.MockTransport(gateway))
    with pytest.raises(Unavailable):
        proxied.promise_get("x")


def test_corr_id_mismatch_is_an_error() -> None:
    def wrong(request: httpx.Request) -> httpx.Response:
        envelope = json.loads(request.content)
        head = {"corrId": "someone-else", "status": 200}
        return httpx.Response(200, json={"kind": envelope["kind"], "head": head, "data": {}})

    client = HttpResonateClient("http://r", transport=httpx.MockTransport(wrong))
    with pytest.raises(ResonateError):
        client.promise_get("x")


def test_poll_yields_execute_messages_only() -> None:
    client, server = make({})
    msgs = list(client.poll("af.default.default", "w1", lambda: False))
    assert [(m.task_id, m.version) for m in msgs] == [("t1", 3)]
    assert server.headers[0]["accept"] == "text/event-stream"


def test_sse_framer_handles_split_chunks() -> None:
    framer = SseFramer()
    assert framer.feed('data: {"a"') == []
    assert framer.feed(":1}\r\n\r\ndata: x\n") == ['{"a":1}']
    assert framer.feed("\n") == ["x"]


def test_parse_message() -> None:
    assert parse_message("not json") is None
    assert parse_message('{"kind":"unblock"}') is None
    assert parse_message('{"kind":"execute","data":{"task":{}}}') is None
    msg = parse_message('{"kind":"execute","data":{"task":{"id":"a","version":2}}}')
    assert msg is not None and msg.task_id == "a" and msg.version == 2
