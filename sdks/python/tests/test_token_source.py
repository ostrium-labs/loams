"""SDK2 Task 2's `python_token_source_refresh`.

Design §44 §7.4, D608 and runtime contract R1: a rejection carrying `reason =
token_expired` gets **exactly one** refresh and **one** retry. Both halves
matter. Refreshing more than once turns an auth outage into a refresh storm;
retrying without refreshing replays a token the server has already rejected.

What is pinned here is the SDK's half: the refresh-once-and-retry loop and the
sharing of one in-flight refresh across concurrent callers. The RFC 8693 token
exchange itself is written to the documented protocol and is not exercised,
because the instance serves no OAuth endpoint yet -- that is stated in
`token_source.py` and in the PR.

The loop is pinned against a real HTTP server, because the interesting part is
what goes out on the wire: one call with the stale bearer, then one with the
fresh one. A unit test of the token source cannot show that the transport
re-sent the second.
"""

from __future__ import annotations

import base64
import http.server
import threading
from collections.abc import Iterator

import pytest

from loams import Loams, TokenExpiredError
from loams.errors.v1.errors_pb2 import ErrorInfo
from loams.instance.v1.instance_pb2 import GetInstanceRequest, GetInstanceResponse
from loams.runtime.errors import UnauthenticatedError
from loams.runtime.token_source import refreshing

#: Connect's JSON encoding puts the bare message name in a detail's `type`,
#: and the client prefixes it. A Go peer or a recorder sends the full type
#: URL instead, which then gets prefixed again; both must resolve.
ERROR_INFO_MESSAGE = "loams.errors.v1.ErrorInfo"
ERROR_INFO_TYPE_URL = "type.googleapis.com/loams.errors.v1.ErrorInfo"
INSTANCE_RPC = "/loams.instance.v1.InstanceService/GetInstance"


def _token_expired_body(type_name: str = ERROR_INFO_MESSAGE) -> bytes:
    """The Connect error body for an expired access token, as the server sends it."""
    info = ErrorInfo(reason="token_expired", hint="refresh and try again")
    encoded = base64.b64encode(info.SerializeToString()).decode("ascii")
    return (
        '{"code":"unauthenticated","message":"the access token expired",'
        f'"details":[{{"type":"{type_name}","value":"{encoded}"}}]}}'
    ).encode()


class _Server:
    """A server that refuses everything until it sees the expected bearer.

    `with_error` decides whether the refusal carries an `ErrorInfo`. A
    rejection with no reason is a different path: the runtime cannot tell it was
    an expiry, so it must not refresh at all.
    """

    def __init__(
        self,
        expected: str,
        *,
        with_error: bool = True,
        type_name: str = ERROR_INFO_MESSAGE,
    ) -> None:
        self.expected = expected
        self.with_error = with_error
        self.type_name = type_name
        self.bearers: list[str] = []
        self._lock = threading.Lock()
        outer = self

        class Handler(http.server.BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.1"

            def log_message(self, *args: object) -> None:
                """Keep the test output clean."""

            def do_POST(self) -> None:
                length = int(self.headers.get("content-length", "0"))
                self.rfile.read(length)
                bearer = self.headers.get("authorization", "")
                with outer._lock:
                    outer.bearers.append(bearer)
                if self.path != INSTANCE_RPC or bearer != f"Bearer {outer.expected}":
                    if outer.with_error:
                        body = _token_expired_body(outer.type_name)
                        self.send_response(401)
                        self.send_header("content-type", "application/json")
                        self.send_header("content-length", str(len(body)))
                        self.end_headers()
                        self.wfile.write(body)
                    else:
                        body = b'{"code":"unauthenticated","message":"no credential"}'
                        self.send_response(401)
                        self.send_header("content-type", "application/json")
                        self.send_header("content-length", str(len(body)))
                        self.end_headers()
                        self.wfile.write(body)
                    return
                payload = GetInstanceResponse(name="Loams").SerializeToString()
                self.send_response(200)
                self.send_header("content-type", "application/proto")
                self.send_header("content-length", str(len(payload)))
                self.end_headers()
                self.wfile.write(payload)

        self._httpd = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.url = f"http://127.0.0.1:{self._httpd.server_address[1]}"
        # Binding is not serving: without this the socket accepts into the
        # backlog and every request hangs instead of erroring, which reads like
        # an SDK bug rather than a harness one.
        self._serving = threading.Thread(target=self._httpd.serve_forever, daemon=True)
        self._serving.start()

    def close(self) -> None:
        self._httpd.shutdown()
        self._httpd.server_close()
        self._serving.join(timeout=5)

    @property
    def attempts(self) -> int:
        with self._lock:
            return len(self.bearers)


@pytest.fixture
def server() -> Iterator[_Server]:
    started = _Server("fresh")
    try:
        yield started
    finally:
        started.close()


def test_python_token_source_refresh() -> None:
    """The required test: one refresh and one retry, on the wire."""
    # The source's first fetch is the seeding one -- an empty cache would send no
    # credential at all, which an instance that requires one answers
    # `unauthenticated`, and the call path reads that as "the token expired" and
    # retries with still no credential. So the sequence is: fetch "stale" to fill
    # the cache, get a `token_expired`, refresh to "fresh", retry, be answered.
    fetch_lock = threading.Lock()
    bearers: list[str] = []
    fetches = 0

    def fetch() -> str:
        nonlocal fetches
        with fetch_lock:
            fetches += 1
            return "stale" if fetches == 1 else "fresh"

    started = _Server("fresh")
    try:
        with Loams(started.url, auth=refreshing(fetch)) as client:
            info = client.instance.get_instance(GetInstanceRequest())
        assert info.name == "Loams"
    finally:
        started.close()

    with fetch_lock:
        bearers = list(started.bearers)
        count = fetches
    assert len(bearers) == 2, f"the server saw {len(bearers)} attempts, want 2: {bearers}"
    assert bearers[0] == "Bearer stale", f"the first attempt sent {bearers[0]!r}"
    assert bearers[1] == "Bearer fresh", f"the retry sent {bearers[1]!r}"
    # Two fetches: one to fill the empty cache, one for the refresh. A third
    # would mean the retry fetched again rather than reading the cache.
    assert count == 2, f"the source was fetched {count} times, want 2"


def test_a_second_expiry_is_reported_rather_than_looped_on() -> None:
    """Refreshing more than once turns an auth outage into a refresh storm."""
    fetch_lock = threading.Lock()
    fetches = 0

    def fetch() -> str:
        nonlocal fetches
        with fetch_lock:
            fetches += 1
            return f"token-{fetches}"

    # Always rejects, so the refresh cannot help and the failure must surface.
    started = _Server("never-matches")
    try:
        with Loams(started.url, auth=refreshing(fetch)) as client:
            with pytest.raises(TokenExpiredError):
                client.instance.get_instance(GetInstanceRequest())
        # One attempt, one refresh, one retry: two attempts, not a loop.
        assert started.attempts == 2, f"the server saw {started.attempts} attempts, want 2"
    finally:
        started.close()
    with fetch_lock:
        count = fetches
    assert count == 2, f"the source was fetched {count} times, want 2: one to seed, one to refresh"


def test_a_rejection_without_a_reason_does_not_refresh() -> None:
    """Without an `ErrorInfo.reason` there is nothing to refresh against."""
    fetch_lock = threading.Lock()
    fetches = 0

    def fetch() -> str:
        nonlocal fetches
        with fetch_lock:
            fetches += 1
            return "always-stale"

    started = _Server("fresh", with_error=False)
    try:
        with Loams(started.url, auth=refreshing(fetch)) as client:
            with pytest.raises(UnauthenticatedError):
                client.instance.get_instance(GetInstanceRequest())
        assert started.attempts == 1, (
            f"the server saw {started.attempts} attempts, want 1: without a reason "
            "there is no refresh to do, so a retry would only replay the same bearer"
        )
    finally:
        started.close()
    with fetch_lock:
        count = fetches
    assert count == 1, f"the source was fetched {count} times, want 1: the seeding fetch only"


def test_one_in_flight_fetch_is_shared_by_concurrent_callers() -> None:
    """A hundred readers must not mint a hundred tokens."""
    fetch_lock = threading.Lock()
    fetches = 0
    release = threading.Event()

    def fetch() -> str:
        nonlocal fetches
        with fetch_lock:
            fetches += 1
        # Hold the exchange open so every other caller piles up behind it, which
        # is the case a lock-per-call implementation gets wrong.
        release.wait(timeout=5)
        return "shared"

    source = refreshing(fetch)
    tokens: list[str | None] = []
    start = threading.Barrier(21)

    def read() -> None:
        start.wait(timeout=5)
        tokens.append(source.token())

    threads = [threading.Thread(target=read) for _ in range(20)]
    for thread in threads:
        thread.start()
    start.wait(timeout=5)
    threading.Timer(0.2, release.set).start()
    for thread in threads:
        thread.join(timeout=10)

    with fetch_lock:
        count = fetches
    assert count == 1, f"the source was fetched {count} times, want 1 for one burst"
    assert tokens == ["shared"] * 20

def test_a_detail_that_already_carries_a_type_url_still_refreshes() -> None:
    """A peer that sends the full type URL must not lose the reason.

    Connect prefixes whatever `type` it is given, so a server or recorder that
    sends `type.googleapis.com/loams.errors.v1.ErrorInfo` arrives as a doubled
    URL. Matching the exact string only finds the bare-name shape, and the
    symptom is quiet: the expiry arrives as a plain `UnauthenticatedError`, the
    reason is dropped, and nothing ever refreshes.
    """
    fetch_lock = threading.Lock()
    fetches = 0

    def fetch() -> str:
        nonlocal fetches
        with fetch_lock:
            fetches += 1
            return "stale" if fetches == 1 else "fresh"

    started = _Server("fresh", type_name=ERROR_INFO_TYPE_URL)
    try:
        with Loams(started.url, auth=refreshing(fetch)) as client:
            assert client.instance.get_instance(GetInstanceRequest()).name == "Loams"
        assert started.attempts == 2, (
            f"the server saw {started.attempts} attempts, want 2: the doubled type URL "
            "must still be recognised as an expiry worth refreshing"
        )
    finally:
        started.close()
