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


@pytest.mark.parametrize("factory", [Loams, AsyncLoams])
def test_invalidate_catalogue_delegates_to_the_system_api(factory) -> None:
    """It is a one-line forward, but it is the escape hatch from a stale
    `GetInstance.services[]`, so it should be proven to actually reach
    `loams.system`.

    Counting the `invalidate` call rather than a `catalogue()` call: `catalogue()`
    dials the instance, and a test about delegation should not need a server (or
    an expected connection-refused to swallow).
    """
    client = factory(ENDPOINT)
    calls: list[int] = []
    client.system.invalidate = lambda: calls.append(1)  # type: ignore[method-assign]
    client.invalidate_catalogue()
    assert calls == [1], "invalidate_catalogue did not reach loams.system"


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

# -- the convenience constructor paths -----------------------------------------
#
# Every other test in this suite authenticates with `auth=`. So the `api_key=`
# shortcut -- the argument most callers will actually use -- had never been
# constructed on either client, and neither had `__repr__` on a module, which is
# what a debugger shows.


@pytest.mark.parametrize("factory", [Loams, AsyncLoams])
def test_the_api_key_shortcut_builds_a_credential(factory) -> None:
    """`api_key="k"` is sugar for `auth=api_key("k")`, and it takes a different
    branch in the constructor: `auth` is None and `api_key` is not.

    Read through the protocol rather than called directly, because the two clients
    get different sources -- the async one is `_AsyncStatic`, whose `token` is a
    coroutine function. Calling it synchronously leaves an un-awaited coroutine,
    which pytest surfaces as an unraisable exception rather than a clean failure.
    """
    client = factory(ENDPOINT, api_key="k")
    assert client.consistency is None, "the shortcut also turned on a session"
    source = client._invoker._token_source
    assert source is not None, "api_key= built no token source at all"

    if factory is Loams:
        assert source.token() == "k", f"the shortcut produced {source.token()!r}"
        return

    async def read() -> str:
        token: str = await source.token()
        return token

    assert asyncio.run(read()) == "k"


@pytest.mark.parametrize("factory", [Loams, AsyncLoams])
def test_no_credential_at_all_builds_no_token_source(factory) -> None:
    """`auth=None` is a valid anonymous client, so this must not raise."""
    assert factory(ENDPOINT)._invoker._token_source is None


@pytest.mark.parametrize("factory", [Loams, AsyncLoams])
def test_a_module_repr_names_its_module_and_service(factory) -> None:
    """`repr(loams.live)` is what a debugger prints, and `tables` and `live` are two
    names over the same service -- so the repr has to carry both to tell them
    apart."""
    client = factory(ENDPOINT)
    assert repr(client.live) == "<loams.live (loams.live.v1.LiveService)>"
    assert repr(client.tables) == "<loams.tables (loams.live.v1.LiveService)>"
    assert repr(client.instance) == "<loams.instance (loams.instance.v1.InstanceService)>"


def test_binding_refuses_an_empty_module_name() -> None:
    """`client.binding("", "watch")` is a caller who forgot the module. The
    streaming branch lets an empty module through to the lookup, so this is the
    guard that stops it."""
    with Loams(ENDPOINT) as client:
        with pytest.raises(LoamsError) as caught:
            client.binding("", "watch")
        assert "module" in str(caught.value).lower()


@pytest.mark.parametrize("factory", [Loams, AsyncLoams])
def test_paginate_names_a_call_the_module_does_not_have(factory) -> None:
    """A real module with a call it does not expose. Distinct from the unknown
    *module* case, and it is what a typo in the call name produces."""
    client = factory(ENDPOINT)
    request = GetInstanceRequest()

    async def drain() -> None:
        async for _ in client.paginate("live", "nosuchcall", request):
            pass

    message = _refusal(lambda: drain() if factory is AsyncLoams else list(
        client.paginate("live", "nosuchcall", request)
    ))
    assert "nosuchcall" in message, f"the refusal does not name the call: {message}"
