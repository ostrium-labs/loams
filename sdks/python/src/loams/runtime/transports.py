"""Transports (design §44 §4, D612).

One port serves the Connect protocol, gRPC and gRPC-Web (D600), so the only
question a client has is which of those to speak. `connect-python` implements
all three behind one client, chosen with a `ProtocolType`:

- **Connect** (the default) is unary over `POST` and server streams over the
  Connect envelope, so a Connect unary call is an HTTP POST with a JSON body and
  `curl` works (design §44 §4). It is the default because it is the encoding the
  recorded conformance corpus holds for every RPC, and because it is what a proxy
  that logs bodies can read.
- **gRPC** is HTTP/2, which is the fastest of the three and the one Flight SQL
  uses. It needs an HTTP/2-capable peer; a proxy that terminates HTTP/1.1 cannot.
- **gRPC-Web** is the framing a browser sends. Python does not need it, but an
  edge that only speaks gRPC-Web does, so it is one argument away.

**Request compression is off by default.** `connect-python` gzips requests
unless told otherwise, and that is the wrong default for an SDK: it breaks any
peer or recording that compares request bytes — the conformance corpus does
exactly that — and it costs CPU on messages that are usually small. A caller who
wants it passes `send_compression`.
"""

from __future__ import annotations

from collections.abc import Iterable, Mapping
from typing import Final

from connectrpc.client import ConnectClient, ConnectClientSync
from connectrpc.compression import Compression
from connectrpc.protocol import ProtocolType

__all__ = [
    "PROTOCOLS",
    "TransportOptions",
    "make_async_client",
    "make_sync_client",
    "protocol_of",
]


class TransportOptions:
    """What a caller may configure on the transport."""

    def __init__(
        self,
        *,
        base_url: str | None = None,
        proto_json: bool = False,
        protocol: ProtocolType = ProtocolType.CONNECT,
        accept_compression: Iterable[Compression] | None = None,
        send_compression: Compression | None = None,
        timeout_ms: int | None = None,
        read_max_bytes: int | None = None,
    ) -> None:
        """:param base_url: the instance's URL, for example `https://acme.loams.dev`.

        A `Loams` sets this per client from its own `endpoint`, so a caller
        building a transport by hand rarely needs it.
        """
        self.base_url = base_url
        #: `True` is the proto3 JSON mapping, which is what `curl` sends;
        #: `False` (the default) is protobuf, which is smaller and needs no JSON
        #: codec on the wire. Both are in the conformance corpus.
        self.proto_json = proto_json
        self.protocol = protocol
        self.accept_compression = accept_compression
        self.send_compression = send_compression
        #: The deadline for a call that sets none, in milliseconds.
        self.timeout_ms = timeout_ms
        self.read_max_bytes = read_max_bytes

    def merge(self, base_url: str) -> TransportOptions:
        """The same options with the base URL filled in, for `Loams` to pass on."""
        if self.base_url is not None:
            return self
        return TransportOptions(
            base_url=base_url,
            proto_json=self.proto_json,
            protocol=self.protocol,
            accept_compression=self.accept_compression,
            send_compression=self.send_compression,
            timeout_ms=self.timeout_ms,
            read_max_bytes=self.read_max_bytes,
        )


#: The three protocols one Loams port speaks, by name, so `Loams(protocol="grpc")`
#: reads the way the design writes it.
PROTOCOLS: Final[dict[str, ProtocolType]] = {
    "connect": ProtocolType.CONNECT,
    "grpc": ProtocolType.GRPC,
    "grpc-web": ProtocolType.GRPC_WEB,
}


def protocol_of(name: str) -> ProtocolType:
    """The protocol a caller named, or a clear error."""
    try:
        return PROTOCOLS[name]
    except KeyError:
        known = ", ".join(sorted(PROTOCOLS))
        raise ValueError(f"unknown protocol {name!r}: expected one of {known}") from None


def _address(options: TransportOptions) -> str:
    if options.base_url is None:
        raise ValueError("the transport has no base_url; build the client through Loams")
    return options.base_url.rstrip("/")


def make_sync_client(options: TransportOptions) -> ConnectClientSync:
    """The default client: Connect over the protocol `connect-python` speaks."""
    return ConnectClientSync(
        _address(options),
        proto_json=options.proto_json,
        protocol=options.protocol,
        accept_compression=options.accept_compression,
        send_compression=options.send_compression,
        timeout_ms=options.timeout_ms,
        read_max_bytes=options.read_max_bytes,
    )


def make_async_client(options: TransportOptions) -> ConnectClient:
    """`make_sync_client` for `AsyncLoams`."""
    return ConnectClient(
        _address(options),
        proto_json=options.proto_json,
        protocol=options.protocol,
        accept_compression=options.accept_compression,
        send_compression=options.send_compression,
        timeout_ms=options.timeout_ms,
        read_max_bytes=options.read_max_bytes,
    )


def headers_of(extra: Mapping[str, str] | None) -> dict[str, str] | None:
    """The caller's headers as a plain dict, or `None`."""
    return dict(extra) if extra is not None else None
