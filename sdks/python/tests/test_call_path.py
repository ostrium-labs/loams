"""What a single call carries, and which session it resolves to.

`call_headers` and `resolve_session` decide two things a caller cannot see from
the outside but depends on completely:

- whether their own headers reach the wire at all, and cannot displace the
  credential; and
- whether a read is `STRONG` on its own or joined to a session's token.

Both were unrun. Coverage measured with stdlib `trace --count`, judged per
statement span, put `options.headers` (the whole point of a public
`CallOptions.headers`), the `loams-consistency` header, and every branch of
`resolve_session` at zero.

The header tests go over a real HTTP server rather than calling `call_headers`
directly, because the interesting failures are not in the function. `dict.update`
followed by `headers["authorization"] = ...` is obviously correct in isolation;
what matters is that connect-python does not drop, reorder or overwrite a header
on its way out, and only a request can show that.
"""

from __future__ import annotations

import asyncio
import http.server
import threading

import pytest

from loams import AsyncLoams, CallOptions, ConsistencyOptions, Loams
from loams.instance.v1.instance_pb2 import GetInstanceRequest, GetInstanceResponse
from loams.runtime.call import call_headers, resolve_session
from loams.runtime.consistency import TOKEN_PREFIX, ConsistencySession

RPC = "/loams.instance.v1.InstanceService/GetInstance"


class _EchoServer:
    """Answers GetInstance, and keeps every header it was sent."""

    def __init__(self) -> None:
        self.seen: list[dict[str, str]] = []
        self._lock = threading.Lock()
        outer = self

        class Handler(http.server.BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.1"

            def log_message(self, *args: object) -> None:
                """Keep the test output clean."""

            def do_POST(self) -> None:
                self.rfile.read(int(self.headers.get("content-length", "0")))
                with outer._lock:
                    outer.seen.append({k.lower(): v for k, v in self.headers.items()})
                payload = GetInstanceResponse(name="Loams").SerializeToString()
                self.send_response(200)
                self.send_header("content-type", "application/proto")
                self.send_header("content-length", str(len(payload)))
                self.end_headers()
                self.wfile.write(payload)

        self._httpd = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.url = f"http://127.0.0.1:{self._httpd.server_address[1]}"
        # Binding is not serving: without this the socket accepts into the
        # backlog and every request hangs instead of erroring.
        self._serving = threading.Thread(target=self._httpd.serve_forever, daemon=True)
        self._serving.start()

    def close(self) -> None:
        self._httpd.shutdown()
        self._httpd.server_close()
        self._serving.join(timeout=5)

    @property
    def headers(self) -> dict[str, str]:
        with self._lock:
            return dict(self.seen[-1]) if self.seen else {}


@pytest.fixture
def server():
    started = _EchoServer()
    try:
        yield started
    finally:
        started.close()


# -- the caller's headers -----------------------------------------------------


def test_a_callers_headers_reach_the_wire(server: _EchoServer) -> None:
    """`CallOptions.headers` is public API. If it is dropped, a caller's tracing
    header, tenant hint or deadline metadata vanishes with no error."""
    with Loams(server.url) as client:
        client.instance.get_instance(
            GetInstanceRequest(),
            options=CallOptions(headers={"x-request-id": "abc123", "x-tenant": "acme"}),
        )
    sent = server.headers
    assert sent.get("x-request-id") == "abc123", f"the request id was dropped: {sent}"
    assert sent.get("x-tenant") == "acme", f"the tenant hint was dropped: {sent}"


def test_a_caller_header_cannot_displace_the_credential(server: _EchoServer) -> None:
    """`Authorization` is set after the caller's headers, so a caller cannot
    override the bearer -- and cannot leak one of their own into the log either."""
    with Loams(server.url, api_key="real-key") as client:
        client.instance.get_instance(
            GetInstanceRequest(),
            options=CallOptions(headers={"authorization": "Bearer not-the-key"}),
        )
    sent = server.headers
    assert sent.get("authorization") == "Bearer real-key", (
        f"a caller-supplied authorization displaced the credential: {sent.get('authorization')!r}"
    )


def test_the_credential_is_never_in_the_query_string(server: _EchoServer) -> None:
    """R1: the token travels in a header, never in the URL, where it would land in
    a proxy log and a browser history."""
    with Loams(server.url, api_key="secret-token") as client:
        client.instance.get_instance(GetInstanceRequest())
    # The stub records headers only, so assert on what connect-python built: the
    # request target is the RPC path with no query string.
    with Loams(server.url, api_key="secret-token") as client:
        binding = client.binding("instance", "get_instance")
    assert "?" not in binding.rpc
    assert server.headers.get("authorization") == "Bearer secret-token"


def test_no_credential_means_no_authorization_header(server: _EchoServer) -> None:
    """`auth=None` is a valid client, and must not send an empty bearer."""
    with Loams(server.url) as client:
        client.instance.get_instance(GetInstanceRequest())
    assert "authorization" not in server.headers, (
        f"an empty credential was sent: {server.headers.get('authorization')!r}"
    )


def test_caller_headers_reach_the_wire_on_the_async_client(server: _EchoServer) -> None:
    """The async client builds its own headers through `_bearer`, so the caller's
    are a separate code path rather than a shared one."""

    async def walk() -> None:
        async with AsyncLoams(server.url) as client:
            await client.instance.get_instance(
                GetInstanceRequest(), options=CallOptions(headers={"x-request-id": "async-1"})
            )

    asyncio.run(walk())
    assert server.headers.get("x-request-id") == "async-1", (
        f"the async client dropped the header: {server.headers}"
    )


# -- the unit itself, for the cases a server cannot show ----------------------


def test_call_headers_assembles_the_three_sources() -> None:
    headers = call_headers(
        CallOptions(headers={"x-a": "1"}), "tok", f"{TOKEN_PREFIX}3/7"
    )
    assert headers["x-a"] == "1"
    assert headers["authorization"] == "Bearer tok"
    assert headers["loams-consistency"] == f"{TOKEN_PREFIX}3/7"


def test_call_headers_omits_what_it_does_not_have() -> None:
    """`None` for a bearer or a token means "do not send it", not "send empty"."""
    assert call_headers(CallOptions(), None, None) == {}


def test_call_headers_copies_rather_than_aliases() -> None:
    """The result is handed to connect-python, which may keep it; a caller mutating
    their own mapping afterwards must not change a client's headers."""
    original = {"x-a": "1"}
    built = call_headers(CallOptions(headers=original), None, None)
    built["x-a"] = "changed"
    assert original["x-a"] == "1", "the caller's mapping was aliased"


# -- which session a call resolves to -----------------------------------------


def test_no_consistency_options_takes_the_clients_own_session() -> None:
    """The client flag means *across calls*, so a call that states no preference
    joins the client's session rather than silently joining none.

    It used to return `None` here, which made `Loams(session_consistency=True)`
    inert: every call had to repeat `ConsistencyOptions(session=True)` or it
    carried no token, and nothing said so.
    """
    client_session = ConsistencySession()
    assert resolve_session(None, client_session) is client_session


def test_no_consistency_options_and_no_client_session_is_none() -> None:
    """The default client, so the default is still no session (D609)."""
    assert resolve_session(None, None) is None


def test_session_false_means_none() -> None:
    """The default, and the way a caller opts a single call out of a client's
    session. `False` is not "use the client's"."""
    client_session = ConsistencySession()
    assert resolve_session(ConsistencyOptions(session=False), client_session) is None


def test_session_true_means_the_clients_own() -> None:
    client_session = ConsistencySession()
    assert resolve_session(ConsistencyOptions(session=True), client_session) is client_session


def test_a_call_can_bring_its_own_store() -> None:
    """A caller who wants a token for one call only, without disturbing the
    client's session."""
    client_session = ConsistencySession()
    own = ConsistencySession()
    assert resolve_session(ConsistencyOptions(session=own), client_session) is own


def test_session_true_with_no_client_session_is_none() -> None:
    """Asking for the client's session on a client that has none must not
    manufacture a store -- it would silently never carry a token."""
    assert resolve_session(ConsistencyOptions(session=True), None) is None


def test_the_consistency_header_goes_out_when_the_store_has_a_token() -> None:
    """The store is the only thing that ever attaches a session token, so the
    wiring from store to header is what this proves.

    Driven against the store the client actually built, via the public
    `loams.consistency` accessor -- rather than by replacing `_consistency` after
    construction, which quietly does nothing: the invoker captures the store at
    construction time, so the replacement is never read. That is a trap worth
    naming, and the reason this test uses the real object.

    Today the store can only be filled by hand, since no token can reach it (see
    tests/test_consistency.py). Filling it by hand is exactly how the header path
    gets proven anyway.
    """
    started = _EchoServer()
    try:
        with Loams(started.url, session_consistency=True) as client:
            store = client.consistency
            assert store is not None, "session_consistency=True built no store"
            assert store.current() is None, "the store started out holding a token"
            store.record(f"{TOKEN_PREFIX}3/7")
            client.instance.get_instance(GetInstanceRequest())
    finally:
        started.close()

    assert started.headers.get("loams-consistency") == f"{TOKEN_PREFIX}3/7", (
        f"the session token was not sent: {started.headers.get('loams-consistency')!r}"
    )


def test_an_empty_store_sends_no_consistency_header(server: _EchoServer) -> None:
    """The header appears only when there is a token to put in it.

    Sending the header with an empty value would be worse than sending nothing: a
    server reading `loams-consistency` cannot tell "no session" from "a session
    whose token is the empty string", and would treat the read as joined to a
    session that does not exist.
    """
    with Loams(server.url, session_consistency=True) as client:
        client.instance.get_instance(GetInstanceRequest())
    assert "loams-consistency" not in server.headers, (
        "a session header went out with an empty store, which a server could read "
        "as a real token"
    )