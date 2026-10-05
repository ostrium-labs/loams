"""SDK2 Task 2's `python_stream_resume_with_cursor`.

R7: a server stream hands out cursors, and a reconnect has to resume from the
last one. Resuming from the beginning instead would silently miss everything
that changed while the client was disconnected, which is the failure this exists
to prevent -- so the cursor, not the message count, is what decides where the
reconnect starts.

Two halves, because they fail differently:

* **Over the wire**, against the recorded `live_watch` fixture: the refusal
  arrives *inside* the Connect streaming envelope with an HTTP 200, so an SDK
  that only looks at the status reports a healthy empty stream. That is a real
  recorded case and `live.watch` is bound, so it runs here.
* **Against a stub**, for the reconnect itself. The corpus's three stream-resume
  scenarios and its heartbeat scenario are all `WatchApprovals`, which no SDK
  binds -- `loams.approvals.v1` carries no `loams.options.v1.module` option, so
  the generator emits no binding and there is nothing to call. They are named in
  `test_conformance_corpus.py`'s account of what cannot be run, rather than here:
  `check-languages.mjs` counts fixtures per language by scanning sources for the
  names in `manifest.required`, so mentioning one here would report it as run.
  A fixture for an RPC the SDK cannot call would test the refusal, not the
  resume, so the resume semantics are driven against a stub here and the corpus
  half waits for a bound RPC.

`Transition` carries no `cursor` field, so the reader and the re-open request are
supplied by the caller -- which is the shape `streams.py` documents for exactly
this case.
"""

from __future__ import annotations

import asyncio
import base64
import http.server
import json
import pathlib
import threading

import pytest

from connectrpc.code import Code

from loams import (
    AsyncLoams,
    TokenExpiredError,
    FeatureNotInVariantError,
    Loams,
    LoamsError,
    UnimplementedError,
    UnavailableError,
)
from loams._gen.facade import MODULES
from loams.errors.v1.errors_pb2 import ErrorInfo
from loams.live.v1.live_pb2 import QuerySet, Resume, StateVersion, Transition, WatchRequest
from loams.runtime.streams import ResumeOptions, async_watch, watch
from loams.runtime.token_source import refreshing

WATCH_RPC = "loams.live.v1.LiveService/Watch"


def _watch_binding():
    for module in MODULES:
        if module.name != "live":
            continue
        for call in module.calls:
            if call.name == "watch":
                return call
    raise AssertionError("live.watch is not in the generated facade")


def _version(ts: int) -> StateVersion:
    return StateVersion(query_set=1, identity=1, ts=ts)


def _transition(ts: int) -> Transition:
    """One transition ending at `ts`. The end version is the cursor for live."""
    return Transition(session_id="s1", start=_version(ts - 1), end=_version(ts))


def _encode(cursor: str) -> int:
    """The cursor as a string, standing in for whatever a stream's cursor is."""
    return int(cursor.removeprefix("ts:"))


def _decode(version: StateVersion) -> str:
    return f"ts:{version.ts}"


def _resume_request(cursor: str | None, original: WatchRequest) -> WatchRequest:
    """Re-open at or after the last version, as `WatchRequest.resume` documents."""
    if cursor is None:
        return WatchRequest(initial=QuerySet(version=1))
    return WatchRequest(resume=Resume(last_version=_version(_encode(cursor)), query_set=QuerySet(version=1)))


def test_python_stream_resume_with_cursor() -> None:
    """The required test: a reconnect resumes, it does not restart."""
    binding = _watch_binding()
    reopened: list[str | None] = []

    def open_stream(request: WatchRequest, options: object = None):
        # The first open is a fresh session; the reconnect carries a resume.
        reopened.append(None if request.HasField("resume") else request.initial.version and "fresh")
        if len(reopened) == 1:
            yield _transition(1)
            yield _transition(2)
            yield _transition(3)
            raise UnavailableError("the node went away", code=Code.UNAVAILABLE)
        # The reconnect starts where the client left off, not at the beginning.
        assert request.resume.last_version.ts == 3, (
            f"the reconnect resumed at ts={request.resume.last_version.ts}, want 3"
        )
        yield _transition(4)
        yield _transition(5)

    seen: list[int] = []
    resume = ResumeOptions(
        resume=_resume_request,
        cursor=lambda transition: _decode(transition.end),
        max_retries=3,
        on_cursor=lambda cursor, message: seen.append(_encode(cursor or "")),
    )

    versions = [t.end.ts for t in watch(binding, open_stream, WatchRequest(initial=QuerySet(version=1)), None, resume)]

    # Five transitions, no gap and no repeat: the reconnect resumed at 3 and
    # carried on with 4. Restarting would have replayed 1,2,3 and losing the
    # cursor would have skipped 4 and 5.
    assert versions == [1, 2, 3, 4, 5], f"the stream produced {versions}"
    assert len(reopened) == 2, f"the stream was opened {len(reopened)} times, want 2"


def test_a_message_without_a_cursor_does_not_erase_the_remembered_one() -> None:
    """A heartbeat carries no cursor and must not reset the position to none."""
    binding = _watch_binding()
    reopened: list[WatchRequest] = []

    def open_stream(request: WatchRequest, options: object = None):
        reopened.append(request)
        if len(reopened) == 1:
            yield _transition(1)
            # A heartbeat: a Transition that repeats the current version and so
            # has nothing new, which is what keeps an idle watch alive.
            yield _transition(1)
            raise UnavailableError("the node went away", code=Code.UNAVAILABLE)
        yield _transition(2)

    resume = ResumeOptions(
        resume=_resume_request,
        cursor=lambda transition: _decode(transition.end),
        max_retries=2,
    )
    versions = [
        t.end.ts
        for t in watch(binding, open_stream, WatchRequest(initial=QuerySet(version=1)), None, resume)
    ]
    assert versions == [1, 1, 2]
    assert reopened[1].resume.last_version.ts == 1, (
        f"the reconnect resumed at ts={reopened[1].resume.last_version.ts}, want 1"
    )


def test_a_failure_a_retry_cannot_answer_is_reported_not_spun_on() -> None:
    """An `unimplemented` stream is not a disconnect; retrying it loops forever."""
    binding = _watch_binding()
    opens = 0

    def open_stream(request: WatchRequest, options: object = None):
        nonlocal opens
        opens += 1
        raise UnimplementedError(
            "Watch is not implemented in this variant yet", code=Code.UNIMPLEMENTED
        )
        yield  # pragma: no cover - unreachable, keeps this a generator

    resume = ResumeOptions(resume=_resume_request, max_retries=5)
    with pytest.raises(UnimplementedError):
        list(watch(binding, open_stream, WatchRequest(initial=QuerySet(version=1)), None, resume))
    assert opens == 1, f"an unimplemented stream was opened {opens} times, want 1"


def test_without_a_resume_policy_the_stream_reopens_from_the_beginning() -> None:
    """The honest default: it may repeat rather than silently skipping.

    Guessing a cursor the caller never supplied would skip changes with nothing
    to show for it, so the default is to re-open and let the caller's de-dup
    notice the repeat.
    """
    binding = _watch_binding()
    requests: list[WatchRequest] = []

    def open_stream(request: WatchRequest, options: object = None):
        requests.append(request)
        if len(requests) == 1:
            yield _transition(1)
            raise UnavailableError("the node went away", code=Code.UNAVAILABLE)
        yield _transition(2)

    versions = [
        t.end.ts for t in watch(binding, open_stream, WatchRequest(initial=QuerySet(version=1)))
    ]
    assert versions == [1, 2]
    assert len(requests) == 2
    assert requests[1].HasField("initial"), "the default re-opened with a resume cursor"
    assert requests[1].initial.version == 1, "and from the caller's original query set"


class _EnvelopeServer:
    """Serves one Connect end-of-stream error frame, as `loams dev` does.

    The refusal is inside the envelope behind an HTTP 200, which is the whole
    point: an SDK that reports the status code reports a healthy empty watch.
    """

    def __init__(self, reason: str) -> None:
        info = ErrorInfo(reason=reason)
        detail = json.dumps(
            {
                "type": "loams.errors.v1.ErrorInfo",
                "value": base64.b64encode(info.SerializeToString()).decode("ascii"),
            }
        )
        payload = json.dumps(
            {
                "error": {
                    "code": "unimplemented",
                    "message": "loams.live.v1.LiveService/Watch is not in the standard variant",
                    "details": [json.loads(detail)],
                }
            }
        ).encode()
        frame = bytes([0x02]) + len(payload).to_bytes(4, "big") + payload
        outer = self

        class Handler(http.server.BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.1"

            def log_message(self, *args: object) -> None:
                """Keep the test output clean."""

            def do_POST(self) -> None:
                self.rfile.read(int(self.headers.get("content-length", "0")))
                self.send_response(200)
                self.send_header("content-type", "application/connect+json")
                self.send_header("content-length", str(len(frame)))
                self.end_headers()
                self.wfile.write(frame)

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


def test_a_stream_refusal_is_read_out_of_the_envelope() -> None:
    """HTTP 200, and the failure is inside the end-of-stream frame.

    R7 and R8 on a stream: the refusal has to arrive as the reason the server
    gave, not as an empty stream and not as a bare `InternalError`. This is the
    behaviour `live_watch` exists to pin against the corpus.
    """
    started = _EnvelopeServer("feature_not_in_variant")
    try:
        # `proto_json` because the frame is Connect JSON. The SDK's default is
        # the binary encoding, which is correct -- see the note on the recorded
        # case below for why the corpus cannot serve that here.
        with Loams(started.url, proto_json=True) as client:
            with pytest.raises(FeatureNotInVariantError) as caught:
                list(client.live.watch(WatchRequest(initial=QuerySet(version=1))))
    finally:
        started.close()
    assert caught.value.reason == "feature_not_in_variant", (
        f"the reason came back as {caught.value.reason!r}"
    )
    assert caught.value.hint is None or isinstance(caught.value.hint, str)


def test_the_recorded_watch_case_replays_and_reports_the_refusal() -> None:
    """The corpus half, which used to be an xfail on a premise that turned out false.

    This test asserted that `live_watch` could not be replayed, on the reasoning
    that connect-python "cannot read this recording in any configuration" -- asking
    for proto and getting the `+proto` label meant it parsed JSON as proto and
    failed. Replaying the recorded bytes directly says otherwise:

        messages: 0 | error: FeatureNotInVariantError: ... not in the standard variant

    The recording is a valid Connect stream: one empty data frame, then a
    JSON-encoded end-of-stream frame, under `application/connect+proto`. The
    Connect protocol defines that end frame as JSON *whatever the codec is*, and
    connect-python implements it that way. `verify-corpus.mjs` was flagging the
    same recording, because its encoder check judged every frame against the
    codec label; both are corrected, and this is now an assertion rather than a
    standing excuse.

    It is worth being explicit about how that went wrong, because the failure mode
    was not a bug report -- it was a plausible story. The observation that
    connect-go reads the bytes happily sat next to "connect-python cannot", and
    the tidy conclusion was that connect-go sniffs where connect-python is strict,
    so the recording must be at fault. Both halves of that were unverified. The
    cheap check nobody ran was: serve the exact bytes at the exact content-type and
    see what the client does.
    """
    root = pathlib.Path(__file__).resolve().parent.parent.parent
    recorded = json.loads((root / "fixtures" / "recorded" / "live_watch.json").read_text())
    response_type = recorded["response"]["headers"]["content-type"]
    body = base64.b64decode(recorded["response"]["bodyBase64"])

    assert response_type == "application/connect+proto", (
        f"the recording changed to {response_type!r}; the reason a proto client "
        "reads is the JSON end frame the protocol requires under that label"
    )

    # The refusal is still in there, and still says what it is for.
    payload = json.loads(body[5:].decode("utf-8"))
    assert "error" in payload, "the recorded frame no longer carries a refusal"
    assert payload["error"]["code"] == "unimplemented", (
        f"the recorded refusal changed to {payload['error']['code']!r}"
    )

    class Handler(http.server.BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def log_message(self, *args: object) -> None:
            """Keep the test output clean."""

        def do_POST(self) -> None:
            self.close_connection = True
            self.rfile.read(int(self.headers.get("content-length", "0")))
            self.send_response(200)
            self.send_header("content-type", response_type)
            self.send_header("content-length", str(len(body)))
            self.send_header("connection", "close")
            self.end_headers()
            self.wfile.write(body)

    httpd = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    url = f"http://127.0.0.1:{httpd.server_address[1]}"
    # Binding is not serving: without this the socket accepts into the backlog and
    # every request hangs instead of erroring.
    serving = threading.Thread(target=httpd.serve_forever, daemon=True)
    serving.start()
    try:
        with Loams(url) as client:
            with pytest.raises(FeatureNotInVariantError) as caught:
                list(
                    client.live.watch(WatchRequest(initial=QuerySet(version=1)))
                )
    finally:
        httpd.shutdown()
        httpd.server_close()
        serving.join(timeout=5)

    assert caught.value.reason == "feature_not_in_variant", (
        f"the reason came back as {caught.value.reason!r}, so the recorded "
        "refusal is not surviving the round trip"
    )


def test_the_async_stream_resumes_rather_than_restarts() -> None:
    """`async_watch` carries the cursor across a reconnect, exactly as `watch` does.

    The async path is a separate implementation, not a wrapper, so it needs its
    own proof: if it re-opened from the beginning the client would replay 1, 2, 3
    and if it lost the cursor it would skip 4 and 5.
    """
    binding = _watch_binding()
    reopened: list[str | None] = []

    async def open_stream(request: WatchRequest, options: object = None):
        reopened.append(None if request.HasField("resume") else "fresh")
        if len(reopened) == 1:
            yield _transition(1)
            yield _transition(2)
            yield _transition(3)
            raise UnavailableError("the node went away", code=Code.UNAVAILABLE)
        assert request.resume.last_version.ts == 3, (
            f"the reconnect resumed at ts={request.resume.last_version.ts}, want 3"
        )
        yield _transition(4)
        yield _transition(5)

    resume = ResumeOptions(resume=_resume_request, cursor=lambda t: _decode(t.end), max_retries=3)

    async def walk():
        return [
            t.end.ts
            async for t in async_watch(
                binding, open_stream, WatchRequest(initial=QuerySet(version=1)), None, resume
            )
        ]

    versions = asyncio.run(walk())
    assert versions == [1, 2, 3, 4, 5], f"the async stream produced {versions}"
    assert len(reopened) == 2, f"the async stream was opened {len(reopened)} times, want 2"


def test_a_heartbeat_does_not_erase_the_cursor_on_the_async_path() -> None:
    """A repeated version is not a cursor, and must not reset the reconnect."""
    binding = _watch_binding()
    reopened: list[WatchRequest] = []

    async def open_stream(request: WatchRequest, options: object = None):
        reopened.append(request)
        if len(reopened) == 1:
            yield _transition(1)
            yield _transition(1)
            raise UnavailableError("the node went away", code=Code.UNAVAILABLE)
        yield _transition(2)

    resume = ResumeOptions(resume=_resume_request, cursor=lambda t: _decode(t.end), max_retries=2)

    async def walk():
        return [
            t.end.ts
            async for t in async_watch(
                binding, open_stream, WatchRequest(initial=QuerySet(version=1)), None, resume
            )
        ]

    assert asyncio.run(walk()) == [1, 1, 2]
    assert reopened[1].resume.last_version.ts == 1, (
        f"the async reconnect resumed at ts={reopened[1].resume.last_version.ts}, want 1"
    )


def test_an_unimplemented_async_stream_is_reported_not_spun_on() -> None:
    """`unimplemented` is not a disconnect, so retrying it would loop forever."""
    binding = _watch_binding()
    opens = 0

    async def open_stream(request: WatchRequest, options: object = None):
        nonlocal opens
        opens += 1
        raise UnimplementedError(
            "Watch is not implemented in this variant yet", code=Code.UNIMPLEMENTED
        )
        yield  # pragma: no cover - unreachable, keeps this an async generator

    resume = ResumeOptions(resume=_resume_request, max_retries=5)

    async def walk():
        return [
            t
            async for t in async_watch(
                binding, open_stream, WatchRequest(initial=QuerySet(version=1)), None, resume
            )
        ]

    with pytest.raises(UnimplementedError):
        asyncio.run(walk())
    assert opens == 1, f"an unimplemented async stream was opened {opens} times, want 1"


def test_an_async_stream_refusal_is_read_out_of_the_envelope() -> None:
    """End to end through `AsyncLoams.stream`, over a real connection.

    This is the only test that runs the wiring the sync one cannot: the async
    facade lookup, the generated async module method, connect-python's async
    streaming client and `async_watch`, all against HTTP 200 with the refusal
    inside the envelope. `async_watch` on a stub proves the cursor logic; this
    proves the bytes still arrive.
    """
    started = _EnvelopeServer("feature_not_in_variant")

    async def walk() -> None:
        try:
            async with AsyncLoams(started.url, proto_json=True) as client:
                async for _ in client.live.watch(WatchRequest(initial=QuerySet(version=1))):
                    pass
        finally:
            started.close()

    with pytest.raises(FeatureNotInVariantError) as caught:
        asyncio.run(walk())
    assert caught.value.reason == "feature_not_in_variant", (
        f"the reason came back as {caught.value.reason!r}"
    )


def test_async_loams_stream_reaches_the_same_failure() -> None:
    """`AsyncLoams.stream(...)` is a third door into the same machinery.

    The module call (`client.live.watch`) and the generic `stream` are separate
    wirings into `async_watch`, and the coroutine bug took out both. This one
    takes explicit `resume_options`, so it is the shape a caller who cares about
    the cursor would reach for.
    """
    started = _EnvelopeServer("feature_not_in_variant")

    async def walk() -> None:
        try:
            async with AsyncLoams(started.url, proto_json=True) as client:
                async for _ in client.stream(
                    "live",
                    "watch",
                    WatchRequest(initial=QuerySet(version=1)),
                    resume_options=ResumeOptions(resume=_resume_request, cursor=lambda t: _decode(t.end)),
                ):
                    pass
        finally:
            started.close()

    with pytest.raises(FeatureNotInVariantError) as caught:
        asyncio.run(walk())
    assert caught.value.reason == "feature_not_in_variant"


def test_a_binding_error_is_raised_when_the_stream_is_opened_not_iterated() -> None:
    """The eager half stays eager: a bad call fails at the call, as it does sync.

    Only the token resolution is deferred, because it is the one thing that must
    be awaited. If the whole body moved into the generator, `client.stream` on a
    module that does not exist would return quietly and fail on first iteration,
    which is a worse error to get and a difference from the sync client.
    """
    async def open_bad_module() -> None:
        async with AsyncLoams("http://127.0.0.1:1") as client:
            with pytest.raises(LoamsError) as caught:
                client.stream("nosuchmodule", "watch", WatchRequest(initial=QuerySet(version=1)))
            assert "nosuchmodule" in str(caught.value)

    asyncio.run(open_bad_module())


# -- a stream that outlives its token ------------------------------------------


class _ExpiringStreamServer:
    """Rejects a stale bearer once, then streams a message.

    A watch is the one call a client holds open long enough for its credential to
    expire underneath it, so it is the call where "refresh and reconnect" has to
    work. `_mapped`/`_async_mapped` have to notice the refusal *before* the caller
    has seen a message -- so there is nothing to yield yet and nothing to resume
    from.

    The refusal is an HTTP 200 with the error inside an end-stream frame, because
    that is how Connect reports a streaming error and there is no other way to
    report one. Two wrong shapes got tried first, and both failed *silently*
    rather than loudly, which is the note worth keeping:

    - 401 with a framed end-stream: connect-python does not read frames from a
      non-2xx, so it decoded a bare `UnauthenticatedError` with no reason -- which
      the runtime correctly refuses to refresh against.
    - 401 with `content-type: application/json`: that is the *unary* JSON content
      type, so the streaming client treated the body as one unary response, found
      no `Transition` in it, and returned an empty stream. No exception, no retry,
      one request. A watch that silently ends looks exactly like a healthy watch
      that saw no changes.

    So: 200, `application/connect+json`, and the end-stream payload is the error
    document wrapped as `{"error": {...}}` -- which is a different shape from the
    unwrapped `{"code": ..., "details": [...]}` a unary 401 uses.
    """

    def __init__(self, expected: str) -> None:
        self.expected = expected
        self.bearers: list[str] = []
        self._lock = threading.Lock()

        info = ErrorInfo(reason="token_expired", hint="refresh and try again")
        encoded = base64.b64encode(info.SerializeToString()).decode("ascii")
        refusal = json.dumps(
            {
                "error": {
                    "code": "unauthenticated",
                    "message": "the access token expired",
                    "details": [{"type": "loams.errors.v1.ErrorInfo", "value": encoded}],
                }
            }
        ).encode()
        refusal_frame = bytes([0x02]) + len(refusal).to_bytes(4, "big") + refusal

        message = _transition(1).SerializeToString()
        message_frame = bytes([0x00]) + len(message).to_bytes(4, "big") + message
        # The end-of-stream frame carries the end-stream JSON document, which in
        # Connect is at least `{}`. A zero-length payload here parses as nothing
        # and the stream comes back *empty* rather than raising, which reads like
        # a client bug rather than a malformed frame -- the first version of this
        # test asserted [1] and got [].
        end_payload = b"{}"
        end_frame = bytes([0x02]) + len(end_payload).to_bytes(4, "big") + end_payload

        outer = self

        class Handler(http.server.BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.1"

            def log_message(self, *args: object) -> None:
                """Keep the test output clean."""

            def do_POST(self) -> None:
                # One request per connection. This test is the only one here that
                # gets a *second* request on the same socket -- the refresh-then-
                # reconnect -- and with keep-alive the reconnect races the tail of
                # the error response, which surfaced as a bare
                # `InternalError: Bad Request` about one run in six. Closing after
                # each response costs nothing at this size and removes the race.
                self.close_connection = True
                self.rfile.read(int(self.headers.get("content-length", "0")))
                bearer = self.headers.get("authorization", "")
                with outer._lock:
                    outer.bearers.append(bearer)
                if bearer != f"Bearer {outer.expected}":
                    # The label must match the codec the client asked for, or
                    # connect-python rejects it with `invalid content-type ...
                    # expecting 'application/connect+proto'` before the reason is
                    # ever read. The end-stream *payload* stays JSON either way:
                    # the Connect protocol defines that frame as JSON regardless
                    # of codec, which is why `_EnvelopeServer` can label a JSON
                    # payload `+json` and this one labels the same shape `+proto`.
                    self.send_response(200)
                    self.send_header("content-type", "application/connect+proto")
                    self.send_header("content-length", str(len(refusal_frame)))
                    self.send_header("connection", "close")
                    self.end_headers()
                    self.wfile.write(refusal_frame)
                    return
                body = message_frame + end_frame
                self.send_response(200)
                self.send_header("content-type", "application/connect+proto")
                self.send_header("content-length", str(len(body)))
                self.send_header("connection", "close")
                self.end_headers()
                self.wfile.write(body)

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
    def attempts(self) -> int:
        with self._lock:
            return len(self.bearers)


def _expiring_fetch() -> "tuple[list[str], object]":
    """A source that seeds `stale` then refreshes to `fresh`, counting fetches."""
    lock = threading.Lock()
    fetches: list[str] = []

    def fetch() -> str:
        with lock:
            value = "stale" if not fetches else "fresh"
            fetches.append(value)
            return value

    return fetches, refreshing(fetch)


def test_a_stream_refreshes_its_token_and_reconnects() -> None:
    """One refresh, one reconnect, and the caller sees the message either way.

    Without this, a watch opened on a token that expires mid-flight simply dies:
    the reconnect is inside `_mapped`, and a bug there is invisible until a
    credential outlives a long-poll.
    """
    started = _ExpiringStreamServer("fresh")
    fetches, source = _expiring_fetch()
    try:
        with Loams(started.url, auth=source) as client:
            versions = [t.end.ts for t in client.live.watch(WatchRequest(initial=QuerySet(version=1)))]
    finally:
        started.close()

    assert versions == [1], f"the stream produced {versions}"
    assert started.attempts == 2, (
        f"the server saw {started.attempts} attempts, want 2: {started.bearers}"
    )
    assert started.bearers[0] == "Bearer stale", f"the first attempt sent {started.bearers[0]!r}"
    assert started.bearers[1] == "Bearer fresh", f"the reconnect sent {started.bearers[1]!r}"
    # Two fetches: one seeding the empty cache, one for the refresh. A third would
    # mean the reconnect fetched again instead of reading the refreshed cache.
    assert fetches == ["stale", "fresh"], f"the source was fetched {fetches}"


def test_an_async_stream_refreshes_its_token_and_reconnects() -> None:
    """The async half, which is a separate implementation of the same decision.

    `_async_mapped` has its own `refreshed` flag and its own `await refresh()`; a
    bug in it would leave the async watch broken while the sync one recovered.
    """
    from loams import async_refreshing

    started = _ExpiringStreamServer("fresh")
    lock = threading.Lock()
    fetches: list[str] = []

    async def fetch() -> str:
        with lock:
            value = "stale" if not fetches else "fresh"
            fetches.append(value)
            return value

    async def walk() -> list[int]:
        async with AsyncLoams(started.url, auth=async_refreshing(fetch)) as client:
            return [
                t.end.ts
                async for t in client.live.watch(WatchRequest(initial=QuerySet(version=1)))
            ]

    try:
        versions = asyncio.run(walk())
    finally:
        started.close()

    assert versions == [1], f"the async stream produced {versions}"
    assert started.attempts == 2, (
        f"the server saw {started.attempts} async attempts, want 2: {started.bearers}"
    )
    assert started.bearers == ["Bearer stale", "Bearer fresh"], (
        f"the async stream sent {started.bearers}"
    )
    assert fetches == ["stale", "fresh"], f"the async source was fetched {fetches}"


def test_a_stream_refreshes_at_most_once() -> None:
    """A second expiry is reported. Refreshing again turns an auth outage into a
    refresh storm, and the watch would never surface the failure at all."""
    started = _ExpiringStreamServer("never-matches")
    fetches, source = _expiring_fetch()
    try:
        with Loams(started.url, auth=source) as client:
            with pytest.raises(TokenExpiredError):
                list(client.live.watch(WatchRequest(initial=QuerySet(version=1))))
    finally:
        started.close()

    assert started.attempts == 2, (
        f"the server saw {started.attempts} attempts, want 2, not a loop: {started.bearers}"
    )
    assert len(fetches) == 2, f"the source was fetched {len(fetches)} times, want 2"


def test_the_sync_client_stream_reads_a_message() -> None:
    """`Loams.stream(module, call, request)` -- the generic door -- on the sync side.

    `client.live.watch(...)` is the other door, and it is covered. This one was
    not called at all: the sync generic path had only its *refusals* tested, which
    is the shape of gap that leaves the success path unexecuted -- the same shape
    as `AsyncLoams.stream` returning a coroutine, which the async refusals never
    could have caught.
    """
    # The key matches straight away, so this exercises the generic wiring rather
    # than the refresh path (which has its own tests above).
    started = _ExpiringStreamServer("fresh")
    try:
        with Loams(started.url, api_key="fresh") as client:
            versions = [
                t.end.ts
                for t in client.stream(
                    "live", "watch", WatchRequest(initial=QuerySet(version=1))
                )
            ]
    finally:
        started.close()
    assert versions == [1], f"the sync generic stream produced {versions}"


def test_a_resume_with_no_cursor_restarts_from_the_original_request() -> None:
    """`ResumeOptions(resume=...)` with no `cursor` cannot build a resume field.

    `WatchRequest.resume` is a message, not a string, so a cursor has to be packed
    into it. With no `cursor=` given, `cursor_field` is asked for a `cursor` field
    that `Transition` does not have, so there is nothing to resume from and the
    reconnect replays the original request. That is the documented fallback, and
    it is the *only* correct answer: sending a resume with an empty cursor would
    tell the server to resume from nothing, which reads as "from the beginning of
    time" or as a malformed request depending on the server.
    """
    binding = _watch_binding()
    requests: list[WatchRequest] = []

    def open_stream(request: WatchRequest, options: object = None):
        requests.append(request)
        if len(requests) == 1:
            yield _transition(1)
            raise UnavailableError("the node went away", code=Code.UNAVAILABLE)
        yield _transition(2)

    # A resume callable, but no cursor: the default cursor lookup finds nothing.
    resume = ResumeOptions(
        resume=lambda _cursor, original: original,
        max_retries=3,
    )

    versions = [t.end.ts for t in watch(binding, open_stream, WatchRequest(initial=QuerySet(version=1)), None, resume)]

    assert versions == [1, 2], f"the stream produced {versions}"
    assert len(requests) == 2, f"the stream was opened {len(requests)} times, want 2"
    assert not requests[1].HasField("resume"), (
        "a resume was sent with no cursor, which the server cannot act on"
    )


def test_on_cursor_sees_every_message_and_the_latest_value() -> None:
    """`on_cursor` is how a caller learns where to resume from, so it has to fire
    per message and carry the running value -- not just the last one seen."""
    binding = _watch_binding()
    seen: list[tuple[str | None, int]] = []

    resume = ResumeOptions(
        resume=_resume_request,
        cursor=lambda t: _decode(t.end),
        on_cursor=lambda cursor, message: seen.append((cursor, message.end.ts)),
    )

    versions = [
        t.end.ts
        for t in watch(
            binding,
            lambda request, options=None: iter([_transition(1), _transition(2), _transition(3)]),
            WatchRequest(initial=QuerySet(version=1)),
            None,
            resume,
        )
    ]

    assert versions == [1, 2, 3]
    assert [ts for _, ts in seen] == [1, 2, 3], (
        f"on_cursor saw {[ts for _, ts in seen]}, want one call per message"
    )
    assert seen[0][0] is not None, "the first on_cursor call carried no cursor"


def test_resume_options_can_declare_a_call_unsafe() -> None:
    """`retry_safe=False` on a stream means a disconnect is the caller's problem.

    `retry_safe` defaults to True for a stream because re-opening one is
    idempotent from the SDK's point of view, but a caller who has side effects
    attached to the cursor wants the failure surfaced instead.
    """
    binding = _watch_binding()
    opens = 0

    def open_stream(request: WatchRequest, options: object = None):
        nonlocal opens
        opens += 1
        raise UnavailableError("the node went away", code=Code.UNAVAILABLE)
        yield  # pragma: no cover - unreachable, keeps this a generator

    resume = ResumeOptions(resume=_resume_request, retry_safe=False, max_retries=5)

    with pytest.raises(UnavailableError):
        list(
            watch(
                binding,
                open_stream,
                WatchRequest(initial=QuerySet(version=1)),
                None,
                resume,
            )
        )
    assert opens == 1, (
        f"a stream declared unsafe was re-opened {opens} times, want 1: the "
        "disconnect should have been reported, not retried"
    )


def test_a_module_and_its_repr_name_themselves() -> None:
    """`repr(loams.live)` is what a debugger shows, so it has to say which module
    and which service -- two modules over the same service otherwise look alike."""
    with Loams("http://127.0.0.1:1") as client:
        text = repr(client.live)
    assert "live" in text and "LiveService" in text, f"the module repr was {text!r}"
