"""Transport option plumbing, and the two paths that refuse to guess.

`loams.runtime.transports` decides which of Connect / gRPC / gRPC-Web a client
speaks and hands the result to `connect-python`. Coverage measured with stdlib
`trace --count` showed its error paths unrun -- and an error path is the one kind
of line that only matters when something is wrong, which is exactly when nobody
wants to discover it raises the wrong thing.

So the focus here is the refusals:

- an unknown protocol name lists the ones that exist, rather than a `KeyError`;
- a transport with no base URL says so, rather than handing `None` to
  `connect-python` and getting an `AttributeError` three frames deeper.

Plus `merge`, whose job is to let a `Loams` fill in its own address while a
caller who set one keeps it, and `headers_of`, which has to preserve the caller's
mapping rather than alias it -- the SDK hands that dict straight to
`connect-python`, which may keep it.
"""

from __future__ import annotations

import pytest

from connectrpc.client import ConnectClient, ConnectClientSync

from loams.runtime.transports import (
    PROTOCOLS,
    _address,
    TransportOptions,
    headers_of,
    make_async_client,
    make_sync_client,
    protocol_of,
)

# -- naming a protocol --------------------------------------------------------


def test_the_three_protocols_are_named_the_way_the_design_writes_them() -> None:
    assert set(PROTOCOLS) == {"connect", "grpc", "grpc-web"}
    assert protocol_of("connect") is PROTOCOLS["connect"]
    assert protocol_of("grpc") is PROTOCOLS["grpc"]
    assert protocol_of("grpc-web") is PROTOCOLS["grpc-web"]


def test_an_unknown_protocol_lists_the_ones_that_exist() -> None:
    """`KeyError` here names the dict, not the mistake. The message has to be
    actionable because a caller who typed `http2` needs to see what to type."""
    with pytest.raises(ValueError) as caught:
        protocol_of("http2")
    message = str(caught.value)
    assert "http2" in message, f"the error did not name what was asked for: {message}"
    for name in PROTOCOLS:
        assert name in message, f"the error did not list {name}: {message}"


def test_the_unknown_protocol_error_does_not_chain_a_key_error() -> None:
    """`from None`, so the traceback shows the one cause rather than two.

    A caller reading a traceback sees `KeyError` above the message that actually
    explains it, which is the more confusing of the two.
    """
    try:
        protocol_of("nope")
    except ValueError as error:
        assert error.__suppress_context__ is True, "the KeyError was left in the chain"
    else:  # pragma: no cover - protocol_of raised, so this cannot run
        pytest.fail("an unknown protocol did not raise")


# -- the base URL -------------------------------------------------------------


def test_a_transport_with_no_base_url_says_so() -> None:
    """`connect-python` would otherwise fail on `None` several frames deeper."""
    with pytest.raises(ValueError) as caught:
        make_sync_client(TransportOptions())
    assert "base_url" in str(caught.value), (
        f"the error does not name the missing field: {caught.value}"
    )


def test_a_trailing_slash_is_not_part_of_the_address() -> None:
    """`https://host/` and `https://host` address the same server.

    A doubled slash in the path is how that goes wrong: quietly, on every call,
    and as a 404 that reads like the RPC name is wrong rather than the address.
    """
    assert _address(TransportOptions(base_url="http://example.test:8080/")) == (
        "http://example.test:8080"
    )
    assert _address(TransportOptions(base_url="http://example.test:8080")) == (
        "http://example.test:8080"
    )


def test_merge_fills_in_the_clients_own_address() -> None:
    merged = TransportOptions(proto_json=True).merge("http://instance.test:9000")
    assert merged.base_url == "http://instance.test:9000"
    assert merged.proto_json is True, "merge dropped an option it was not asked to change"


def test_merge_leaves_a_base_url_the_caller_set() -> None:
    """A `Loams` merges its own endpoint in; it must not overwrite an explicit one."""
    options = TransportOptions(base_url="http://caller.test:1234")
    assert options.merge("http://client.test:9000") is options, (
        "merge rebuilt the options instead of keeping the caller's base_url"
    )
    assert options.base_url == "http://caller.test:1234"


def test_merge_carries_every_option_through() -> None:
    """The reason it is a method rather than a constructor call at the use site."""
    original = TransportOptions(
        proto_json=True,
        protocol=PROTOCOLS["grpc"],
        timeout_ms=1234,
        read_max_bytes=4096,
        accept_compression=("identity",),
        send_compression="gzip",
    )
    merged = original.merge("http://instance.test:9000")
    assert merged.protocol is original.protocol
    assert merged.timeout_ms == 1234
    assert merged.read_max_bytes == 4096
    assert merged.accept_compression == ("identity",)
    assert merged.send_compression == "gzip"


# -- building clients ---------------------------------------------------------


def test_the_sync_client_gets_the_address_and_the_options() -> None:
    client = make_sync_client(TransportOptions(base_url="http://instance.test:9000"))
    assert isinstance(client, ConnectClientSync)


def test_the_async_client_is_a_different_class() -> None:
    """Not a formality: the sync and async clients are separate implementations,
    and swapping one for the other would be an `await` on a sync result."""
    client = make_async_client(TransportOptions(base_url="http://instance.test:9000"))
    assert isinstance(client, ConnectClient)
    assert not isinstance(client, ConnectClientSync)


# -- caller headers -----------------------------------------------------------


def test_no_caller_headers_stays_none() -> None:
    """`None` and `{}` are not the same to `connect-python`: `None` means the
    client applies its own defaults, an empty dict means "no headers at all"."""
    assert headers_of(None) is None


def test_caller_headers_are_copied_not_aliased() -> None:
    """The dict goes straight to `connect-python`, which may hold it, so a later
    mutation by the caller would otherwise change the client's headers."""
    original = {"x-request-id": "abc"}
    handed_over = headers_of(original)
    assert handed_over == original
    assert handed_over is not original, "the caller's dict was aliased, not copied"
    original["x-request-id"] = "changed"
    assert handed_over["x-request-id"] == "abc", "the copy tracked a later mutation"