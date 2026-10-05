"""The call path: the one place a facade method becomes an RPC (design §44 §7.4;
runtime contract R1–R4).

It does four things the generated facade cannot, and nothing else:

- attaches the bearer from the client's token source;
- retries on the call's class **from the generated bindings**, with M1.6's
  backoff numbers, and refreshes the token once on `token_expired`;
- gives a mutating call an idempotency key once per logical call and reuses it
  on every retry, so a retried write is the same write (D610);
- turns whatever is thrown into the typed `LoamsError` hierarchy, so a caller
  branches on `reason` and never on a message.

Both clients share it: `CallInvoker` is the sync path and `AsyncCallInvoker` the
awaitable one, and the retry loop, the key lifecycle and the error mapping are
written once as free functions so the two cannot drift on the parts R1–R4 pin.
"""

from __future__ import annotations

from collections.abc import (
    AsyncIterator,
    Awaitable,
    Callable,
    Iterator,
    Mapping,
    MutableMapping,
)
from typing import Any, TypeVar

from connectrpc.client import ConnectClient, ConnectClientSync
from connectrpc.code import Code
from connectrpc.method import MethodInfo
from google.protobuf.message import Message

from loams._gen.facade import IDEMPOTENCY_LEVELS, METHODS, CallBinding, Pagination
from loams.runtime.errors import LoamsError, TokenExpiredError, to_loams_error
from loams.runtime.options import CallOptions, ConsistencyOptions, ConsistencyTokenStore
from loams.runtime.retry import backoff_seconds, should_retry, sleep
from loams.runtime.token_source import (
    AsyncTokenSource,
    TokenSource,
    async_refresh_of,
    refresh_of,
)
from loams.runtime.uuidv7 import uuidv7

__all__ = [
    "AsyncCallInvoker",
    "CallInvoker",
    "apply_idempotency_key",
    "call_headers",
    "call_with_retry",
    "async_call_with_retry",
    "record_consistency",
    "resolve_session",
]

#: The request-message field a mutation is keyed by (D610). Read from the
#: generated message's descriptor rather than guessed from the object a caller
#: built, so a caller who omits an `optional` field still gets a key and
#: `MutateRequest` (which has one) is never confused with a request that has
#: none.
IDEMPOTENCY_KEY = "idempotency_key"

REQ = TypeVar("REQ", bound=Message)
RES = TypeVar("RES", bound=Message)


# ---------------------------------------------------------------------------
# The idempotency key (R3).
# ---------------------------------------------------------------------------


def _declares_key(request: Message) -> bool:
    """Whether the request message's *schema* declares `idempotency_key`."""
    return IDEMPOTENCY_KEY in {field.name for field in request.DESCRIPTOR.fields}


def apply_idempotency_key(
    request: Message, supplied: str | None = None
) -> tuple[Message, bool]:
    """Gives a mutating call its idempotency key, once per logical call.

    D610: a mutation may only be retried when it carries one, and the same key
    has to go out on every attempt or the retry is a second write. So the key is
    decided here, before the first attempt, and the request is copied rather
    than rewritten afterwards.

    :returns: the keyed request, and whether this call is keyed at all. A
        message without an `idempotency_key` field is left exactly as the caller
        wrote it.
    """
    if not _declares_key(request):
        return request, False
    existing = getattr(request, IDEMPOTENCY_KEY, "")
    if isinstance(existing, str) and existing != "":
        # A key the caller supplied is theirs; it is not replaced.
        return request, True
    keyed = type(request)()
    keyed.CopyFrom(request)
    setattr(keyed, IDEMPOTENCY_KEY, supplied if supplied else uuidv7())
    return keyed, True


# ---------------------------------------------------------------------------
# Headers (R1, R4).
# ---------------------------------------------------------------------------


def call_headers(
    options: CallOptions,
    bearer: str | None,
    consistency: str | None,
) -> dict[str, str]:
    """The headers a call carries: the caller's, plus the SDK's own.

    `Authorization` is set last so a header a caller passed cannot displace the
    bearer, and the token never appears in the query string (R1).
    """
    headers: MutableMapping[str, str] = {}
    if options.headers is not None:
        headers.update(options.headers)
    if bearer is not None:
        headers["authorization"] = f"Bearer {bearer}"
    if consistency is not None:
        headers["loams-consistency"] = consistency
    return dict(headers)


def resolve_session(
    options: ConsistencyOptions | None, client_session: ConsistencyTokenStore | None
) -> ConsistencyTokenStore | None:
    """The session a call asked for.

    Its own store if it brought one, the client's if it asked for the session's,
    none otherwise. Off by default (D609).

    A call that expresses no preference takes the client's session, because that is
    what `Loams(session_consistency=True)` means: the token is held *across calls*.
    It used to return `None` here, which made the constructor flag inert -- every
    call had to repeat `ConsistencyOptions(session=True)` or it joined no session at
    all, and no error said so. `ConsistencyOptions(session=False)` is still how a
    single call opts out.
    """
    if options is None:
        return client_session
    chosen = options.session
    if chosen is False:
        return None
    if chosen is True:
        return client_session
    return chosen


# ---------------------------------------------------------------------------
# The retry loop (R1, R2).
# ---------------------------------------------------------------------------


def _retry_decision(error: LoamsError, retry_safe: bool, attempt: int, max_retries: int) -> bool:
    return should_retry(error.code, retry_safe, attempt, max_retries)


def call_with_retry(
    send: Callable[[], Message],
    *,
    rpc: str,
    retry_safe: bool,
    max_retries: int,
    on_refresh: Callable[[], None] | None,
) -> Message:
    """Runs `send` until it answers or the policy says stop.

    :raises LoamsError: the last failure, mapped. R1: a 401 whose reason is
        `token_expired` gets exactly one refresh and one retry. A source that
        cannot refresh (`api_key`) makes the refresh a no-op, and a second expiry
        is reported rather than looped on.
    """
    refreshed = False
    attempt = 0
    while True:
        try:
            return send()
        except BaseException as thrown:  # noqa: BLE001 - re-raised as a LoamsError
            error = to_loams_error(thrown, rpc)
            if (
                isinstance(error, TokenExpiredError)
                and not refreshed
                and on_refresh is not None
            ):
                refreshed = True
                on_refresh()
                continue
            if not _retry_decision(error, retry_safe, attempt, max_retries):
                raise error from None
            sleep(backoff_seconds(attempt))
            attempt += 1


async def async_call_with_retry(
    send: Callable[[], Awaitable[Message]],
    *,
    rpc: str,
    retry_safe: bool,
    max_retries: int,
    on_refresh: Callable[[], Awaitable[None]] | None,
) -> Message:
    """`call_with_retry` for `AsyncCallInvoker`, with the same decisions."""
    refreshed = False
    attempt = 0
    while True:
        try:
            return await send()
        except BaseException as thrown:  # noqa: BLE001 - re-raised as a LoamsError
            error = to_loams_error(thrown, rpc)
            if (
                isinstance(error, TokenExpiredError)
                and not refreshed
                and on_refresh is not None
            ):
                refreshed = True
                await on_refresh()
                continue
            if not _retry_decision(error, retry_safe, attempt, max_retries):
                raise error from None
            await _async_sleep(backoff_seconds(attempt))
            attempt += 1


async def _async_sleep(seconds: float) -> None:
    """Sleeps without blocking the event loop."""
    import asyncio

    if seconds > 0:
        await asyncio.sleep(seconds)


# ---------------------------------------------------------------------------
# Recording a write's consistency token (R4).
# ---------------------------------------------------------------------------


def record_consistency(session: ConsistencyTokenStore | None, response: object) -> None:
    """Merges the consistency token a write answered with into the session.

    A token the session cannot merge must not turn a committed write into a
    thrown error, because a caller that retries on that error performs the write
    twice. `ConsistencySession.conflicts` counts it instead.

    This reads the response **message** field, which is one of the two places
    D609 puts a token. The other is the response header `loams-consistency-token`
    -- the only one that currently carries anything, since no generated message
    has a `consistency_token` field -- and `connect-python==0.9.0` has no call
    that returns response headers, so this function never sees it. Which is why
    a `ConsistencySession` stays empty in practice and `session_consistency=True`
    is inert rather than merely conservative. The Rust SDK reads the header
    (`token_from_headers` in `sdks/rust/src/request.rs`); closing this needs
    connect-python to expose headers, not a change here.
    """
    if session is None or response is None:
        return
    token = getattr(response, "consistency_token", None)
    if not isinstance(token, str) or token == "":
        return
    try:
        session.record(token)
    except LoamsError:
        return


# ---------------------------------------------------------------------------
# The invokers.
# ---------------------------------------------------------------------------


def _method_of(binding: CallBinding) -> MethodInfo[Message, Message]:
    """The `MethodInfo` a binding names, from the generated table."""
    found = METHODS.get(binding.rpc)
    if found is None:
        raise LoamsError(
            f"the generated table has no method for {binding.rpc}",
            code=Code.INTERNAL,
            rpc=binding.rpc,
        )
    return found


def _idempotency_level(binding: CallBinding) -> object:
    return IDEMPOTENCY_LEVELS[binding.idempotency]


class _BaseInvoker:
    """What the two invokers share: the client, the source and the session."""

    def __init__(
        self,
        client: object,
        max_retries: int,
        consistency: ConsistencyTokenStore | None,
    ) -> None:
        self._client = client
        self._max_retries = max_retries
        self._consistency = consistency

    @property
    def consistency(self) -> ConsistencyTokenStore | None:
        """The session store, when `session_consistency` is on."""
        return self._consistency

    def _plan(
        self, binding: CallBinding, options: CallOptions, request: Message
    ) -> tuple[Message, bool, int, ConsistencyTokenStore | None, str | None]:
        """Everything both paths decide the same way before an attempt."""
        keyed, has_key = apply_idempotency_key(request, options.idempotency_key)
        max_retries = options.max_retries if options.max_retries is not None else self._max_retries
        # D610: a `safe` call always retries; a mutation retries once it carries
        # an idempotency key, which `apply_idempotency_key` has just decided.
        retry_safe = options.retry_safe if options.retry_safe is not None else (
            binding.retry == "safe" or has_key
        )
        session = resolve_session(options.consistency, self._consistency)
        # An explicit token wins; otherwise the session's, which is what makes
        # `consistency=ConsistencyOptions(session=True)` read-your-writes rather
        # than record it and never send it (R4).
        consistency = options.consistency.token if options.consistency is not None else None
        if consistency is None and session is not None:
            consistency = session.current()
        return keyed, retry_safe, max_retries, session, consistency

    def binding_for(self, module: str, call: str) -> CallBinding:
        """The binding a module and call name identify."""
        from loams._gen.facade import MODULES

        for entry in MODULES:
            if entry.name != module:
                continue
            for candidate in entry.calls:
                if candidate.name == call:
                    return candidate
        raise LoamsError(
            f"loams.{module} has no generated call {call}", code=Code.INTERNAL
        )

    def pagination_of(self, binding: CallBinding) -> Pagination:
        """The pagination a binding names, or a clear error when it names none."""
        if binding.pagination is None:
            raise LoamsError(
                f"loams.{binding.module}.{binding.name} is not a paged call: "
                "the proto's facade options name no pagination",
                code=Code.INVALID_ARGUMENT,
                rpc=binding.rpc,
            )
        return binding.pagination


class CallInvoker(_BaseInvoker):
    """The sync call path: a generated binding plus a request becomes a response."""

    def __init__(
        self,
        client: ConnectClientSync,
        token_source: TokenSource | None,
        max_retries: int,
        consistency: ConsistencyTokenStore | None,
    ) -> None:
        super().__init__(client, max_retries, consistency)
        self._token_source = token_source
        self._refresh = refresh_of(token_source) if token_source is not None else None

    def unary(self, binding: CallBinding, request: Message, options: CallOptions | None = None) -> Message:
        """One unary call: the whole runtime contract, applied to one binding."""
        call_options = options or CallOptions()
        keyed, retry_safe, max_retries, session, consistency = self._plan(
            binding, call_options, request
        )
        method = _method_of(binding)

        def send() -> Message:
            client = self._client
            assert isinstance(client, ConnectClientSync)
            bearer = self._token_source.token() if self._token_source is not None else None
            return client.execute_unary(
                request=keyed,
                method=method,
                headers=call_headers(call_options, bearer, consistency),
                timeout_ms=call_options.timeout_ms,
                # Unary calls go out as POST even for `no_side_effects`
                # methods. D438 notes reads are marked NO_SIDE_EFFECTS
                # "(HTTP GET)", but every recorded fixture in
                # `sdks/fixtures/recorded` is a POST, the fixture server keys
                # cases on the method, and the Go and Rust SDKs post. A GET
                # gets a 404 and the whole conformance suite fails.
                # POST, as in the sync path above.
                use_get=False,
            )

        response = call_with_retry(
            send,
            rpc=binding.rpc,
            retry_safe=retry_safe,
            max_retries=max_retries,
            on_refresh=self._refresh,
        )
        record_consistency(session, response)
        return response

    def stream(
        self, binding: CallBinding, request: Message, options: CallOptions | None = None
    ) -> Iterator[Message]:
        """One server stream, with the errors mapped.

        The mapping is not optional here. A refusal on a stream arrives inside
        the Connect envelope, not as an HTTP status, so a caller that iterated
        the raw iterator would see a `ConnectError` with no `reason` — the one
        place in the SDK where `reason` could go missing. `loams.stream()` adds
        the resume on top.
        """
        call_options = options or CallOptions()
        _, _, _, session, consistency = self._plan(binding, call_options, request)
        method = _method_of(binding)
        client = self._client
        assert isinstance(client, ConnectClientSync)

        def open_stream() -> Iterator[Message]:
            """A fresh stream per call, so a refresh can actually re-open one.

            Deliberately a callable and not a stream built once here: `_mapped`
            re-opens after a token expiry by calling this again, and a stream
            built eagerly could only be re-iterated -- which silently yields
            nothing, because the first attempt already consumed it. The bearer is
            read per attempt for the same reason the retry loop reads it per
            attempt: the whole point is to send the *new* token.
            """
            return client.execute_server_stream(
                request=request,
                method=method,
                headers=call_headers(call_options, self._bearer(), consistency),
                timeout_ms=call_options.timeout_ms,
            )

        return self._mapped(open_stream, binding.rpc, session, call_options)

    def _bearer(self) -> str | None:
        return self._token_source.token() if self._token_source is not None else None

    def _mapped(
        self,
        open_stream: Callable[[], Iterator[Message]],
        rpc: str,
        session: ConsistencyTokenStore | None,
        options: CallOptions,
    ) -> Iterator[Message]:
        """Wraps a stream so every throw becomes a `LoamsError`."""
        refreshed = False
        yielded = False
        refresh = self._refresh
        while True:
            source = open_stream()
            try:
                for message in source:
                    yielded = True
                    record_consistency(session, message)
                    yield message
                return
            except BaseException as thrown:  # noqa: BLE001 - re-raised as a LoamsError
                error = to_loams_error(thrown, rpc)
                # R1 on a stream: one refresh and one re-open, and only if
                # nothing has been yielded yet. Once messages are flowing the
                # caller is holding a position in the stream, and re-opening it
                # is `loams.stream()`'s job (it resumes from the cursor);
                # replaying from the start here would duplicate everything the
                # caller has already seen.
                if (
                    isinstance(error, TokenExpiredError)
                    and not refreshed
                    and refresh is not None
                    and not yielded
                ):
                    refreshed = True
                    refresh()
                    continue
                raise error from None


class AsyncCallInvoker(_BaseInvoker):
    """The async call path, with the same decisions as `CallInvoker`."""

    def __init__(
        self,
        client: ConnectClient,
        token_source: AsyncTokenSource | None,
        max_retries: int,
        consistency: ConsistencyTokenStore | None,
    ) -> None:
        super().__init__(client, max_retries, consistency)
        self._token_source = token_source
        self._refresh = async_refresh_of(token_source) if token_source is not None else None

    async def unary(
        self, binding: CallBinding, request: Message, options: CallOptions | None = None
    ) -> Message:
        """One unary call: the whole runtime contract, applied to one binding."""
        call_options = options or CallOptions()
        keyed, retry_safe, max_retries, session, consistency = self._plan(
            binding, call_options, request
        )
        method = _method_of(binding)

        async def send() -> Message:
            client = self._client
            assert isinstance(client, ConnectClient)
            bearer = (
                await self._token_source.token() if self._token_source is not None else None
            )
            return await client.execute_unary(
                request=keyed,
                method=method,
                headers=call_headers(call_options, bearer, consistency),
                timeout_ms=call_options.timeout_ms,
                use_get=False,
            )

        response = await async_call_with_retry(
            send,
            rpc=binding.rpc,
            retry_safe=retry_safe,
            max_retries=max_retries,
            on_refresh=self._refresh,
        )
        record_consistency(session, response)
        return response

    async def _bearer(self) -> str | None:
        if self._token_source is None:
            return None
        return await self._token_source.token()

    def stream(
        self, binding: CallBinding, request: Message, options: CallOptions | None = None
    ) -> AsyncIterator[Message]:
        """One async server stream, with the errors mapped.

        A plain `def` returning the iterator, not an `async def` returning one,
        because that is what the generated `LiveModule.watch` declares --
        `def watch(...) -> AsyncIterator[Transition]` -- and it is what
        `AsyncLoams.stream` passes to `async_watch`. As an `async def` this
        returned a coroutine, so `async for t in client.live.watch(req)` raised
        `TypeError: 'async for' requires an object with __aiter__ method, got
        coroutine`, and `AsyncLoams.stream` was broken by the same cause. Neither
        path had ever been run.

        The shape mirrors the sync `stream`: validation stays eager so a bad
        binding fails at the call, and only the token resolution is deferred,
        since that is the one thing that has to be awaited.
        """
        call_options = options or CallOptions()
        _, _, _, session, consistency = self._plan(binding, call_options, request)
        method = _method_of(binding)
        client = self._client
        assert isinstance(client, ConnectClient)

        async def open_stream() -> AsyncIterator[Message]:
            """A fresh stream per call, so a refresh can actually re-open one.

            The same reason as the sync `stream`, and the same bug if it is built
            once: `_async_mapped` re-opens by calling this again, and re-iterating
            a consumed async iterator silently yields nothing.
            """
            bearer = await self._bearer()
            return client.execute_server_stream(
                request=request,
                method=method,
                headers=call_headers(call_options, bearer, consistency),
                timeout_ms=call_options.timeout_ms,
            )

        async def open() -> AsyncIterator[Message]:
            async for message in _async_mapped(
                open_stream, binding.rpc, session, self._refresh, call_options
            ):
                yield message

        return open()


async def _async_mapped(
    open_stream: Callable[[], Awaitable[AsyncIterator[Message]]],
    rpc: str,
    session: ConsistencyTokenStore | None,
    refresh: Callable[[], Awaitable[None]] | None,
    options: CallOptions,
) -> AsyncIterator[Message]:
    """Wraps an async stream so every throw becomes a `LoamsError`."""
    del options
    refreshed = False
    yielded = False
    while True:
        source = await open_stream()
        try:
            async for message in source:
                yielded = True
                record_consistency(session, message)
                yield message
            return
        except BaseException as thrown:  # noqa: BLE001 - re-raised as a LoamsError
            error = to_loams_error(thrown, rpc)
            if (
                isinstance(error, TokenExpiredError)
                and not refreshed
                and refresh is not None
                and not yielded
            ):
                refreshed = True
                await refresh()
                continue
            raise error from None
