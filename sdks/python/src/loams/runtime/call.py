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
import time
from dataclasses import dataclass
from typing import Any, Final, TypeVar

from connectrpc.client import ConnectClient, ConnectClientSync, ResponseMetadata
from connectrpc.code import Code
from connectrpc.method import MethodInfo
from google.protobuf.message import Message

from loams._gen.facade import IDEMPOTENCY_LEVELS, METHODS, CallBinding, Pagination
from loams.runtime.consistency import TOKEN_PREFIX
from loams.runtime.errors import (
    DeadlineExceededError,
    InvalidArgumentError,
    LoamsError,
    TokenExpiredError,
    to_loams_error,
)
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
    "APP_PACKAGES",
    "SESSION_PACKAGES",
    "CONSISTENCY_TOKEN_HEADER",
    "DEFAULT_TIMEOUT_MS",
    "AsyncCallInvoker",
    "CallInvoker",
    "prepare_request",
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

#: The unary deadline when neither the client nor the call sets one (R12).
DEFAULT_TIMEOUT_MS: Final[int] = 30_000

#: The response header a consistency token arrives in (R4).
CONSISTENCY_TOKEN_HEADER: Final[str] = "loams-consistency-token"

#: Below this many seconds a budget counts as spent: the transport's own timer
#: fires a little before the SDK's, and an attempt with a millisecond left can
#: only time out again.
_DEADLINE_SLACK: Final[float] = 0.01

#: The packages R11 (default namespace) and R4's `consistency.at_least` apply to.
#: Elsewhere a `namespace` field can mean something else entirely --
#: `loams.live.v1.CreateAppRequest.namespace` is free text naming an app's
#: environment -- and must never be filled or refused.
APP_PACKAGES: Final[frozenset[str]] = frozenset(
    {"loams.collection.v1", "loams.graph.v1", "loams.sql.v1", "loams.stream.v1"}
)

#: The packages whose reads take the session's token as `consistency.at_least`
#: (R4, SDKP-17): collections and SQL. A graph's token names one graph's stream
#: and a Graph read refuses any other, so graph reads never take the session's.
SESSION_PACKAGES: Final[frozenset[str]] = frozenset({"loams.collection.v1", "loams.sql.v1"})

#: The response fields a consistency token is read from (R4, SDKP §2.1).
_TOKEN_FIELDS: Final[tuple[str, ...]] = ("consistency_token", "token", "read_token")

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
    as_header: bool = True,
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
    # A request with a `consistency` field carries the token there (R4); one
    # without carries it as a header, which is all such an RPC can read.
    if consistency is not None and as_header:
        headers["loams-consistency"] = consistency
    return dict(headers)


# ---------------------------------------------------------------------------
# The request as it goes out (R4, R11).
# ---------------------------------------------------------------------------


def _string_field(request: Message, name: str) -> bool:
    field = request.DESCRIPTOR.fields_by_name.get(name)
    return (
        field is not None
        and field.type == field.TYPE_STRING
        and field.containing_oneof is None
        and not _is_repeated(field)
    )


def _is_repeated(field: Any) -> bool:
    # `label` is deprecated in newer protobuf runtimes in favour of
    # `is_repeated`; both are read so either runtime works.
    repeated = getattr(field, "is_repeated", None)
    if isinstance(repeated, bool):
        return repeated
    return bool(field.label == field.LABEL_REPEATED)


def carries_consistency(request: Message) -> bool:
    """Whether the request's `consistency` message has an `at_least` field.

    Both shapes in the protos qualify: a plain field
    (`loams.collection.v1.Consistency`) and an arm of a oneof
    (`loams.graph.v1.Consistency`). Setting the field selects the arm either way.
    """
    field = request.DESCRIPTOR.fields_by_name.get("consistency")
    if field is None or field.message_type is None:
        return False
    return "at_least" in field.message_type.fields_by_name


def _in_field(binding: CallBinding, request: Message) -> bool:
    """Whether this call carries the session token in `consistency.at_least`
    rather than in the `loams-consistency` header."""
    return binding.package in SESSION_PACKAGES and carries_consistency(request)


def prepare_request(
    binding: CallBinding,
    request: Message,
    namespace: str | None,
    consistency: str | None,
) -> Message:
    """The request as it goes out.

    - **R11**: a request with a string `namespace` field that the caller left
      empty gets the client's default namespace; with neither, the call is
      refused with `InvalidArgumentError` before any network call.
    - **R4**: a collection or SQL read whose `consistency` the caller left
      unset is sent `consistency.at_least = <session token>`; a graph read never
      is (SDKP-17: a graph's token names one graph).

    An explicit value always wins. The caller's request is copied, never
    mutated, so reusing it for a second call sends the same thing twice. Both
    rules apply only to the app packages (`APP_PACKAGES`).
    """
    if binding.package not in APP_PACKAGES:
        return request
    out = request
    if _string_field(request, "namespace") and getattr(request, "namespace") == "":
        if not namespace:
            raise InvalidArgumentError(
                f"{binding.rpc} needs a namespace: pass one, or give the client a default "
                "namespace",
                code=Code.INVALID_ARGUMENT,
                reason="namespace_required",
                rpc=binding.rpc,
                metadata={"field": "namespace"},
            )
        out = _copy(out)
        setattr(out, "namespace", namespace)
    if (
        consistency is not None
        and binding.package in SESSION_PACKAGES
        and carries_consistency(request)
        and not request.HasField("consistency")
    ):
        out = _copy(out) if out is request else out
        setattr(getattr(out, "consistency"), "at_least", consistency)
    return out


def _copy(request: Message) -> Message:
    copy = type(request)()
    copy.CopyFrom(request)
    return copy


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
    remaining: Callable[[], float | None] | None = None,
) -> Message:
    """Runs `send` until it answers or the policy says stop.

    `remaining` is the seconds left of the call's deadline (R12), or `None` for
    no deadline: no attempt starts once it is spent, and a backoff never sleeps
    past it.

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
            left = remaining() if remaining is not None else None
            if (left is not None and left <= 0) or not _retry_decision(
                error, retry_safe, attempt, max_retries
            ):
                raise error from None
            delay = backoff_seconds(attempt)
            sleep(delay if left is None else min(delay, left))
            attempt += 1


async def async_call_with_retry(
    send: Callable[[], Awaitable[Message]],
    *,
    rpc: str,
    retry_safe: bool,
    max_retries: int,
    on_refresh: Callable[[], Awaitable[None]] | None,
    remaining: Callable[[], float | None] | None = None,
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
            left = remaining() if remaining is not None else None
            if (left is not None and left <= 0) or not _retry_decision(
                error, retry_safe, attempt, max_retries
            ):
                raise error from None
            delay = backoff_seconds(attempt)
            await _async_sleep(delay if left is None else min(delay, left))
            attempt += 1


async def _async_sleep(seconds: float) -> None:
    """Sleeps without blocking the event loop."""
    import asyncio

    if seconds > 0:
        await asyncio.sleep(seconds)


# ---------------------------------------------------------------------------
# Recording a write's consistency token (R4).
# ---------------------------------------------------------------------------


def record_consistency(
    session: ConsistencyTokenStore | None,
    response: object,
    header_token: str | None = None,
    scope: str = "",
) -> None:
    """Merges every consistency token a response carried into the session.

    The response fields `consistency_token`, `token` and `read_token`, and the
    `loams-consistency-token` response header (R4, SDKP §2.1). `token` is a
    common field name, so only a `v1:` string is taken: anything else (a page
    token, a bearer) is not the session's business.

    A `v1:` token the session cannot parse must not turn a committed write into a
    thrown error, because a caller that retries on that error performs the write
    twice. `ConsistencySession.rejected` counts it instead.
    """
    if session is None:
        return
    tokens: list[str] = []
    if response is not None:
        for name in _TOKEN_FIELDS:
            value = getattr(response, name, None)
            if isinstance(value, str) and value != "":
                tokens.append(value)
    if header_token:
        tokens.append(header_token)
    for token in tokens:
        if not token.startswith(TOKEN_PREFIX):
            continue
        try:
            session.record(token, scope)
        except LoamsError:
            continue


def _header_token(metadata: ResponseMetadata) -> str | None:
    value = metadata.headers.get(CONSISTENCY_TOKEN_HEADER)
    return value if isinstance(value, str) and value != "" else None


def _client_deadline(binding: CallBinding, timeout_ms: int) -> DeadlineExceededError:
    return DeadlineExceededError(
        f"{binding.rpc} did not answer within the client's {timeout_ms} ms deadline",
        code=Code.DEADLINE_EXCEEDED,
        reason="client_deadline_exceeded",
        rpc=binding.rpc,
        metadata={"timeout_ms": str(timeout_ms)},
    )


@dataclass(frozen=True, slots=True)
class _Deadline:
    """One call's budget, retries included (R12)."""

    timeout_ms: int | None
    expires_at: float | None

    @classmethod
    def start(cls, timeout_ms: int | None) -> _Deadline:
        if timeout_ms is None or timeout_ms <= 0:
            return cls(None, None)
        return cls(timeout_ms, time.monotonic() + timeout_ms / 1000)

    def remaining(self) -> float | None:
        """Seconds left, or `None` for no deadline. A budget within the slack of
        its end answers 0."""
        if self.expires_at is None:
            return None
        left = self.expires_at - time.monotonic()
        return left if left > _DEADLINE_SLACK else 0.0

    def remaining_ms(self) -> int | None:
        """Milliseconds left for the next attempt, at least 1."""
        if self.expires_at is None:
            return None
        return max(1, int((self.expires_at - time.monotonic()) * 1000))

    def expired(self) -> bool:
        left = self.remaining()
        return left is not None and left <= 0


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
        namespace: str | None = None,
        timeout_ms: int | None = DEFAULT_TIMEOUT_MS,
    ) -> None:
        self._client = client
        self._max_retries = max_retries
        self._consistency = consistency
        self._namespace = namespace
        self._timeout_ms = timeout_ms

    @property
    def namespace(self) -> str | None:
        """The client's default namespace (R11), if it has one."""
        return self._namespace

    def _deadline(self, options: CallOptions) -> _Deadline:
        """A unary call's budget: the call's, else the client's (R12)."""
        timeout = options.timeout_ms if options.timeout_ms is not None else self._timeout_ms
        return _Deadline.start(timeout)

    @property
    def consistency(self) -> ConsistencyTokenStore | None:
        """The session store, when `session_consistency` is on."""
        return self._consistency

    def _plan(
        self, binding: CallBinding, options: CallOptions, request: Message
    ) -> tuple[Message, bool, int, ConsistencyTokenStore | None, str | None]:
        """Everything both paths decide the same way before an attempt."""
        session = resolve_session(options.consistency, self._consistency)
        token = options.consistency.token if options.consistency is not None else None
        if token is None and session is not None:
            # R4 (SDKP-17): one token per proto package.
            token = session.current(binding.package)
        request = prepare_request(binding, request, self._namespace, token)
        keyed, has_key = apply_idempotency_key(request, options.idempotency_key)
        max_retries = options.max_retries if options.max_retries is not None else self._max_retries
        # D610: a `safe` call always retries; a mutation retries once it carries
        # an idempotency key, which `apply_idempotency_key` has just decided.
        retry_safe = options.retry_safe if options.retry_safe is not None else (
            binding.retry == "safe" or has_key
        )
        # An explicit token wins; otherwise the session's, which is what makes
        # `consistency=ConsistencyOptions(session=True)` read-your-writes rather
        # than record it and never send it (R4).
        consistency = token
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
        namespace: str | None = None,
        timeout_ms: int | None = DEFAULT_TIMEOUT_MS,
    ) -> None:
        super().__init__(client, max_retries, consistency, namespace, timeout_ms)
        self._token_source = token_source
        self._refresh = refresh_of(token_source) if token_source is not None else None

    def unary(self, binding: CallBinding, request: Message, options: CallOptions | None = None) -> Message:
        """One unary call: the whole runtime contract, applied to one binding."""
        call_options = options or CallOptions()
        keyed, retry_safe, max_retries, session, consistency = self._plan(
            binding, call_options, request
        )
        method = _method_of(binding)
        as_header = not _in_field(binding, keyed)
        deadline = self._deadline(call_options)
        header_token: list[str | None] = [None]

        def send() -> Message:
            client = self._client
            assert isinstance(client, ConnectClientSync)
            bearer = self._token_source.token() if self._token_source is not None else None
            with ResponseMetadata() as metadata:
                answer = client.execute_unary(
                    request=keyed,
                    method=method,
                    headers=call_headers(call_options, bearer, consistency, as_header),
                    # R12: what is left of the call's one budget, so the server
                    # sees it as `connect-timeout-ms` and a retry cannot outlive it.
                    timeout_ms=deadline.remaining_ms(),
                    # Unary calls go out as POST even for `no_side_effects`
                    # methods. D438 notes reads are marked NO_SIDE_EFFECTS
                    # "(HTTP GET)", but every recorded fixture in
                    # `sdks/fixtures/recorded` is a POST, the fixture server keys
                    # cases on the method, and the Go and Rust SDKs post. A GET
                    # gets a 404 and the whole conformance suite fails.
                    use_get=False,
                )
            header_token[0] = _header_token(metadata)
            return answer

        try:
            response = call_with_retry(
                send,
                rpc=binding.rpc,
                retry_safe=retry_safe,
                max_retries=max_retries,
                on_refresh=self._refresh,
                remaining=deadline.remaining,
            )
        except LoamsError as error:
            if deadline.expired() and deadline.timeout_ms is not None:
                raise _client_deadline(binding, deadline.timeout_ms) from error
            raise
        record_consistency(session, response, header_token[0], binding.package)
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
        prepared, _, _, session, consistency = self._plan(binding, call_options, request)
        as_header = not _in_field(binding, prepared)
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
                request=prepared,
                method=method,
                headers=call_headers(call_options, self._bearer(), consistency, as_header),
                timeout_ms=call_options.timeout_ms,
            )

        return self._mapped(open_stream, binding.rpc, session, call_options, binding.package)

    def _bearer(self) -> str | None:
        return self._token_source.token() if self._token_source is not None else None

    def _mapped(
        self,
        open_stream: Callable[[], Iterator[Message]],
        rpc: str,
        session: ConsistencyTokenStore | None,
        options: CallOptions,
        scope: str = "",
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
                    record_consistency(session, message, None, scope)
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
        namespace: str | None = None,
        timeout_ms: int | None = DEFAULT_TIMEOUT_MS,
    ) -> None:
        super().__init__(client, max_retries, consistency, namespace, timeout_ms)
        self._token_source = token_source
        self._refresh = async_refresh_of(token_source) if token_source is not None else None

    async def unary(
        self, binding: CallBinding, request: Message, options: CallOptions | None = None
    ) -> Message:
        """One unary call: the whole runtime contract, applied to one binding."""
        import asyncio

        call_options = options or CallOptions()
        keyed, retry_safe, max_retries, session, consistency = self._plan(
            binding, call_options, request
        )
        method = _method_of(binding)
        as_header = not _in_field(binding, keyed)
        deadline = self._deadline(call_options)
        header_token: list[str | None] = [None]

        async def send() -> Message:
            client = self._client
            assert isinstance(client, ConnectClient)
            bearer = (
                await self._token_source.token() if self._token_source is not None else None
            )
            with ResponseMetadata() as metadata:
                pending = client.execute_unary(
                    request=keyed,
                    method=method,
                    headers=call_headers(call_options, bearer, consistency, as_header),
                    timeout_ms=deadline.remaining_ms(),
                    use_get=False,
                )
                # The SDK holds the budget itself as well as handing it to the
                # transport, so the deadline is the same on every transport.
                left = deadline.remaining()
                answer = await (pending if left is None else asyncio.wait_for(pending, left))
            header_token[0] = _header_token(metadata)
            return answer

        try:
            response = await async_call_with_retry(
                send,
                rpc=binding.rpc,
                retry_safe=retry_safe,
                max_retries=max_retries,
                on_refresh=self._refresh,
                remaining=deadline.remaining,
            )
        except LoamsError as error:
            if deadline.expired() and deadline.timeout_ms is not None:
                raise _client_deadline(binding, deadline.timeout_ms) from error
            raise
        record_consistency(session, response, header_token[0], binding.package)
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
        prepared, _, _, session, consistency = self._plan(binding, call_options, request)
        as_header = not _in_field(binding, prepared)
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
                request=prepared,
                method=method,
                headers=call_headers(call_options, bearer, consistency, as_header),
                timeout_ms=call_options.timeout_ms,
            )

        async def open() -> AsyncIterator[Message]:
            async for message in _async_mapped(
                open_stream, binding.rpc, session, self._refresh, call_options, binding.package
            ):
                yield message

        return open()


async def _async_mapped(
    open_stream: Callable[[], Awaitable[AsyncIterator[Message]]],
    rpc: str,
    session: ConsistencyTokenStore | None,
    refresh: Callable[[], Awaitable[None]] | None,
    options: CallOptions,
    scope: str = "",
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
                record_consistency(session, message, None, scope)
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
