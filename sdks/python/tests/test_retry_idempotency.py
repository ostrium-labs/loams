"""SDK2 Task 2's `python_retry_reuses_idempotency_key`.

D610 and runtime contract R5: a mutation may only be retried when it carries an
idempotency key, and **the same key has to go out on every attempt**. A retry
that mints a fresh key is a second write the server cannot collapse, so the
failure this prevents is a duplicated mutation, not a failed one.

The key travels in the request body (`MutateRequest.idempotency_key`, proto3
`optional`), not as a header, so the server here decodes each attempt's body and
records the key it carried. Asserting a header would have passed against a
server that never saw one.

Pinned:

- every attempt of a retried mutation carries the same key;
- a key the caller supplied is the one that goes out, on every attempt;
- a minted key is a UUIDv7, so keys sort in issue order;
- two logical calls get different keys, or one would be deduplicated against
  the other;
- the caller's message is never mutated -- the key goes on a clone, because the
  same request object gets reused;
- a request whose schema declares no `idempotency_key` is not keyed, and is left
  exactly as the caller wrote it;
- a read retries without a key, because it is `safe`, which is a different reason
  from a mutation being retryable.
"""

from __future__ import annotations

import asyncio
import http.server
import threading
import uuid
from collections.abc import Iterator

import pytest

from connectrpc.code import Code

from loams import AsyncLoams, Loams, UnavailableError
from loams.instance.v1.instance_pb2 import GetInstanceRequest, GetInstanceResponse
from loams.live.v1.live_pb2 import (
    DeployRequest,
    MutateRequest,
    MutateResponse,
    QueryRequest,
    WatchRequest,
)
from loams.runtime.call import apply_idempotency_key
from loams.runtime.options import CallOptions
from loams.runtime.retry import is_retryable_code
from loams.runtime.uuidv7 import uuidv7

MUTATE_RPC = "/loams.live.v1.LiveService/Mutate"


class _RecordingServer:
    """Records each attempt's idempotency key, failing the first one.

    Reads it out of the decoded request body, which is where the key actually
    travels, so the assertion is on the wire rather than on a header the SDK
    never promised to send.
    """

    def __init__(
        self,
        *,
        fail_first: int = 1,
        request_type: type = MutateRequest,
        response_type: type = MutateResponse,
    ) -> None:
        self.keys: list[str] = []
        self.fail_first = fail_first
        self.request_type = request_type
        self.response_type = response_type
        self._lock = threading.Lock()
        self._served = 0
        outer = self

        class Handler(http.server.BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.1"

            def log_message(self, *args: object) -> None:
                """Keep the test output clean."""

            def do_POST(self) -> None:
                length = int(self.headers.get("content-length", "0"))
                request = outer.request_type.FromString(self.rfile.read(length))
                with outer._lock:
                    outer._served += 1
                    attempt = outer._served
                    outer.keys.append(getattr(request, "idempotency_key", ""))
                if attempt <= outer.fail_first:
                    body = b'{"code":"unavailable","message":"try again"}'
                    self.send_response(503)
                else:
                    body = outer.response_type().SerializeToString()
                    self.send_response(200)
                self.send_header("content-type", "application/proto")
                self.send_header("content-length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)

        self._httpd = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.url = f"http://127.0.0.1:{self._httpd.server_address[1]}"
        # Binding is not serving: without this a request hangs instead of
        # failing, which reads like an SDK bug rather than a harness one.
        self._serving = threading.Thread(target=self._httpd.serve_forever, daemon=True)
        self._serving.start()

    def close(self) -> None:
        self._httpd.shutdown()
        self._httpd.server_close()
        self._serving.join(timeout=5)

    @property
    def attempts(self) -> int:
        with self._lock:
            return self._served


@pytest.fixture
def server() -> Iterator[_RecordingServer]:
    started = _RecordingServer()
    try:
        yield started
    finally:
        started.close()


def test_python_retry_reuses_idempotency_key(server: _RecordingServer) -> None:
    """The required test: every attempt of a retried mutation carries one key."""
    # No `retry_safe` override: `Mutate`'s generated retry class is `manual`, and
    # it retries anyway because it carries a key. That is D610, and it is what
    # makes this the interesting call to pin.
    with Loams(server.url) as client:
        client.tables.mutate(MutateRequest())

    assert server.attempts == 2, f"the server saw {server.attempts} attempts, want 2"
    first, second = server.keys
    assert first != "", "the first attempt carried no idempotency key"
    assert first == second, (
        f"the retry carried a different key ({first!r} then {second!r}), so the "
        "server would have applied the write twice"
    )


def test_a_key_the_caller_supplied_is_the_one_that_goes_out(server: _RecordingServer) -> None:
    """Supplying a key makes a retry yours, not the SDK's."""
    with Loams(server.url) as client:
        client.tables.mutate(MutateRequest(), options=CallOptions(idempotency_key="my-key"))

    assert server.keys == ["my-key", "my-key"], f"the attempts carried {server.keys}"


def test_a_minted_key_is_a_uuidv7(server: _RecordingServer) -> None:
    """Unique would do for correctness; time-ordered is why it is v7.

    A v4 would still be reused across a retry, but the keys would not be
    issue-ordered, which is what makes a stalled set of mutations findable by
    range scan.
    """
    with Loams(server.url) as client:
        client.tables.mutate(MutateRequest())

    key = server.keys[0]
    parsed = uuid.UUID(key)
    assert parsed.version == 7, f"the minted key {key} is version {parsed.version}, want 7"


def test_two_logical_calls_get_different_keys() -> None:
    """Otherwise one call would be deduplicated against the other."""
    first, _ = apply_idempotency_key(MutateRequest())
    second, _ = apply_idempotency_key(MutateRequest())
    assert first.idempotency_key != second.idempotency_key, "two calls shared one key"
    assert first.idempotency_key != "" and second.idempotency_key != ""


def test_the_callers_message_is_never_mutated() -> None:
    """The key goes on a clone, because the same request object gets reused.

    Writing into the caller's message would leave a key on an object they may
    send again, and the second send would then be deduplicated against the first
    without the caller asking.
    """
    callers = MutateRequest(function="python")
    keyed, has_key = apply_idempotency_key(callers)

    assert has_key, "a request whose schema declares the key was not keyed"
    assert keyed.idempotency_key != "", "the keyed request has an empty key"
    assert callers.idempotency_key == "", (
        f"the caller's message was mutated to {callers.idempotency_key!r}"
    )
    assert keyed is not callers, "the keyed request is the caller's own message"


def test_a_key_the_caller_already_set_is_not_replaced() -> None:
    """A key on the message is theirs, whether or not it came through options."""
    keyed, has_key = apply_idempotency_key(MutateRequest(idempotency_key="mine"))
    assert has_key
    assert keyed.idempotency_key == "mine", f"the caller's key became {keyed.idempotency_key!r}"

    supplied, _ = apply_idempotency_key(MutateRequest(), "from-options")
    assert supplied.idempotency_key == "from-options"


def test_a_schema_without_the_field_is_left_exactly_as_written() -> None:
    """A key on a message whose schema has no such field is a field it cannot
    know, so there is nothing to key -- and the request is not touched."""
    for message in (DeployRequest(), QueryRequest(), WatchRequest()):
        result, has_key = apply_idempotency_key(message)
        assert not has_key, f"{type(message).__name__} was keyed despite declaring no key field"
        assert result is message, f"{type(message).__name__} was rebuilt for nothing"


def test_a_read_retries_without_a_key_and_unavailable_is_retryable() -> None:
    """A read is retryable because it is `safe`, which is a different reason.

    `GetInstance`'s retry class is `safe` and its schema declares no key, so it
    retries with no key at all -- where `Mutate` retries only because it carries
    one. Both retry; the reasons are not the same, and conflating them would let
    an unkeyed mutation be repeated.
    """
    assert is_retryable_code(Code.UNAVAILABLE), (
        "unavailable is not one of the retryable codes"
    )

    started = _RecordingServer(
        fail_first=1, request_type=GetInstanceRequest, response_type=GetInstanceResponse
    )
    try:
        with Loams(started.url) as client:
            client.instance.get_instance(GetInstanceRequest())
    finally:
        started.close()

    assert started.attempts == 2, f"the read made {started.attempts} attempts, want 2"
    assert started.keys == ["", ""], "a read should carry no key on any attempt"


# -- the async half -----------------------------------------------------------
#
# `async_call_with_retry` is a separate implementation, not a wrapper, and every
# test above drives `Loams`. An async path that reapplied `apply_idempotency_key`
# per attempt would mint a fresh key on the retry, which the server cannot
# collapse -- a duplicated mutation that still returns success, so nothing above
# would notice. Same reason the stream tests have an async half.


def test_the_async_retry_reuses_the_idempotency_key() -> None:
    """The required test's async twin, against the same recording server."""
    started = _RecordingServer()
    try:
        async def call() -> None:
            async with AsyncLoams(started.url) as client:
                await client.tables.mutate(MutateRequest())

        asyncio.run(call())
    finally:
        started.close()

    assert started.attempts == 2, f"the server saw {started.attempts} attempts, want 2"
    first, second = started.keys
    assert first != "", "the first async attempt carried no idempotency key"
    assert first == second, (
        f"the async retry carried a different key ({first!r} then {second!r}), so "
        "the write happened twice"
    )


def test_the_async_retry_does_not_mutate_the_callers_message() -> None:
    """The key goes on a clone, so the same request object can be reused."""
    started = _RecordingServer()
    callers = MutateRequest()
    try:
        async def call() -> None:
            async with AsyncLoams(started.url) as client:
                await client.tables.mutate(callers)

        asyncio.run(call())
    finally:
        started.close()
    assert callers.idempotency_key == "", (
        f"the async call mutated the caller's message to {callers.idempotency_key!r}"
    )


def test_the_async_retry_gives_up_and_reports(server: _RecordingServer) -> None:
    """Retries are bounded, and the last failure is what the caller sees."""
    server.fail_first = 99  # never succeeds

    async def call() -> None:
        async with AsyncLoams(server.url, max_retries=2) as client:
            await client.tables.mutate(MutateRequest())

    with pytest.raises(UnavailableError):
        asyncio.run(call())
    assert server.attempts == 3, (
        f"the async client made {server.attempts} attempts with max_retries=2, want 3"
    )


def test_the_async_retry_does_not_retry_a_read_for_free(server: _RecordingServer) -> None:
    """A read retries without a key, because it is `safe` -- a different reason.

    Same shape as the sync case, pinned separately because the async retry loop
    is its own code: conflating "retryable" with "keyed" in one of them would
    either retry a mutation without a key or refuse to retry a read.
    """
    server.request_type = GetInstanceRequest
    server.response_type = GetInstanceResponse

    async def call() -> None:
        async with AsyncLoams(server.url) as client:
            await client.instance.get_instance(GetInstanceRequest())

    asyncio.run(call())
    assert server.attempts == 2, f"the read made {server.attempts} attempts, want 2"
    assert server.keys == ["", ""], (
        f"a read carried an idempotency key: {server.keys}"
    )
