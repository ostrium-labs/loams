# SPDX-License-Identifier: Apache-2.0
"""A small Connect-protocol client, JSON encoding, sync and async.

The Loams Python SDK covers the services it generates a facade for (instance,
streams, SQL, graph, ...). `loams.postgres.v1.PostgresService` (served by
`pg-control`) and `loams.house.v1.HouseService` (proxied by the engine's main
port) have no SDK module yet, so the hooks for them call the documented RPCs
directly, over the Connect protocol's JSON encoding:

- a unary call is `POST {base}/{package.Service}/{Method}` with
  `Content-Type: application/json` and the request message as proto3 JSON; the
  answer is the response message, or a Connect error object on a non-200;
- a server-streaming call sends `Content-Type: application/connect+json` and
  one enveloped request, and reads enveloped messages back: one flag byte, a
  4-byte big-endian length, then the JSON. Flag `0x02` marks the end-of-stream
  message, which carries the error, if any.

Requests are written in lowerCamelCase (the proto3 JSON default). Responses
are read with `field()`, which accepts either spelling, so the client does not
depend on how the server prints names.
"""

from __future__ import annotations

import json
import struct
from collections.abc import AsyncIterator, Iterator, Mapping
from dataclasses import dataclass
from dataclasses import field as dc_field
from typing import Any

import httpx

__all__ = [
    "AsyncConnectClient",
    "ConnectClient",
    "LoamsApiError",
    "camel",
    "field",
]

_END_STREAM = 0x02
_COMPRESSED = 0x01
_ENVELOPE = struct.Struct(">BI")


class LoamsApiError(Exception):
    """A Loams RPC failed.

    :param code: the Connect code (`not_found`, `unauthenticated`, ...), or
        `unavailable`/`unknown` for a failure below the API.
    :param reason: the `ErrorInfo.reason` the server sent, when it sent one
        (for example `house_query_not_found`).
    :param metadata: the `ErrorInfo.metadata`. Never secrets.
    """

    def __init__(
        self,
        message: str,
        *,
        code: str = "unknown",
        reason: str | None = None,
        metadata: Mapping[str, str] | None = None,
        rpc: str | None = None,
        http_status: int | None = None,
    ) -> None:
        super().__init__(message)
        self.message = message
        self.code = code
        self.reason = reason
        self.metadata: dict[str, str] = dict(metadata or {})
        self.rpc = rpc
        self.http_status = http_status

    def __str__(self) -> str:
        where = f" ({self.rpc})" if self.rpc else ""
        why = f" [{self.reason}]" if self.reason else ""
        return f"{self.code}{why}{where}: {self.message}"

    @property
    def not_found(self) -> bool:
        return self.code == "not_found"


def camel(name: str) -> str:
    """`query_id` -> `queryId` (proto3 JSON's lowerCamelCase)."""
    head, *rest = name.split("_")
    return head + "".join(part[:1].upper() + part[1:] for part in rest)


def field(message: Mapping[str, Any] | None, name: str, default: Any = None) -> Any:
    """Reads a proto3 JSON field by its proto name, in either spelling."""
    if not message:
        return default
    if name in message:
        return message[name]
    return message.get(camel(name), default)


def _status_code(http_status: int) -> str:
    # The Connect spec's HTTP-to-code mapping for errors without a body.
    return {
        400: "internal",
        401: "unauthenticated",
        403: "permission_denied",
        404: "unimplemented",
        408: "deadline_exceeded",
        409: "aborted",
        412: "failed_precondition",
        429: "unavailable",
        502: "unavailable",
        503: "unavailable",
        504: "unavailable",
    }.get(http_status, "unknown")


def _error_from_json(body: Mapping[str, Any], *, rpc: str, http_status: int | None) -> LoamsApiError:
    code = str(body.get("code") or (_status_code(http_status) if http_status else "unknown"))
    message = str(body.get("message") or "")
    reason: str | None = None
    metadata: dict[str, str] = {}
    for detail in body.get("details") or []:
        # A server that includes `debug` gives the detail decoded; the type is
        # loams.errors.v1.ErrorInfo or google.rpc.ErrorInfo (same shape).
        debug = detail.get("debug") if isinstance(detail, Mapping) else None
        if isinstance(debug, Mapping) and "reason" in debug:
            reason = str(debug["reason"])
            meta = debug.get("metadata") or {}
            if isinstance(meta, Mapping):
                metadata.update({str(k): str(v) for k, v in meta.items()})
            break
    return LoamsApiError(
        message or code, code=code, reason=reason, metadata=metadata, rpc=rpc, http_status=http_status
    )


def _error_from_response(response: httpx.Response, *, rpc: str) -> LoamsApiError:
    try:
        body = response.json()
    except (ValueError, json.JSONDecodeError):
        body = None
    if not isinstance(body, Mapping):
        return LoamsApiError(
            response.text[:500] or f"HTTP {response.status_code}",
            code=_status_code(response.status_code),
            rpc=rpc,
            http_status=response.status_code,
        )
    return _error_from_json(body, rpc=rpc, http_status=response.status_code)


def envelope(message: Mapping[str, Any]) -> bytes:
    """One enveloped JSON message (flags 0)."""
    payload = json.dumps(message, separators=(",", ":")).encode()
    return _ENVELOPE.pack(0, len(payload)) + payload


class EnvelopeReader:
    """Splits a byte stream into Connect envelopes.

    `feed()` takes bytes as they arrive and returns the messages they complete;
    `end` is the end-of-stream message once seen.
    """

    def __init__(self, *, rpc: str) -> None:
        self._buffer = bytearray()
        self._rpc = rpc
        self.end: dict[str, Any] | None = None

    def feed(self, chunk: bytes) -> list[dict[str, Any]]:
        self._buffer.extend(chunk)
        out: list[dict[str, Any]] = []
        while len(self._buffer) >= _ENVELOPE.size:
            flags, length = _ENVELOPE.unpack_from(self._buffer, 0)
            if len(self._buffer) < _ENVELOPE.size + length:
                break
            payload = bytes(self._buffer[_ENVELOPE.size : _ENVELOPE.size + length])
            del self._buffer[: _ENVELOPE.size + length]
            if flags & _COMPRESSED:
                raise LoamsApiError(
                    "the server sent a compressed message, which this client did not ask for",
                    code="internal",
                    rpc=self._rpc,
                )
            decoded = json.loads(payload) if payload else {}
            if flags & _END_STREAM:
                self.end = decoded
            else:
                out.append(decoded)
        return out

    def finish(self) -> None:
        """Raises the stream's error, or a protocol error if it never ended."""
        if self._buffer:
            raise LoamsApiError("the stream ended inside a message", code="internal", rpc=self._rpc)
        if self.end is None:
            raise LoamsApiError(
                "the stream ended without an end-of-stream message", code="unavailable", rpc=self._rpc
            )
        error = self.end.get("error")
        if error:
            raise _error_from_json(error, rpc=self._rpc, http_status=None)


@dataclass
class _Base:
    """What both clients share: the base URL, the bearer and the timeout."""

    base_url: str
    token: str | None = None
    timeout: float = 30.0
    verify: bool | str = True
    headers: dict[str, str] = dc_field(default_factory=dict)

    def _url(self, service: str, method: str) -> str:
        return f"{self.base_url.rstrip('/')}/{service}/{method}"

    def _headers(self, content_type: str, timeout_ms: int | None) -> dict[str, str]:
        headers = {
            "Content-Type": content_type,
            "Connect-Protocol-Version": "1",
            "User-Agent": "airflow-provider-loams",
            **self.headers,
        }
        if self.token:
            headers["Authorization"] = f"Bearer {self.token}"
        if timeout_ms:
            headers["Connect-Timeout-Ms"] = str(int(timeout_ms))
        return headers


class ConnectClient(_Base):
    """Blocking Connect client (JSON)."""

    def _client(self) -> httpx.Client:
        return httpx.Client(timeout=self.timeout, verify=self.verify)

    def call(
        self,
        service: str,
        method: str,
        request: Mapping[str, Any] | None = None,
        *,
        timeout_ms: int | None = None,
    ) -> dict[str, Any]:
        """One unary call; answers the response message as a dict."""
        rpc = f"{service}/{method}"
        with self._client() as client:
            try:
                response = client.post(
                    self._url(service, method),
                    content=json.dumps(dict(request or {}), separators=(",", ":")),
                    headers=self._headers("application/json", timeout_ms),
                )
            except httpx.TransportError as exc:
                raise LoamsApiError(str(exc) or type(exc).__name__, code="unavailable", rpc=rpc) from exc
        if response.status_code != 200:
            raise _error_from_response(response, rpc=rpc)
        body = response.json() if response.content else {}
        return dict(body) if isinstance(body, Mapping) else {}

    def stream(
        self,
        service: str,
        method: str,
        request: Mapping[str, Any] | None = None,
        *,
        timeout_ms: int | None = None,
    ) -> Iterator[dict[str, Any]]:
        """One server-streaming call; yields each response message as a dict."""
        rpc = f"{service}/{method}"
        reader = EnvelopeReader(rpc=rpc)
        with self._client() as client:
            try:
                with client.stream(
                    "POST",
                    self._url(service, method),
                    content=envelope(dict(request or {})),
                    headers=self._headers("application/connect+json", timeout_ms),
                ) as response:
                    if response.status_code != 200:
                        response.read()
                        raise _error_from_response(response, rpc=rpc)
                    for chunk in response.iter_bytes():
                        yield from reader.feed(chunk)
            except httpx.TransportError as exc:
                raise LoamsApiError(str(exc) or type(exc).__name__, code="unavailable", rpc=rpc) from exc
        reader.finish()


class AsyncConnectClient(_Base):
    """Asyncio Connect client (JSON), for triggers."""

    def _client(self) -> httpx.AsyncClient:
        return httpx.AsyncClient(timeout=self.timeout, verify=self.verify)

    async def call(
        self,
        service: str,
        method: str,
        request: Mapping[str, Any] | None = None,
        *,
        timeout_ms: int | None = None,
    ) -> dict[str, Any]:
        rpc = f"{service}/{method}"
        async with self._client() as client:
            try:
                response = await client.post(
                    self._url(service, method),
                    content=json.dumps(dict(request or {}), separators=(",", ":")),
                    headers=self._headers("application/json", timeout_ms),
                )
            except httpx.TransportError as exc:
                raise LoamsApiError(str(exc) or type(exc).__name__, code="unavailable", rpc=rpc) from exc
        if response.status_code != 200:
            raise _error_from_response(response, rpc=rpc)
        body = response.json() if response.content else {}
        return dict(body) if isinstance(body, Mapping) else {}

    async def stream(
        self,
        service: str,
        method: str,
        request: Mapping[str, Any] | None = None,
        *,
        timeout_ms: int | None = None,
    ) -> AsyncIterator[dict[str, Any]]:
        rpc = f"{service}/{method}"
        reader = EnvelopeReader(rpc=rpc)
        async with self._client() as client:
            try:
                async with client.stream(
                    "POST",
                    self._url(service, method),
                    content=envelope(dict(request or {})),
                    headers=self._headers("application/connect+json", timeout_ms),
                ) as response:
                    if response.status_code != 200:
                        await response.aread()
                        raise _error_from_response(response, rpc=rpc)
                    async for chunk in response.aiter_bytes():
                        for message in reader.feed(chunk):
                            yield message
            except httpx.TransportError as exc:
                raise LoamsApiError(str(exc) or type(exc).__name__, code="unavailable", rpc=rpc) from exc
        reader.finish()
