"""Token sources (design §44 §7.4, D608; runtime contract R1).

A token source returns a bearer and can be asked for a new one. Tokens travel in
`Authorization: Bearer` and never in a query string. A `401` carrying
`reason = token_expired` triggers one refresh and one retry; that logic lives in
the call path, so a source stays a source.

Every source here has a sync and an async form, because `Loams` and `AsyncLoams`
are separate objects over one contract and a caller who chose the async one must
not have to block a thread to refresh a token. The two forms of one credential
read the same thing — `api_key(k).token()` and `async_api_key(k).token()` both
answer `k` — and `python_token_source_refresh` pins that they do.

`refresh` is **optional** on a source, which Python's `Protocol` cannot say
directly. `refresh_of` is how the call path asks: it returns the bound method or
`None`, and `None` is what makes a refresh a no-op for a credential that does not
expire. That is R1's "a source that cannot refresh makes the refresh a no-op".
"""

from __future__ import annotations

import asyncio
import json
import os
import threading
import urllib.error
import urllib.parse
import urllib.request
from collections.abc import Awaitable, Callable, Mapping
from typing import Final, Protocol, runtime_checkable

__all__ = [
    "AsyncTokenSource",
    "OidcExchangeOptions",
    "TokenSource",
    "api_key",
    "async_api_key",
    "async_env_token",
    "async_oidc_exchange",
    "async_refreshing",
    "async_static_token",
    "env_token",
    "oidc_exchange",
    "refresh_of",
    "async_refresh_of",
    "refreshing",
    "static_token",
]

#: `LOAMS_API_KEY`, then `LOAMS_TOKEN`, then nothing. The environment is read on
#: every call rather than once at construction, so a process that receives its
#: credentials after the client is built (a sidecar, a test) still authenticates.
ENV_API_KEY: Final[str] = "LOAMS_API_KEY"
ENV_TOKEN: Final[str] = "LOAMS_TOKEN"
#: The endpoint variable the examples read, named here so the two cannot drift.
ENV_ENDPOINT: Final[str] = "LOAMS_ENDPOINT"


@runtime_checkable
class TokenSource(Protocol):
    """Where a call's bearer comes from.

    `token()` is called once per attempt, so a source may return a different
    token each time. A source that *can* refresh also has a `refresh()`; ask for
    it with `refresh_of` rather than with `hasattr` at each call site.
    """

    def token(self) -> str | None:
        """The bearer to send, or `None` to send no credential at all."""
        ...


@runtime_checkable
class AsyncTokenSource(Protocol):
    """`TokenSource` for `AsyncLoams`. The same contract, awaited."""

    async def token(self) -> str | None:
        """The bearer to send, or `None` to send no credential at all."""
        ...


def refresh_of(source: TokenSource) -> Callable[[], None] | None:
    """The source's refresh, or `None` when it has none (an API key)."""
    candidate = getattr(source, "refresh", None)
    return candidate if callable(candidate) else None


def async_refresh_of(source: AsyncTokenSource) -> Callable[[], Awaitable[None]] | None:
    """The source's async refresh, or `None` when it has none."""
    candidate = getattr(source, "refresh", None)
    return candidate if callable(candidate) else None


# ---------------------------------------------------------------------------
# Credentials that do not expire.
# ---------------------------------------------------------------------------


class _Static:
    """A credential that never changes, so there is nothing to refresh."""

    def __init__(self, value: str, name: str) -> None:
        if value == "":
            raise ValueError(f"{name}: the credential is empty")
        self._value = value

    def token(self) -> str:
        return self._value


class _Env:
    """`LOAMS_API_KEY`, then `LOAMS_TOKEN`, then nothing."""

    def __init__(self, environment: Mapping[str, str] | None = None) -> None:
        self._environment = environment

    def _read(self) -> str | None:
        environment = self._environment if self._environment is not None else os.environ
        return environment.get(ENV_API_KEY) or environment.get(ENV_TOKEN) or None

    def token(self) -> str | None:
        return self._read()


class _AsyncStatic:
    """`_Static` for `AsyncLoams`: the same credential, awaited.

    Separate rather than a subclass because the two protocols disagree on
    `token`'s return type -- `str` against `Coroutine[..., str]` -- and a class
    cannot be both. Handing the sync object to an async client was worse than a
    type error: `await source.token()` on a `str` fails at the first call with
    "object str can't be used in 'await' expression", which surfaces as an
    opaque `LoamsError` from the retry loop rather than as a wrong credential.
    """

    def __init__(self, value: str, name: str) -> None:
        if value == "":
            raise ValueError(f"{name}: the credential is empty")
        self._value = value

    async def token(self) -> str:
        return self._value


class _AsyncEnv:
    """`_Env` for `AsyncLoams`. Same lookup order, awaited."""

    def __init__(self, environment: Mapping[str, str] | None = None) -> None:
        self._environment = environment

    def _read(self) -> str | None:
        environment = self._environment if self._environment is not None else os.environ
        return environment.get(ENV_API_KEY) or environment.get(ENV_TOKEN) or None

    async def token(self) -> str | None:
        return self._read()



def api_key(key: str) -> TokenSource:
    """A Loams API key. The key does not expire, so there is nothing to refresh."""
    source = _Static(key, "api_key")
    return source


def async_api_key(key: str) -> AsyncTokenSource:
    """`api_key` for `AsyncLoams`."""
    return _AsyncStatic(key, "api_key")


def static_token(token: str) -> TokenSource:
    """A token that is already valid, for a caller who manages its own."""
    source = _Static(token, "static_token")
    return source


def async_static_token(token: str) -> AsyncTokenSource:
    """`static_token` for `AsyncLoams`."""
    return _AsyncStatic(token, "static_token")


def env_token(environment: Mapping[str, str] | None = None) -> TokenSource:
    """`LOAMS_API_KEY`, then `LOAMS_TOKEN`, then nothing."""
    source = _Env(environment)
    return source


def async_env_token(environment: Mapping[str, str] | None = None) -> AsyncTokenSource:
    """`env_token` for `AsyncLoams`."""
    return _AsyncEnv(environment)


# ---------------------------------------------------------------------------
# Credentials that do.
# ---------------------------------------------------------------------------


class _Cache:
    """The token and the count of exchanges, shared by both forms of a source."""

    def __init__(self) -> None:
        self.token_value: str | None = None
        #: How many exchanges have actually happened. What the "one refresh for
        #: a burst of concurrent callers" test reads.
        self.calls = 0


class _Refreshing:
    """Sync side: one in-flight fetch shared by concurrent callers."""

    def __init__(self, fetch: Callable[[], str], cache: _Cache) -> None:
        self._fetch = fetch
        self._cache = cache
        self._lock = threading.Lock()
        self._in_flight: threading.Event | None = None

    def token(self) -> str:
        if self._cache.token_value is not None:
            return self._cache.token_value
        # The first `token()` fetches: a source whose cache starts empty would
        # send no credential at all, and an instance that requires one answers
        # `unauthenticated`, which the call path treats as "the token expired"
        # and retries — with still no credential.
        self.refresh()
        assert self._cache.token_value is not None
        return self._cache.token_value

    def refresh(self) -> None:
        with self._lock:
            if self._in_flight is not None:
                event = self._in_flight
                mine = False
            else:
                event = threading.Event()
                self._in_flight = event
                mine = True
        if not mine:
            event.wait()
            return
        try:
            self._cache.calls += 1
            self._cache.token_value = self._fetch()
        finally:
            event.set()
            with self._lock:
                if self._in_flight is event:
                    self._in_flight = None


class _AsyncRefreshing:
    """Async side: one in-flight fetch shared by concurrent awaiters.

    The lock is what shares a single exchange, and the re-read *inside* it is
    what stops a second waiter fetching again after the first populated the
    cache.
    """

    def __init__(self, fetch: Callable[[], Awaitable[str]], cache: _Cache) -> None:
        self._fetch = fetch
        self._cache = cache
        self._lock: asyncio.Lock | None = None

    def _get_lock(self) -> asyncio.Lock:
        if self._lock is None:
            self._lock = asyncio.Lock()
        return self._lock

    async def token(self) -> str:
        if self._cache.token_value is not None:
            return self._cache.token_value
        await self.refresh()
        assert self._cache.token_value is not None
        return self._cache.token_value

    async def refresh(self) -> None:
        async with self._get_lock():
            if self._cache.token_value is None:
                self._cache.calls += 1
                self._cache.token_value = await self._fetch()


def refreshing(fetch: Callable[[], str]) -> TokenSource:
    """A source that caches and calls `fetch` when asked to refresh."""
    source = _Refreshing(fetch, _Cache())
    return source


def async_refreshing(fetch: Callable[[], Awaitable[str]]) -> AsyncTokenSource:
    """A source that caches and awaits `fetch` when asked to refresh."""
    source = _AsyncRefreshing(fetch, _Cache())
    return source


# ---------------------------------------------------------------------------
# The RFC 8693 token exchange.
# ---------------------------------------------------------------------------


class OidcExchangeOptions:
    """What the token exchange a person signed in through Authentik needs
    (§44 §7.4, D608; §19 §5.2).

    The instance's `/oauth/token` protocol endpoint takes the identity token and
    answers with a Loams access token, which is then cached until it expires.

    **Not covered by the conformance suite:** the instance serves no OAuth
    endpoint yet (the auth plan, MT, and API1 Task 7 build it), so this is
    written to the documented request and response and cannot be exercised
    against a live server. `python_token_source_refresh` covers the caching and
    the refresh-once-and-retry behaviour that `refreshing()` implements, which is
    the part the SDK owns.
    """

    def __init__(
        self,
        *,
        endpoint: str,
        client_id: str,
        subject_token: Callable[[], str],
        post: Callable[[str, bytes], tuple[int, bytes]] | None = None,
    ) -> None:
        """:param post: injected for tests; defaults to a form POST over `urllib`."""
        self.endpoint = endpoint
        self.client_id = client_id
        self.subject_token = subject_token
        self.post = post


def _default_post(url: str, body: bytes) -> tuple[int, bytes]:
    """One form POST, returning `(status, body)`."""
    request = urllib.request.Request(  # noqa: S310 - the URL is the instance's own endpoint
        url, data=body, headers={"content-type": "application/x-www-form-urlencoded"}
    )
    try:
        with urllib.request.urlopen(request) as response:  # noqa: S310
            return int(response.status), response.read()
    except urllib.error.HTTPError as error:
        return int(error.code), error.read()


def _exchange(options: OidcExchangeOptions) -> str:
    """The exchange itself: one form POST, and the access token out of it."""
    post = options.post or _default_post
    status, body = post(
        options.endpoint,
        urllib.parse.urlencode(
            {
                "grant_type": "urn:ietf:params:oauth:grant-type:token-exchange",
                "subject_token_type": "urn:ietf:params:oauth:token-type:id_token",
                "requested_token_type": "urn:ietf:params:oauth:token-type:access_token",
                "subject_token": options.subject_token(),
                "client_id": options.client_id,
            }
        ).encode(),
    )
    if not 200 <= status < 300:
        raise RuntimeError(f"the token exchange answered {status}")
    access_token = json.loads(body).get("access_token")
    if not isinstance(access_token, str) or access_token == "":
        raise RuntimeError("the token exchange answered no access_token")
    return access_token


def oidc_exchange(options: OidcExchangeOptions) -> TokenSource:
    """The token exchange a person signed in through Authentik needs."""
    source = _Refreshing(lambda: _exchange(options), _Cache())
    return source


async def _exchange_in_executor(options: OidcExchangeOptions) -> str:
    """The exchange off the event loop, so `AsyncLoams` never blocks a caller."""
    loop = asyncio.get_running_loop()
    return str(await loop.run_in_executor(None, lambda: _exchange(options)))


def async_oidc_exchange(options: OidcExchangeOptions) -> AsyncTokenSource:
    """`oidc_exchange` for `AsyncLoams`."""
    source = _AsyncRefreshing(lambda: _exchange_in_executor(options), _Cache())
    return source
