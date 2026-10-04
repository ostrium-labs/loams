"""`SystemApi` and `AsyncSystemApi`: the catalogue, the guard and the version check.

These two are separate implementations over the same decisions, and only the
synchronous one had any coverage. That is how `AsyncSystemApi.catalogue` came to
hold a **coroutine** in its `_in_flight` slot:

- a coroutine can be awaited exactly once, so the second reader that shared the
  in-flight fetch got `RuntimeError: cannot reuse already awaited coroutine`
  rather than the catalogue; and
- there was no `finally`, so a fetch that raised left the dead coroutine in the
  slot forever, and every later check failed the same way instead of retrying.

Both turn one failed or concurrent `GetInstance` into a permanently broken
`loams.system` with an error that names neither the RPC nor the cause. The sync
version has a `try/finally` for exactly this reason.

The tests below are written against the promises in the docstrings -- one call
for twenty concurrent readers, a retry after failure -- because those are the
promises a UI relying on this for a cold start actually depends on.
"""

from __future__ import annotations

import asyncio

import pytest

from loams.instance.v1.instance_pb2 import GetInstanceResponse, ServiceStatus
from loams.runtime.errors import FeatureNotInVariantError, LoamsError
from loams.system import AsyncSystemApi, SystemApi, package_of, spoken_packages

SERVED = "loams.live.v1"
ABSENT = "loams.notifications.v1"


def _services() -> tuple[ServiceStatus, ...]:
    return (
        ServiceStatus(package=SERVED, available=True),
        ServiceStatus(package=ABSENT, available=False),
    )


def _response() -> GetInstanceResponse:
    return GetInstanceResponse(services=_services(), server_version="1.2.3")


class _Stub:
    """Counts the calls, so caching is observable rather than inferred."""

    def __init__(self, *responses: GetInstanceResponse | Exception) -> None:
        self.calls = 0
        self._responses = list(responses) or [_response()]

    def _next(self) -> GetInstanceResponse:
        self.calls += 1
        item = self._responses.pop(0) if len(self._responses) > 1 else self._responses[0]
        if isinstance(item, Exception):
            raise item
        return item

    def sync(self, request, *, options=None):
        return self._next()

    async def call(self, request, *, options=None):
        # A real `get_instance` awaits the network, so it suspends. A stub that
        # returns without suspending cannot reproduce a concurrency bug: the
        # first reader would finish before the second one starts and the rest
        # would only ever read the warm cache.
        await asyncio.sleep(0)
        return self._next()


# -- the shared decisions -----------------------------------------------------


def test_package_of_takes_a_module_or_a_package() -> None:
    assert package_of(SERVED) == SERVED, "a package name passed through unchanged"
    assert package_of("live") == SERVED, "a module name did not resolve to its package"


def test_an_unknown_module_name_is_refused() -> None:
    """A typo must not read as 'the feature is absent', which `guard` would turn
    into a `FeatureNotInVariantError` blaming the server."""
    with pytest.raises(LoamsError):
        package_of("nosuchmodule")


# -- sync ---------------------------------------------------------------------


def test_the_catalogue_is_fetched_once_and_cached() -> None:
    stub = _Stub()
    system = SystemApi(stub.sync)
    first, second = system.catalogue(), system.catalogue()
    assert stub.calls == 1, f"the catalogue cost {stub.calls} calls, want 1"
    assert first is second
    assert system.served() == (SERVED,)
    assert system.unavailable() == (ABSENT,)


def test_invalidate_makes_the_next_check_call_again() -> None:
    stub = _Stub()
    system = SystemApi(stub.sync)
    system.catalogue()
    system.invalidate()
    system.catalogue()
    assert stub.calls == 2, "invalidate() did not drop the cache"


def test_a_failed_fetch_is_not_left_in_flight() -> None:
    """The sync side has the `try/finally`; this is what it is for."""
    stub = _Stub(RuntimeError("the instance is down"))
    system = SystemApi(stub.sync)
    with pytest.raises(RuntimeError):
        system.catalogue()
    stub._responses = [_response()]
    assert system.available(SERVED) is True, (
        "one failure poisoned the system API; the retry never happened"
    )


def test_available_accepts_either_spelling() -> None:
    system = SystemApi(_Stub().sync)
    assert system.available("live") is True
    assert system.available(SERVED) is True
    assert system.available(ABSENT) is False


def test_guard_raises_the_servers_own_refusal() -> None:
    system = SystemApi(_Stub().sync)
    system.guard(SERVED)
    with pytest.raises(FeatureNotInVariantError) as caught:
        system.guard(ABSENT)
    assert caught.value.reason == "feature_not_in_variant"
    assert caught.value.metadata.get("package") == ABSENT


def test_version_reports_a_missing_package_as_a_warning() -> None:
    """A missing package is not an exception: the SDK still works for the rest."""
    stub = _Stub()
    report = SystemApi(stub.sync).version()
    assert report.server_version == "1.2.3"
    assert report.compatible is False, (
        f"an empty api_versions should read as missing, got {report.missing}"
    )
    assert set(report.missing) == set(spoken_packages())


def test_version_is_compatible_when_the_server_speaks_everything() -> None:
    stub = _Stub()
    stub._responses = [GetInstanceResponse(api_versions=spoken_packages(), server_version="1.2.3")]
    report = SystemApi(stub.sync).version()
    assert report.compatible is True
    assert report.missing == ()


# -- async --------------------------------------------------------------------


def test_the_async_catalogue_is_fetched_once_and_cached() -> None:
    stub = _Stub()

    async def walk() -> tuple[int, tuple[str, ...], tuple[str, ...]]:
        system = AsyncSystemApi(stub.call)
        first, second = await system.catalogue(), await system.catalogue()
        assert first is second, "the second catalogue() built a new one"
        return stub.calls, await system.served(), await system.unavailable()

    calls, served, unavailable = asyncio.run(walk())
    assert calls == 1, f"the async catalogue cost {calls} calls, want 1"
    assert served == (SERVED,)
    assert unavailable == (ABSENT,)


def test_concurrent_async_readers_share_one_fetch() -> None:
    """Twenty availability checks on a cold start must cost one call, not twenty.

    This is the docstring's promise and the reason `_in_flight` exists. It is
    also where the coroutine-in-a-slot bug showed: each reader awaited the same
    coroutine object, and the second await raised `RuntimeError: cannot reuse
    already awaited coroutine`.
    """

    async def walk() -> tuple[int, list[bool]]:
        stub = _Stub()
        system = AsyncSystemApi(stub.call)
        results = await asyncio.gather(*(system.available(SERVED) for _ in range(20)))
        return stub.calls, list(results)

    calls, results = asyncio.run(walk())
    assert calls == 1, f"a cold start made {calls} calls, want 1"
    assert all(results), "a concurrent reader did not get an answer"


def test_a_failed_async_fetch_is_retried_rather_than_poisoning_the_api() -> None:
    """Without a `finally`, the dead coroutine stayed in the slot forever."""
    stub = _Stub(RuntimeError("the instance is down"), _response())

    async def walk() -> bool:
        system = AsyncSystemApi(stub.call)
        with pytest.raises(RuntimeError):
            await system.catalogue()
        return await system.available(SERVED)

    assert asyncio.run(walk()) is True, (
        "one failed GetInstance left the async system API permanently broken; the "
        "retry raised instead of re-fetching"
    )


def test_the_async_guard_raises_the_servers_own_refusal() -> None:
    """The async guard must produce the same class and reason as the sync one.

    A caller writes one `except FeatureNotInVariantError` around a feature; if
    the async guard raised anything else, "the instance does not have this" and
    "the guard said no" would need separate handling.
    """

    async def walk() -> FeatureNotInVariantError:
        system = AsyncSystemApi(_Stub().call)
        await system.guard(SERVED)
        with pytest.raises(FeatureNotInVariantError) as caught:
            await system.guard(ABSENT)
        return caught.value

    refused = asyncio.run(walk())
    assert refused.reason == "feature_not_in_variant"
    assert refused.metadata.get("package") == ABSENT


def test_the_async_version_reports_missing_packages() -> None:
    stub = _Stub()

    async def walk():
        return await AsyncSystemApi(stub.call).version()

    report = asyncio.run(walk())
    assert report.compatible is False
    assert report.missing


def test_async_invalidate_drops_the_cache() -> None:
    stub = _Stub()

    async def walk() -> int:
        system = AsyncSystemApi(stub.call)
        await system.catalogue()
        system.invalidate()
        await system.catalogue()
        return stub.calls

    assert asyncio.run(walk()) == 2, "async invalidate() did not drop the cache"