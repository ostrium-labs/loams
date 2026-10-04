"""The client surface itself: the guards, the properties and the two wirings.

`loams.py` and `aio.py` are described in their own docstrings as "the thin join"
-- they read the generated table, build one object per module, and hand every
call to the same invoker. That is exactly why their untested lines went
unnoticed: nothing in them looks like behaviour.

Coverage measured with stdlib `trace --count` had, on both clients:

- both constructor guards unrun (`api_key` and `auth` together, empty endpoint);
- `consistency`, `proto_rev` and `proto_packages` unrun;
- `binding()` unrun, including its "a module name is required" guard;
- `invalidate_catalogue()` unrun; and
- `paginate()` unrun on both, plus `stream()` on the sync client.

The constructor guards matter most: `pass api_key or auth, not both` is a
misuse error, and an SDK that silently prefers one of two credentials is worse
than one that refuses.

`paginate` cannot be driven end to end because no RPC is paged yet --
`ListCollections` is API1 Task 2, as `Loams.paginate`'s own docstring says -- so
what is pinned here is the refusal, which is the part a caller meets today.
"""

from __future__ import annotations

import asyncio

import pytest

from loams import AsyncLoams, Loams
from loams._gen.facade import PROTO_PACKAGES, PROTO_REV
from loams.instance.v1.instance_pb2 import GetInstanceRequest
from loams.live.v1.live_pb2 import WatchRequest
from loams.runtime.errors import LoamsError

ENDPOINT = "http://127.0.0.1:1"  # never dialled by these tests


# -- the constructor guards ---------------------------------------------------


@pytest.mark.parametrize("factory", [Loams, AsyncLoams])
def test_two_credentials_are_refused(factory) -> None:
    """`api_key` and `auth` answer the same question, so one of them would win by
    accident and the caller would find out on the first 401 instead of here."""
    from loams import api_key, async_api_key

    source = api_key("k") if factory is Loams else async_api_key("k")
    with pytest.raises(ValueError) as caught:
        factory(ENDPOINT, api_key="k", auth=source)
    assert "not both" in str(caught.value), (
        f"the error does not say which mistake was made: {caught.value}"
    )


@pytest.mark.parametrize("factory", [Loams, AsyncLoams])
@pytest.mark.parametrize("endpoint", ["", "   "])
def test_an_empty_endpoint_is_refused(factory, endpoint: str) -> None:
    """A blank address would otherwise become a request to nothing."""
    with pytest.raises(ValueError):
        factory(endpoint)


# -- the properties -----------------------------------------------------------


@pytest.mark.parametrize("factory", [Loams, AsyncLoams])
def test_repr_names_the_client_and_the_endpoint(factory) -> None:
    """`repr` is what a debugger and a log line show, so it has to say which
    endpoint -- two clients against two instances are otherwise indistinguishable."""
    client = factory(ENDPOINT)
    assert ENDPOINT in repr(client)
    assert factory.__name__ in repr(client)


@pytest.mark.parametrize("factory", [Loams, AsyncLoams])
def test_the_proto_revision_is_the_generated_one(factory) -> None:
    client = factory(ENDPOINT)
    assert client.proto_rev == PROTO_REV, (
        "the client reported a revision other than the one it was generated from, "
        "which is the comparison R9 exists to make"
    )


@pytest.mark.parametrize("factory", [Loams, AsyncLoams])
def test_the_proto_packages_are_the_loams_ones(factory) -> None:
    client = factory(ENDPOINT)
    packages = client.proto_packages
    assert packages, "a generated SDK reported no proto packages"
    assert all(name.startswith("loams.") for name in packages), (
        f"a non-Loams package leaked into {packages}"
    )
    assert set(packages) <= set(PROTO_PACKAGES), (
        f"{set(packages) - set(PROTO_PACKAGES)} is not in the generated table"
    )
    assert packages == tuple(name for name in PROTO_PACKAGES if name.startswith("loams."))


@pytest.mark.parametrize("factory", [Loams, AsyncLoams])
def test_consistency_is_off_unless_it_was_asked_for(factory) -> None:
    """`None` when off, so a caller can tell "off" from "on but empty" -- which is
    the state it is stuck in today."""
    assert factory(ENDPOINT).consistency is None
    assert factory(ENDPOINT, session_consistency=True).consistency is not None


@pytest.mark.parametrize("factory", [Loams, AsyncLoams])
def test_the_session_store_is_empty_on_both_clients(factory) -> None:
    """The transport gap, from the caller's side rather than the store's.

    Turned on, the store exists and holds nothing, which is what the two
    docstrings say and what `tests/test_consistency.py` explains.
    """
    session = factory(ENDPOINT, session_consistency=True).consistency
    assert session is not None
    assert session.current() is None, "a session token arrived from nowhere"


# -- the binding lookup -------------------------------------------------------


@pytest.mark.parametrize("factory", [Loams, AsyncLoams])
def test_binding_names_the_rpc_it_will_call(factory) -> None:
    client = factory(ENDPOINT)
    binding = client.binding("live", "watch")
    assert binding.rpc == "loams.live.v1.LiveService/Watch", (
        f"the binding named {binding.rpc!r}, which is not the RPC"
    )
    assert binding.streaming == "server"


@pytest.mark.parametrize("factory", [Loams, AsyncLoams])
def test_binding_refuses_a_module_the_generator_has_not_seen(factory) -> None:
    """`loams.vector` does not exist until its proto carries facade options, so a
    caller reaching for it gets a `LoamsError`, not an `AttributeError`."""
    client = factory(ENDPOINT)
    with pytest.raises(LoamsError):
        client.binding("vector", "search")
    with pytest.raises(LoamsError):
        client.binding("live", "nosuchcall")


# -- invalidating the catalogue ------------------------------------------------


def test_invalidate_catalogue_delegates_to_the_system_api() -> None:
    """It is a one-line forward, but it is the escape hatch from a stale
    `GetInstance.services[]`, so it should be proven to reach the system API."""
    calls: list[int] = []
    client = Loams(ENDPOINT)
    original = client.system.catalogue

    def counting() -> object:
        calls.append(1)
        return original

    client.system.catalogue = counting  # type: ignore[method-assign]
    client.invalidate_catalogue()
    client.system.catalogue()
    assert calls == [1], "invalidate_catalogue did not reach loams.system"


# -- pagination, before there is anything to paginate ------------------------


def _refusal(call) -> str:
    """The message from whatever `call` raises, sync or async.

    The two clients raise the same errors from the same places, and the async one
    raises inside a coroutine, so a test that wants the message rather than the
    class has to drive both.
    """
    try:
        result = call()
        if asyncio.iscoroutine(result):
            asyncio.run(result)
    except (LoamsError, ValueError) as error:
        return str(error)
    pytest.fail("the call did not refuse")


@pytest.mark.parametrize("factory", [Loams, AsyncLoams])
def test_paginate_refuses_an_unknown_module(factory) -> None:
    client = factory(ENDPOINT)
    request = GetInstanceRequest()
    message = _refusal(lambda: client.paginate("nosuchmodule", "get_instance", request))
    assert "nosuchmodule" in message, f"the refusal does not name the module: {message}"


@pytest.mark.parametrize("factory", [Loams, AsyncLoams])
def test_paginate_refuses_a_call_the_binding_does_not_name(factory) -> None:
    """No RPC is paged yet, so every binding is unpaged and the refusal is the
    whole behaviour a caller can meet today.

    The message names the call, because `paginate` takes a module and a call as
    strings and a typo in either is otherwise invisible.
    """
    client = factory(ENDPOINT)
    request = GetInstanceRequest()

    async def drain() -> None:
        async for _ in client.paginate("instance", "get_instance", request):
            pass

    message = _refusal(lambda: drain() if factory is AsyncLoams else list(
        client.paginate("instance", "get_instance", request)
    ))
    assert "loams.instance.get_instance" in message, f"the refusal does not name the call: {message}"


# -- the stream wirings, refused ----------------------------------------------


@pytest.mark.parametrize("factory", [Loams, AsyncLoams])
def test_stream_refuses_an_unknown_module_and_call(factory) -> None:
    client = factory(ENDPOINT)
    request = WatchRequest()
    if factory is Loams:
        for module, call, expected in (
            ("nosuchmodule", "watch", "nosuchmodule"),
            ("live", "nosuchcall", "nosuchcall"),
        ):
            with pytest.raises(LoamsError) as caught:
                client.stream(module, call, request)
            assert expected in str(caught.value)
        return

    async def walk() -> None:
        with pytest.raises(LoamsError) as by_module:
            client.stream("nosuchmodule", "watch", request)
        with pytest.raises(LoamsError) as by_call:
            client.stream("live", "nosuchcall", request)
        assert "nosuchmodule" in str(by_module.value)
        assert "nosuchcall" in str(by_call.value)

    asyncio.run(walk())