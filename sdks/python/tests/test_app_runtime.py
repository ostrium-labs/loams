"""SDKP Part A for Python: R4's session, R11–R14, the collection handle and
`search.query_all`.

The canonical names, which `check-languages.mjs --suite app` looks for:

- `python_session_read_your_writes` (R4 as amended)
- `python_default_namespace` (R11)
- `python_timeout_deadline` (R12)
- `python_write_batch_chunks` (R13)
- `python_streams_subscribe_resumes` (R14)
- `python_collection_handle_delegates` (SDKP §2.8)
- `python_search_query_all_requires_tiebreak` (SDKP §2.4)

The call-path clauses go over a real HTTP server speaking Connect unary, with
`loams.collection.v1.DocumentService` bindings shaped as SDKP P0a's annotations
will generate them, and every one of them runs on both the sync and the async
client. The loops (`write_batch`, `subscribe`, `query_all`) are pinned against
stub callables, since what they own is the loop, not the transport.
"""

from __future__ import annotations

import asyncio
import http.server
import json
import threading
import time
from collections.abc import Callable, Iterator
from typing import Any

import pytest
from connectrpc.method import IdempotencyLevel, MethodInfo
from google.protobuf.message import Message

from loams import AsyncLoams, CallOptions, ConsistencyOptions, Loams
from loams._gen.facade import METHODS, MODULES, CallBinding
from loams.app import AsyncSearchCalls, DocumentCalls, SearchCalls
from loams.builders import Filter, Search, Sort
from loams.collection.v1.document_pb2 import (
    GetDocumentsRequest,
    GetDocumentsResponse,
    WriteDocumentsRequest,
    WriteDocumentsResponse,
)
from loams.handle import AsyncCollectionHandle, CollectionHandle
from loams.runtime.errors import BatchError, DeadlineExceededError, InvalidArgumentError
from loams.runtime.subscribe import AsyncSubscription, SubscribeRequest, Subscription

SERVICE = "loams.collection.v1.DocumentService"
KINDS = ["sync", "async"]


def _binding(name: str, method: str, retry: str) -> CallBinding:
    return CallBinding(
        module="documents",
        name=name,
        proto_name=name,
        method=method,
        rpc=f"{SERVICE}/{method}",
        service=SERVICE,
        package="loams.collection.v1",
        idempotency="no_side_effects" if retry == "safe" else "none",  # type: ignore[arg-type]
        retry=retry,  # type: ignore[arg-type]
        streaming="unary",
        pagination=None,
    )


WRITE = _binding("write", "WriteDocuments", "manual")
GET = _binding("get", "GetDocuments", "safe")

_TYPES: dict[str, tuple[type[Message], type[Message]]] = {
    "WriteDocuments": (WriteDocumentsRequest, WriteDocumentsResponse),
    "GetDocuments": (GetDocumentsRequest, GetDocumentsResponse),
}


@pytest.fixture(autouse=True)
def _document_methods(monkeypatch: pytest.MonkeyPatch) -> None:
    """The `MethodInfo`s P0a will generate, until it does."""
    for method, (request, response) in _TYPES.items():
        monkeypatch.setitem(
            METHODS,
            f"{SERVICE}/{method}",
            MethodInfo(
                name=method,
                service_name=SERVICE,
                input=request,
                output=response,
                idempotency_level=IdempotencyLevel.NO_SIDE_EFFECTS
                if method == "GetDocuments"
                else IdempotencyLevel.UNKNOWN,
            ),
        )


Answer = Callable[[str, Message, int], "tuple[Message | None, dict[str, str], float, int]"]


class _AppServer:
    """Answers `DocumentService` over Connect unary and records what it was sent.

    `answer(method, request, n)` returns `(response, headers, delay, status)`;
    a non-200 status answers a Connect error with that HTTP status.
    """

    def __init__(self, answer: Answer) -> None:
        self.requests: list[tuple[str, Message, dict[str, str]]] = []
        self._lock = threading.Lock()
        outer = self

        class Handler(http.server.BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.1"

            def log_message(self, *args: object) -> None:
                """Keep the test output clean."""

            def do_POST(self) -> None:
                body = self.rfile.read(int(self.headers.get("content-length", "0")))
                method = self.path.rsplit("/", 1)[-1]
                request_type, _ = _TYPES[method]
                request = request_type.FromString(body)
                with outer._lock:
                    outer.requests.append(
                        (method, request, {k.lower(): v for k, v in self.headers.items()})
                    )
                    n = len(outer.requests)
                response, headers, delay, status = answer(method, request, n)
                if delay:
                    time.sleep(delay)
                if status != 200:
                    payload = json.dumps({"code": "unavailable", "message": "restarting"}).encode()
                    content_type = "application/json"
                else:
                    assert response is not None
                    payload = response.SerializeToString()
                    content_type = "application/proto"
                try:
                    self.send_response(status)
                    self.send_header("content-type", content_type)
                    self.send_header("content-length", str(len(payload)))
                    for name, value in headers.items():
                        self.send_header(name, value)
                    self.end_headers()
                    self.wfile.write(payload)
                except (BrokenPipeError, ConnectionResetError):
                    pass

        self._httpd = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self._httpd.daemon_threads = True
        self.url = f"http://127.0.0.1:{self._httpd.server_address[1]}"
        self._thread = threading.Thread(target=self._httpd.serve_forever, daemon=True)
        self._thread.start()

    def close(self) -> None:
        self._httpd.shutdown()
        self._httpd.server_close()
        self._thread.join(timeout=5)

    def sent(self, method: str) -> list[Message]:
        with self._lock:
            return [request for name, request, _ in self.requests if name == method]

    def headers(self, index: int) -> dict[str, str]:
        with self._lock:
            return self.requests[index][2]


def _ok(method: str, response: Message | None = None) -> tuple[Message, dict[str, str], float, int]:
    _, response_type = _TYPES[method]
    return (response if response is not None else response_type(), {}, 0.0, 200)


@pytest.fixture
def make_server() -> Iterator[Callable[[Answer], _AppServer]]:
    started: list[_AppServer] = []

    def make(answer: Answer) -> _AppServer:
        server = _AppServer(answer)
        started.append(server)
        return server

    yield make
    for server in started:
        server.close()


def call(kind: str, url: str, binding: CallBinding, request: Message, options: CallOptions | None = None, **client: Any) -> Message:
    """One call through the real invoker of the sync or the async client."""
    if kind == "sync":
        with Loams(url, **client) as sync:
            return sync._invoker.unary(binding, request, options)

    async def run() -> Message:
        async with AsyncLoams(url, **client) as aio:
            return await aio._invoker.unary(binding, request, options)

    return asyncio.run(run())


def calls(kind: str, url: str, steps: list[tuple[CallBinding, Message]], **client: Any) -> None:
    """Several calls on one client, so a session carries across them."""
    if kind == "sync":
        with Loams(url, **client) as sync:
            for binding, request in steps:
                sync._invoker.unary(binding, request)
        return

    async def run() -> None:
        async with AsyncLoams(url, **client) as aio:
            for binding, request in steps:
                await aio._invoker.unary(binding, request)

    asyncio.run(run())


# -- R4: python_session_read_your_writes -------------------------------------


@pytest.mark.parametrize("kind", KINDS)
def test_python_session_read_your_writes(kind: str, make_server: Callable[[Answer], _AppServer]) -> None:
    """A write's token goes into the session, and the next read is sent
    `consistency.at_least` in its field rather than as a header."""

    def answer(method: str, _request: Message, n: int) -> tuple[Message, dict[str, str], float, int]:
        if method == "WriteDocuments":
            token = "v1:s4/p0@12" if n == 1 else "v1:s4/p1@3"
            return _ok(method, WriteDocumentsResponse(token=token))
        return _ok(method)

    server = make_server(answer)
    write = WriteDocumentsRequest(collection="notes")
    read = GetDocumentsRequest(collection="notes")
    calls(kind, server.url, [(WRITE, write), (WRITE, write), (GET, read)], namespace="app",
          session_consistency=True)
    sent = server.sent("GetDocuments")[0]
    assert isinstance(sent, GetDocumentsRequest)
    assert sent.consistency.at_least == "v1:s4/p0@12,s4/p1@3"
    assert "loams-consistency" not in server.headers(2), "the field carries it, not a header"


@pytest.mark.parametrize("kind", KINDS)
def test_python_session_read_your_writes_from_the_header(
    kind: str, make_server: Callable[[Answer], _AppServer]
) -> None:
    """The `loams-consistency-token` response header reaches the session too."""

    def answer(method: str, _request: Message, _n: int) -> tuple[Message, dict[str, str], float, int]:
        response, _, delay, status = _ok(method)
        headers = {"loams-consistency-token": "v1:s9/p0@1"} if method == "WriteDocuments" else {}
        return response, headers, delay, status

    server = make_server(answer)
    calls(
        kind,
        server.url,
        [(WRITE, WriteDocumentsRequest(collection="notes")), (GET, GetDocumentsRequest(collection="notes"))],
        namespace="app",
        session_consistency=True,
    )
    sent = server.sent("GetDocuments")[0]
    assert isinstance(sent, GetDocumentsRequest)
    assert sent.consistency.at_least == "v1:s9/p0@1"


@pytest.mark.parametrize("kind", KINDS)
def test_an_explicit_consistency_wins_over_the_session(
    kind: str, make_server: Callable[[Answer], _AppServer]
) -> None:
    from loams.collection.v1.document_pb2 import Consistency, Freshness

    def answer(method: str, _request: Message, _n: int) -> tuple[Message, dict[str, str], float, int]:
        if method == "WriteDocuments":
            return _ok(method, WriteDocumentsResponse(token="v1:s1/p0@1"))
        return _ok(method)

    server = make_server(answer)
    eventual = GetDocumentsRequest(
        collection="notes", consistency=Consistency(freshness=Freshness.FRESHNESS_EVENTUAL)
    )
    calls(kind, server.url, [(WRITE, WriteDocumentsRequest(collection="notes")), (GET, eventual)],
          namespace="app", session_consistency=True)
    sent = server.sent("GetDocuments")[0]
    assert isinstance(sent, GetDocumentsRequest)
    assert sent.consistency.at_least == ""
    assert sent.consistency.freshness == Freshness.FRESHNESS_EVENTUAL


@pytest.mark.parametrize("kind", KINDS)
def test_a_call_opts_out_of_the_session(kind: str, make_server: Callable[[Answer], _AppServer]) -> None:
    server = make_server(lambda method, _r, _n: _ok(method))
    store_token = "v1:s1/p0@1"
    if kind == "sync":
        with Loams(server.url, namespace="app", session_consistency=True) as client:
            assert client.consistency is not None
            client.consistency.record(store_token, "loams.collection.v1")
            client._invoker.unary(
                GET,
                GetDocumentsRequest(collection="notes"),
                CallOptions(consistency=ConsistencyOptions(session=False)),
            )
    else:

        async def run() -> None:
            async with AsyncLoams(server.url, namespace="app", session_consistency=True) as client:
                assert client.consistency is not None
                client.consistency.record(store_token, "loams.collection.v1")
                await client._invoker.unary(
                    GET,
                    GetDocumentsRequest(collection="notes"),
                    CallOptions(consistency=ConsistencyOptions(session=False)),
                )

        asyncio.run(run())
    sent = server.sent("GetDocuments")[0]
    assert isinstance(sent, GetDocumentsRequest)
    assert not sent.HasField("consistency")


# -- R11: python_default_namespace -------------------------------------------


@pytest.mark.parametrize("kind", KINDS)
def test_python_default_namespace(kind: str, make_server: Callable[[Answer], _AppServer]) -> None:
    server = make_server(lambda method, _r, _n: _ok(method))
    call(kind, server.url, GET, GetDocumentsRequest(collection="notes"), namespace="app")
    call(kind, server.url, GET, GetDocumentsRequest(namespace="other", collection="notes"), namespace="app")
    sent = server.sent("GetDocuments")
    assert [getattr(r, "namespace") for r in sent] == ["app", "other"]


@pytest.mark.parametrize("kind", KINDS)
def test_python_default_namespace_refuses_neither_before_any_call(
    kind: str, make_server: Callable[[Answer], _AppServer]
) -> None:
    server = make_server(lambda method, _r, _n: _ok(method))
    with pytest.raises(InvalidArgumentError) as caught:
        call(kind, server.url, GET, GetDocumentsRequest(collection="notes"))
    assert caught.value.metadata["field"] == "namespace"
    assert caught.value.reason == "namespace_required"
    assert server.requests == [], "the refusal went over the network"


@pytest.mark.parametrize("kind", KINDS)
def test_python_default_namespace_leaves_a_namespace_field_outside_the_app_packages_alone(
    kind: str,
) -> None:
    """`loams.live.v1.CreateAppRequest.namespace` is free text naming an app's
    environment: R11 must neither fill it nor refuse it."""
    from loams.live.v1.admin_pb2 import CreateAppRequest
    from loams.runtime.call import prepare_request

    create_app = next(
        call
        for module in MODULES
        for call in module.calls
        if call.rpc == "loams.live.v1.LiveAdminService/CreateApp"
    )
    request = CreateAppRequest(name="shop")
    for namespace in (None, "app"):
        assert prepare_request(create_app, request, namespace, "v1:s1/p0@1") is request
    # And through each client's plan: no refusal, nothing filled.
    if kind == "sync":
        with Loams("http://127.0.0.1:1", namespace="app") as client:
            plan = client._invoker._plan(create_app, CallOptions(), request)
    else:
        client_async = AsyncLoams("http://127.0.0.1:1", namespace="app")
        plan = client_async._invoker._plan(create_app, CallOptions(), request)
    assert getattr(plan[0], "namespace") == ""


def test_python_default_namespace_leaves_the_callers_request_alone(
    make_server: Callable[[Answer], _AppServer],
) -> None:
    server = make_server(lambda method, _r, _n: _ok(method))
    request = GetDocumentsRequest(collection="notes")
    call("sync", server.url, GET, request, namespace="app")
    assert request.namespace == "", "the caller's request was mutated"


def test_python_default_namespace_from_env() -> None:
    env = {"LOAMS_ENDPOINT": "http://127.0.0.1:1", "LOAMS_NAMESPACE": "env", "LOAMS_API_KEY": "k"}
    with Loams.from_env(env) as client:
        assert client.namespace == "env"
    with Loams.from_env(env, namespace="kw") as client:
        assert client.namespace == "kw"
    assert AsyncLoams.from_env(env).namespace == "env"
    with pytest.raises(Exception, match="LOAMS_ENDPOINT"):
        Loams.from_env({})


# -- R12: python_timeout_deadline --------------------------------------------


@pytest.mark.parametrize("kind", KINDS)
def test_python_timeout_deadline(kind: str, make_server: Callable[[Answer], _AppServer]) -> None:
    """The client's deadline raises `client_deadline_exceeded` and is not
    retried, even on a `safe` call with retries left."""

    def slow(method: str, _r: Message, _n: int) -> tuple[Message, dict[str, str], float, int]:
        response, headers, _, status = _ok(method)
        return response, headers, 1.5, status

    server = make_server(slow)
    started = time.monotonic()
    with pytest.raises(DeadlineExceededError) as caught:
        call(kind, server.url, GET, GetDocumentsRequest(collection="notes"), namespace="app",
             timeout_ms=200, max_retries=3)
    assert caught.value.reason == "client_deadline_exceeded"
    assert time.monotonic() - started < 1.4
    assert len(server.requests) == 1, "a spent budget was retried"
    budget = int(server.headers(0).get("connect-timeout-ms", "0"))
    assert 0 < budget <= 200, "the budget did not go out as connect-timeout-ms"


@pytest.mark.parametrize("kind", KINDS)
def test_python_timeout_deadline_per_call_override(
    kind: str, make_server: Callable[[Answer], _AppServer]
) -> None:
    def slowish(method: str, _r: Message, _n: int) -> tuple[Message, dict[str, str], float, int]:
        response, headers, _, status = _ok(method)
        return response, headers, 0.3, status

    server = make_server(slowish)
    request = GetDocumentsRequest(collection="notes")
    # 0 disables the client's 100 ms deadline; a generous per-call one also wins.
    call(kind, server.url, GET, request, CallOptions(timeout_ms=0), namespace="app", timeout_ms=100)
    call(kind, server.url, GET, request, CallOptions(timeout_ms=5_000), namespace="app", timeout_ms=100)
    with pytest.raises(DeadlineExceededError):
        call(kind, server.url, GET, request, namespace="app", timeout_ms=100)


@pytest.mark.parametrize("kind", KINDS)
def test_python_timeout_deadline_defaults_to_30_s(
    kind: str, make_server: Callable[[Answer], _AppServer]
) -> None:
    server = make_server(lambda method, _r, _n: _ok(method))
    call(kind, server.url, GET, GetDocumentsRequest(collection="notes"), namespace="app")
    budget = int(server.headers(0).get("connect-timeout-ms", "0"))
    assert 29_000 < budget <= 30_000


# -- R13: python_write_batch_chunks ------------------------------------------


def _docs(n: int) -> list[dict[str, Any]]:
    return [{"id": i + 1, "source": {"title": f"note {i + 1}"}} for i in range(n)]


def test_python_write_batch_chunks() -> None:
    """Chunks by rows, one key per chunk, and the tokens merge."""
    sent: list[tuple[WriteDocumentsRequest, str | None]] = []

    def write(request: Message, options: CallOptions | None) -> WriteDocumentsResponse:
        assert isinstance(request, WriteDocumentsRequest)
        sent.append((request, options.idempotency_key if options else None))
        n = len(sent)
        return WriteDocumentsResponse(token=f"v1:s1/p{n % 2}@{n * 10}")

    result = DocumentCalls(write, None).write_batch("notes", _docs(5), chunk_rows=2)
    assert (result.chunks, result.rows) == (3, 5)
    assert [len(r.ops) for r, _ in sent] == [2, 2, 1]
    assert len({key for _, key in sent}) == 3
    assert result.token == "v1:s1/p0@20,s1/p1@30"


def test_python_write_batch_chunks_by_bytes() -> None:
    sent: list[WriteDocumentsRequest] = []

    def write(request: Message, _options: CallOptions | None) -> WriteDocumentsResponse:
        assert isinstance(request, WriteDocumentsRequest)
        sent.append(request)
        return WriteDocumentsResponse()

    big = [{"id": i, "source": {"body": "x" * 400}} for i in (1, 2, 3)]
    DocumentCalls(write, None).write_batch("notes", big, chunk_bytes=1_000)
    assert [len(r.ops) for r in sent] == [2, 1]


def test_python_write_batch_chunks_stops_at_the_first_failure() -> None:
    from connectrpc.code import Code

    calls_made = 0

    def write(_request: Message, _options: CallOptions | None) -> WriteDocumentsResponse:
        nonlocal calls_made
        calls_made += 1
        if calls_made == 2:
            raise InvalidArgumentError("bad doc", code=Code.INVALID_ARGUMENT)
        return WriteDocumentsResponse(token="v1:s1/p0@5")

    with pytest.raises(BatchError) as caught:
        DocumentCalls(write, None).write_batch("notes", _docs(6), chunk_rows=2)
    error = caught.value
    assert error.reason == "batch_partially_applied"
    assert error.code == Code.INVALID_ARGUMENT
    assert (error.chunks_applied, error.rows_applied) == (1, 2)
    assert error.token == "v1:s1/p0@5"
    assert calls_made == 2


@pytest.mark.parametrize("kind", KINDS)
def test_python_write_batch_chunks_reuses_a_chunks_key_on_retry(
    kind: str, make_server: Callable[[Answer], _AppServer]
) -> None:
    def flaky(method: str, _r: Message, n: int) -> tuple[Message | None, dict[str, str], float, int]:
        if n == 1:
            return None, {}, 0.0, 503
        return _ok(method)

    server = make_server(flaky)  # type: ignore[arg-type]
    docs = _docs(1)
    if kind == "sync":
        with Loams(server.url, namespace="app", max_retries=1) as client:
            DocumentCalls(lambda r, o: client._invoker.unary(WRITE, r, o), None).write_batch("notes", docs)
    else:
        from loams.app import AsyncDocumentCalls

        async def run() -> None:
            async with AsyncLoams(server.url, namespace="app", max_retries=1) as client:
                await AsyncDocumentCalls(
                    lambda r, o: client._invoker.unary(WRITE, r, o), None
                ).write_batch("notes", docs)

        asyncio.run(run())
    keys = [getattr(r, "idempotency_key") for r in server.sent("WriteDocuments")]
    assert len(keys) == 2
    assert keys[0] != "" and keys[0] == keys[1]


# -- R14: python_streams_subscribe_resumes -----------------------------------


class _Record:
    def __init__(self, offset: int) -> None:
        self.offset = offset
        self.value = f"r{offset}"


class _Fetched:
    def __init__(self, records: list[_Record]) -> None:
        self.records = records


def _log() -> tuple[Callable[[dict[str, Any]], _Fetched], list[dict[str, Any]]]:
    """A partition holding offsets 0..4, served two at a time."""
    requests: list[dict[str, Any]] = []

    def fetch(fields: dict[str, Any]) -> _Fetched:
        requests.append(fields)
        start = int(fields["offset"])
        return _Fetched([_Record(o) for o in range(5) if o >= start][:2])

    return fetch, requests


def test_python_streams_subscribe_resumes() -> None:
    fetch, requests = _log()
    sub = Subscription(fetch, SubscribeRequest("events", from_offset=1))
    seen = []
    for record in sub:
        seen.append(record.value)
        if len(seen) == 3:
            break
    assert seen == ["r1", "r2", "r3"]
    assert sub.offset == 4
    assert [r["offset"] for r in requests] == [1, 3]

    resumed = Subscription(fetch, SubscribeRequest("events", from_offset=sub.offset))
    assert next(resumed).value == "r4"
    resumed.cancel()
    assert list(resumed) == []


def test_python_streams_subscribe_backs_off_on_an_empty_fetch() -> None:
    pauses: list[float] = []
    fetches = 0
    sub: Subscription

    def fetch(_fields: dict[str, Any]) -> _Fetched:
        nonlocal fetches
        fetches += 1
        if fetches == 6:
            sub.cancel()
        return _Fetched([])

    sub = Subscription(fetch, SubscribeRequest("events"), sleep=pauses.append)
    assert list(sub) == []
    assert pauses == [0.1, 0.2, 0.4, 0.8, 1.0]


def test_python_streams_subscribe_resumes_async() -> None:
    fetch, requests = _log()

    async def afetch(fields: dict[str, Any]) -> _Fetched:
        return fetch(fields)

    async def run() -> tuple[list[str], int]:
        sub = AsyncSubscription(afetch, SubscribeRequest("events", from_offset=1))
        seen: list[str] = []
        async for record in sub:
            seen.append(record.value)
            if len(seen) == 3:
                break
        return seen, sub.offset

    seen, offset = asyncio.run(run())
    assert seen == ["r1", "r2", "r3"]
    assert offset == 4


def test_python_streams_subscribe_cancel_wakes_an_async_backoff() -> None:
    async def empty(_fields: dict[str, Any]) -> _Fetched:
        return _Fetched([])

    async def run() -> float:
        sub = AsyncSubscription(empty, SubscribeRequest("events"))
        started = time.monotonic()
        pending = asyncio.ensure_future(sub.__anext__())
        await asyncio.sleep(0.05)
        sub.cancel()
        with pytest.raises(StopAsyncIteration):
            await pending
        return time.monotonic() - started

    assert asyncio.run(run()) < 0.5


# -- SDKP §2.8: python_collection_handle_delegates ---------------------------


class _Recorder:
    """A module whose every call records its arguments."""

    def __init__(self, calls: list[tuple[str, tuple[Any, ...], dict[str, Any]]], is_async: bool) -> None:
        self._calls = calls
        self._async = is_async

    def __getattr__(self, name: str) -> Any:
        def record(*args: Any, **kwargs: Any) -> Any:
            self._calls.append((name, args, kwargs))
            if self._async and name != "scroll_all":
                async def done() -> None:
                    return None

                return done()
            return iter([]) if name == "scroll_all" else None

        return record


def _handle_modules(is_async: bool) -> tuple[dict[str, Any], list[tuple[str, tuple[Any, ...], dict[str, Any]]]]:
    calls: list[tuple[str, tuple[Any, ...], dict[str, Any]]] = []
    recorder = _Recorder(calls, is_async)
    return {"documents": recorder, "search": recorder, "collections": recorder}, calls


def _check_delegation(calls: list[tuple[str, tuple[Any, ...], dict[str, Any]]]) -> None:
    by_name = {name: (args, kwargs) for name, args, kwargs in calls}
    for name in ("upsert", "delete", "patch", "write_batch"):
        args, kwargs = by_name[name]
        assert args[0] == "notes" and kwargs["namespace"] == "app", name
    assert by_name["scroll_all"][0] == ("notes",)
    assert by_name["scroll_all"][1]["namespace"] == "app"
    # `get` is called twice: `documents.get`, then `collections.get` for `info`.
    gets = [args[0] for n, args, _ in calls if n == "get"]
    assert [type(r).__name__ for r in gets] == ["GetDocumentsRequest", "GetCollectionRequest"]
    for request in [*gets, by_name["count"][0][0], by_name["drop"][0][0]]:
        assert (request.namespace, request.collection) == ("app", "notes"), type(request).__name__
    searched = [args[0] for n, args, _ in calls if n == "query"]
    assert len(searched) == 2
    for request in searched:
        assert (request.namespace, request.collection) == ("app", "notes")


def test_python_collection_handle_delegates() -> None:
    modules, calls = _handle_modules(False)
    notes = CollectionHandle(modules, "notes", namespace="app")
    notes.upsert([{"id": 1}])
    notes.delete([1])
    notes.patch(1, set={"a": 1})
    notes.write_batch([{"id": 2}])
    notes.get([1])
    notes.count()
    list(notes.scroll_all())
    notes.search(Search("ignored").text("refund"))
    notes.vector_search("embedding", [1, 0, 0, 0], limit=3)
    notes.info()
    notes.drop()
    _check_delegation(calls)


def test_python_collection_handle_delegates_async() -> None:
    modules, calls = _handle_modules(True)
    notes = AsyncCollectionHandle(modules, "notes", namespace="app")

    async def run() -> None:
        await notes.upsert([{"id": 1}])
        await notes.delete([1])
        await notes.patch(1, set={"a": 1})
        await notes.write_batch([{"id": 2}])
        await notes.get([1])
        await notes.count()
        notes.scroll_all()
        await notes.search(Search("ignored").text("refund"))
        await notes.vector_search("embedding", [1, 0, 0, 0], limit=3)
        await notes.info()
        await notes.drop()

    asyncio.run(run())
    _check_delegation(calls)


def test_python_collection_handle_is_what_loams_collection_returns() -> None:
    with Loams("http://127.0.0.1:1", namespace="app") as client:
        handle = client.collection("notes")
    assert isinstance(handle, CollectionHandle)
    assert handle.name == "notes"
    assert isinstance(AsyncLoams("http://127.0.0.1:1").collection("notes"), AsyncCollectionHandle)


# -- SDKP §2.4: python_search_query_all_requires_tiebreak --------------------


class _Hit:
    def __init__(self, pk: int) -> None:
        from google.protobuf.struct_pb2 import Value

        self.pk = pk
        self.sort_values = [Value(number_value=float(pk))]


class _Page:
    def __init__(self, hits: list[_Hit]) -> None:
        self.hits = hits


def test_python_search_query_all_requires_tiebreak() -> None:
    queries: list[Message] = []

    def query(request: Message, _options: CallOptions | None) -> _Page:
        queries.append(request)
        return _Page([])

    search = SearchCalls(query)
    no_sort = Search("notes").text("refund").limit(2)
    with pytest.raises(InvalidArgumentError) as caught:
        next(search.query_all(no_sort))
    assert caught.value.reason == "search_after_needs_tiebreak"
    with pytest.raises(InvalidArgumentError):
        next(search.query_all(no_sort.sort(Sort.score())))
    assert queries == []


def test_python_search_query_all_pages_with_search_after() -> None:
    pages = [[1, 2], [3, 4], [5]]
    queries: list[Any] = []

    def query(request: Message, _options: CallOptions | None) -> _Page:
        queries.append(request)
        return _Page([_Hit(pk) for pk in pages[len(queries) - 1]])

    builder = Search("notes").text("refund").sort(Sort.score(), Sort.pk()).limit(2)
    assert [hit.pk for hit in SearchCalls(query).query_all(builder)] == [1, 2, 3, 4, 5]
    assert len(queries) == 3
    assert list(queries[0].search_after) == []
    assert queries[1].search_after[0].number_value == 2.0


def test_python_search_query_all_requires_tiebreak_async() -> None:
    async def query(_request: Message, _options: CallOptions | None) -> _Page:
        return _Page([])

    async def run() -> None:
        with pytest.raises(InvalidArgumentError):
            await AsyncSearchCalls(query).query_all(Search("notes").text("refund")).__anext__()

    asyncio.run(run())


def test_the_filter_builder_feeds_a_search() -> None:
    request = Search("notes").filter(Filter.eq("tags", "policy")).build()
    assert request.filter.term.field == "tags"  # type: ignore[attr-defined]


# -- the two TypeScript defects, pinned absent here ---------------------------


def test_a_renamed_facade_call_resolves_its_rpc_not_its_facade_name() -> None:
    """`admin.keys.create` is `KeyService/CreateKey`. The TypeScript invoker looked
    the method up by the facade name and failed; this one keys `METHODS` by RPC."""
    from loams.runtime.call import _method_of

    create = next(
        call for module in MODULES if module.name == "admin.keys" for call in module.calls
        if call.name == "create"
    )
    assert _method_of(create).name == "CreateKey"


def test_paging_reads_an_item_field_with_an_underscore() -> None:
    """The TypeScript iterator dropped `service_accounts` items; Python messages
    use the proto's own field names, which this pins."""
    from loams.admin.v1.agent_pb2 import (
        ListServiceAccountsRequest,
        ListServiceAccountsResponse,
        ServiceAccount,
    )
    from loams.runtime.pagination import paginate

    binding = next(
        call for module in MODULES for call in module.calls
        if call.pagination is not None and call.pagination.items == "service_accounts"
    )
    pages = [
        ListServiceAccountsResponse(service_accounts=[ServiceAccount(id="a")], next_page_token="p2"),
        ListServiceAccountsResponse(service_accounts=[ServiceAccount(id="b")]),
    ]
    seen = list(paginate(binding, lambda _r, _o: pages.pop(0), ListServiceAccountsRequest()))
    assert [item.id for item in seen] == ["a", "b"]


# -- R4 as amended: one token per package, and none for graph reads (SDKP-17) --


def test_the_session_keeps_one_token_per_package() -> None:
    from loams.runtime.consistency import ConsistencySession

    session = ConsistencySession()
    session.record("v1:s4/p0@12", "loams.collection.v1")
    session.record("v1:s7/p0@1", "loams.graph.v1")
    assert session.current("loams.collection.v1") == "v1:s4/p0@12"
    assert session.current("loams.graph.v1") == "v1:s7/p0@1"
    assert session.current() is None


def test_a_graph_read_never_takes_the_session_token() -> None:
    """A graph's token names one graph, and a Graph read refuses any other, so
    the session's token goes to collection and SQL reads only."""
    from loams._gen.facade import MODULES as ALL
    from loams.graph.v1.graph_pb2 import ExecuteRequest
    from loams.runtime.call import prepare_request

    execute = next(
        c for m in ALL if m.name == "graph" for c in m.calls if c.method == "Execute"
    )
    out = prepare_request(
        execute, ExecuteRequest(graph="g", statement="MATCH (n) RETURN n"), "app", "v1:s4/p0@12"
    )
    assert isinstance(out, ExecuteRequest)
    assert out.namespace == "app", "R11 still fills a graph request's namespace"
    assert not out.HasField("consistency")
