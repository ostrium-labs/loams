"""`AsyncLoams` and its token sources, which nothing else here covers.

Every other test in this suite drives the synchronous client. That left the
async client untested while it was being changed -- and it was broken. The three
`async_*` token factories returned the *synchronous* source object with a
`# type: ignore[return-value]` on the return, which silenced the one check that
would have caught it. `AsyncCallInvoker._bearer` does `await source.token()`, so
every `AsyncLoams` call carrying any credential raised `object str can't be used
in 'await' expression` -- surfacing as an opaque `LoamsError` from the retry loop
rather than as a wrong credential, which is a miserable thing to diagnose.

So these are mostly small tests, and the smallness is the point: a client's whole
credential path should be a handful of assertions, not an integration suite.

`test_the_async_and_sync_sources_answer_the_same_question` is the one that would
have caught it, because it asserts the *protocol* rather than one call.
"""

from __future__ import annotations

import asyncio
import inspect

import pytest

from loams import (
    AsyncLoams,
    async_api_key,
    async_env_token,
    async_refreshing,
    async_static_token,
)
from loams.instance.v1.instance_pb2 import GetInstanceRequest
from loams.runtime.token_source import AsyncTokenSource, TokenSource

#: Every async factory, with a credential each one accepts.
ASYNC_FACTORIES = (
    (async_api_key, ("loams_key",), {}),
    (async_static_token, ("a-token",), {}),
    (async_env_token, (), {"environment": {"LOAMS_API_KEY": "loams_key"}}),
)


def test_the_async_and_sync_sources_answer_the_same_question() -> None:
    """`AsyncTokenSource.token` must be awaitable, and actually awaitable.

    Structural, so it catches a factory handing back the sync object whatever the
    call path happens to do with it.
    """
    for factory, args, kwargs in ASYNC_FACTORIES:
        source = factory(*args, **kwargs)
        assert isinstance(source, AsyncTokenSource), (
            f"{factory.__name__} returned {type(source).__name__}, which is not an "
            "AsyncTokenSource"
        )
        method = getattr(type(source), "token")
        assert inspect.iscoroutinefunction(method), (
            f"{factory.__name__} returned a source whose token() is "
            f"{'async' if method.__qualname__.endswith('atoken') else 'not a'} coroutine "
            "function, so `await source.token()` cannot work"
        )


def test_every_async_factory_yields_the_credential_it_was_given() -> None:
    """Each factory's value, read through the async protocol."""
    for factory, args, kwargs in ASYNC_FACTORIES:
        source = factory(*args, **kwargs)
        expected = args[0] if args else kwargs["environment"]["LOAMS_API_KEY"]
        assert asyncio.run(source.token()) == expected, factory.__name__


def test_an_empty_credential_is_refused_by_both_forms() -> None:
    """A client that sends no credential is worse than one that fails to build."""
    from loams import api_key, env_token, static_token

    for factory in (api_key, async_api_key, static_token, async_static_token):
        with pytest.raises(ValueError):
            factory("")


@pytest.mark.parametrize("factory,args,kwargs", ASYNC_FACTORIES)
def test_async_calls_send_the_credential(endpoint: str, factory, args, kwargs) -> None:
    """A call carrying a credential reaches the server, and the server answers.

    The fixture server records no credential requirement for `GetInstance`, so
    this proves the header was *built*, not that the server demanded it. It is
    the end-to-end half of the bug above: the failure was inside this call.
    """
    source = factory(*args, **kwargs)

    async def call() -> object:
        async with AsyncLoams(endpoint, auth=source) as client:
            return await client.instance.get_instance(GetInstanceRequest())

    assert asyncio.run(call()).name != ""


def test_an_async_call_without_a_credential_still_works(endpoint: str) -> None:
    """`auth=None` sends no header, which is a valid client."""
    seen: list[str | None] = []

    class _Capturing:
        async def token(self) -> str | None:
            seen.append("called")
            return None

    async def call() -> object:
        async with AsyncLoams(endpoint, auth=_Capturing()) as client:
            return await client.instance.get_instance(GetInstanceRequest())

    assert asyncio.run(call()) is not None
    assert seen == ["called"], "the token source was not consulted"


def test_an_async_refreshing_source_is_awaited_and_caches(endpoint: str) -> None:
    """The async refreshing source keeps its single-exchange guarantee."""
    fetches = 0

    async def fetch() -> str:
        nonlocal fetches
        fetches += 1
        return f"token-{fetches}"

    source = async_refreshing(fetch)

    async def call() -> object:
        async with AsyncLoams(endpoint, auth=source) as client:
            await client.instance.get_instance(GetInstanceRequest())
            await client.instance.get_instance(GetInstanceRequest())

    asyncio.run(call())
    assert fetches == 1, f"the source was fetched {fetches} times, want 1 for two calls"


def test_a_structural_protocol_check_cannot_tell_the_two_forms_apart() -> None:
    """Documents the trap that hid the bug, so it cannot hide the next one.

    `TokenSource` and `AsyncTokenSource` are `runtime_checkable` protocols whose
    only difference is whether `token` returns a `str` or a coroutine. A
    structural check sees *method presence*, not return type, so the synchronous
    source satisfies the asynchronous protocol:

        isinstance(api_key("k"), AsyncTokenSource)  # True

    So neither `isinstance` nor the return annotation can catch a factory handing
    back the wrong form -- `async_api_key` returned `_Static` and mypy was silenced
    by a `# type: ignore[return-value]` rather than fixed. Only awaiting tells
    them apart, which is why `test_the_async_and_sync_sources_answer_the_same_question`
    asserts on the method rather than on the instance, and why this is pinned as
    a documented fact instead of a convenient belief.
    """
    from loams import api_key

    sync_source = api_key("k")
    assert not inspect.iscoroutinefunction(type(sync_source).token)
    assert isinstance(sync_source, TokenSource)
    # The misleading half: true, and the reason the bug was invisible.
    assert isinstance(sync_source, AsyncTokenSource), (
        "runtime protocol checks only see method presence; if this ever becomes "
        "False the distinction is being enforced somewhere new"
    )
    # Which is why the async factories are checked structurally, by method.
    assert all(
        inspect.iscoroutinefunction(type(factory(*args, **kwargs)).token)
        for factory, args, kwargs in ASYNC_FACTORIES
    ), "an async factory handed back a synchronous source"


def test_an_async_refresh_re_fetches_over_a_populated_cache() -> None:
    """`refresh()` must actually refresh. The cache being full is the normal case.

    This is the regression test for the bug that made the async client unable to
    recover from an expired token: `_AsyncRefreshing.refresh` fetched only when
    the cache was *empty*, so over a populated cache it did nothing at all. R1's
    refresh-once-and-retry then re-opened the stream and resent the very token the
    server had just rejected, and the watch ended as if it had seen no changes.
    Nothing raised anywhere -- which is why only a wire-level test found it.
    """
    from loams.runtime.token_source import async_refreshing, refreshing

    calls: list[str] = []

    async def fetch_async() -> str:
        calls.append("fetch")
        return f"token-{len(calls)}"

    def fetch_sync() -> str:
        calls.append("fetch")
        return f"token-{len(calls)}"

    async def walk_async() -> tuple[str, str]:
        source = async_refreshing(fetch_async)
        first = await source.token()
        await source.refresh()
        second = await source.token()
        return first, second

    assert asyncio.run(walk_async()) == ("token-1", "token-2")
    assert len(calls) == 2, f"a refresh over a full cache fetched {len(calls) - 1} times, want 1"

    calls.clear()
    sync_source = refreshing(fetch_sync)
    assert sync_source.token() == "token-1"
    sync_source.refresh()
    assert sync_source.token() == "token-2", "the sync source does not agree, which would be news"


def test_concurrent_async_refreshes_share_one_fetch() -> None:
    """The sharing guarantee survives the move to a `Task`.

    Twenty callers refreshing at once must produce one exchange, not twenty --
    against a token endpoint that would otherwise see a stampede. This is why the
    in-flight slot holds a `Task`: a coroutine can be awaited once, so concurrent
    awaiters on one would raise instead of sharing.
    """
    import asyncio

    calls = 0

    async def fetch() -> str:
        nonlocal calls
        calls += 1
        await asyncio.sleep(0.01)  # a real exchange is not instant
        return f"token-{calls}"

    async def walk() -> None:
        source = async_refreshing(fetch)
        await source.token()  # seed, so the refreshes below start from a full cache
        await asyncio.gather(*(source.refresh() for _ in range(20)))
        assert await source.token() == "token-2"

    asyncio.run(walk())
    assert calls == 2, f"a seed plus 20 concurrent refreshes made {calls} fetches, want 2"


def test_a_failed_async_refresh_is_not_left_in_flight() -> None:
    """A transient failure must not poison the source for the rest of the process."""
    import asyncio

    calls = 0

    async def fetch() -> str:
        nonlocal calls
        calls += 1
        if calls == 1:
            raise RuntimeError("the token endpoint is down")
        return "token-ok"

    async def walk() -> str:
        source = async_refreshing(fetch)
        with pytest.raises(RuntimeError):
            await source.token()
        return await source.token()

    assert asyncio.run(walk()) == "token-ok", (
        "one failed fetch left a dead awaitable in the in-flight slot, so the "
        "retry never happened"
    )
    assert calls == 2, f"the source was fetched {calls} times, want 2"
