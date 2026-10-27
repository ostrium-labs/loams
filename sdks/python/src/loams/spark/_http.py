"""The HTTP layer under `loams.spark` (design 81, SP-b D2006 and D2007). **Preview.**

The Spark job facade is plain JSON over HTTP (`/v1/engines/spark/...`), not a
Connect service, so it does not go through the generated stubs. This module is
the whole transport: a one-method `Http` protocol a test can replace, a default
built on `urllib` so the `loams[spark]` extra adds no HTTP dependency, and a
small client that adds the bearer token, the namespace header and the mapping
of a non-2xx answer onto the SDK's typed errors (design 44 §7.4, D611).
"""

from __future__ import annotations

import json
import os
import urllib.error
import urllib.parse
import urllib.request
from collections.abc import Mapping
from typing import Final, Protocol

from connectrpc.code import Code

from loams.runtime.errors import (
    DeadlineExceededError,
    InternalError,
    InvalidArgumentError,
    LoamsError,
    NotFoundError,
    PermissionDeniedError,
    ResourceExhaustedError,
    UnauthenticatedError,
    UnavailableError,
    UnimplementedError,
)
from loams.runtime.token_source import ENV_ENDPOINT, env_token

__all__ = [
    "ENV_NAMESPACE",
    "NAMESPACE_HEADER",
    "Http",
    "JsonClient",
    "UrllibHttp",
    "error_for_status",
    "resolve_endpoint",
    "resolve_token",
]

#: The namespace the job runs in, when the caller does not pass one. The same
#: variable `Loams.from_env` reads.
ENV_NAMESPACE: Final[str] = "LOAMS_NAMESPACE"

#: The header the facade reads the namespace from. The Connect API carries the
#: namespace in the request body (R11); the JSON facade has no such field on
#: every route, so it travels as a header.
NAMESPACE_HEADER: Final[str] = "x-loams-namespace"

#: A request that hangs longer than this is a dead endpoint, not a slow job: the
#: facade answers at once and the job runs behind it.
_TIMEOUT_SECS: Final[float] = 30.0

#: The longest error body quoted in an exception message.
_MAX_MESSAGE: Final[int] = 500


class Http(Protocol):
    """One HTTP exchange. Returns the status, the headers and the body.

    A non-2xx status is returned, not raised: the caller maps it. A failure
    below HTTP (no route, refused, timed out) is raised.
    """

    def request(
        self, method: str, url: str, headers: Mapping[str, str], body: bytes | None
    ) -> tuple[int, dict[str, str], bytes]:
        """Sends one request and reads the whole answer."""
        ...


class UrllibHttp:
    """The default `Http`, on the standard library."""

    def __init__(self, timeout_secs: float = _TIMEOUT_SECS) -> None:
        """:param timeout_secs: the socket timeout of every request."""
        self._timeout = timeout_secs

    def request(
        self, method: str, url: str, headers: Mapping[str, str], body: bytes | None
    ) -> tuple[int, dict[str, str], bytes]:
        """Sends one request with `urllib.request.urlopen`."""
        scheme = urllib.parse.urlsplit(url).scheme
        if scheme not in ("http", "https"):
            raise InvalidArgumentError(
                f"loams.spark: endpoint must be http or https, got {scheme or 'no'} scheme",
                code=Code.INVALID_ARGUMENT,
            )
        req = urllib.request.Request(url, data=body, headers=dict(headers), method=method)
        try:
            with urllib.request.urlopen(req, timeout=self._timeout) as resp:
                return resp.status, dict(resp.headers.items()), resp.read()
        except urllib.error.HTTPError as err:
            with err:
                return err.code, dict(err.headers.items()), err.read()
        except (urllib.error.URLError, TimeoutError, OSError) as err:
            # A failure from below the API carries no reason (D611).
            raise UnavailableError(
                f"loams.spark: {method} {url}: {err}", code=Code.UNAVAILABLE
            ) from err


#: HTTP status to Connect code, for the statuses whose meaning is not in doubt.
_CODE_BY_STATUS: Final[dict[int, Code]] = {
    400: Code.INVALID_ARGUMENT,
    401: Code.UNAUTHENTICATED,
    403: Code.PERMISSION_DENIED,
    404: Code.NOT_FOUND,
    429: Code.RESOURCE_EXHAUSTED,
    500: Code.INTERNAL,
    501: Code.UNIMPLEMENTED,
    503: Code.UNAVAILABLE,
    504: Code.DEADLINE_EXCEEDED,
}

_CLASS_BY_CODE: Final[dict[Code, type[LoamsError]]] = {
    Code.INVALID_ARGUMENT: InvalidArgumentError,
    Code.UNAUTHENTICATED: UnauthenticatedError,
    Code.PERMISSION_DENIED: PermissionDeniedError,
    Code.NOT_FOUND: NotFoundError,
    Code.RESOURCE_EXHAUSTED: ResourceExhaustedError,
    Code.INTERNAL: InternalError,
    Code.UNIMPLEMENTED: UnimplementedError,
    Code.UNAVAILABLE: UnavailableError,
    Code.DEADLINE_EXCEEDED: DeadlineExceededError,
}


def error_for_status(status: int, body: bytes, *, rpc: str) -> LoamsError:
    """The typed error for a non-2xx answer.

    The message is the body's `message` (or `error`) field when the body is a
    JSON object, else the body text, truncated. A server reason is kept in
    `metadata["reason"]` rather than `reason`, because `reason` is typed by the
    registry and this facade's reasons are not registered yet.
    """
    code = _CODE_BY_STATUS.get(status, Code.UNKNOWN)
    cls = _CLASS_BY_CODE.get(code, LoamsError)
    text = body.decode("utf-8", errors="replace").strip()
    metadata = {"http_status": str(status)}
    message = text
    try:
        parsed: object = json.loads(text) if text else None
    except ValueError:
        parsed = None
    if isinstance(parsed, dict):
        found = parsed.get("message") or parsed.get("error")
        if isinstance(found, str) and found:
            message = found
        reason = parsed.get("reason")
        if isinstance(reason, str) and reason:
            metadata["reason"] = reason
    if len(message) > _MAX_MESSAGE:
        message = message[:_MAX_MESSAGE] + "..."
    return cls(
        f"{rpc}: HTTP {status}" + (f": {message}" if message else ""),
        code=code,
        metadata=metadata,
        rpc=rpc,
    )


def resolve_endpoint(endpoint: str | None) -> str:
    """The facade's base URL: the argument, else `LOAMS_ENDPOINT`."""
    value = (endpoint or os.environ.get(ENV_ENDPOINT, "")).strip().rstrip("/")
    if not value:
        raise InvalidArgumentError(
            "loams.spark: no endpoint: pass endpoint= or set LOAMS_ENDPOINT",
            code=Code.INVALID_ARGUMENT,
            metadata={"field": ENV_ENDPOINT},
        )
    return value


def resolve_token(token: str | None) -> str | None:
    """The bearer: the argument, else the environment.

    The environment is read the way the rest of the SDK reads it
    (`loams.runtime.token_source.env_token`: `LOAMS_API_KEY`, then
    `LOAMS_TOKEN`), so one process never authenticates as two principals.
    """
    if token:
        return token
    return env_token().token()


class JsonClient:
    """The facade client: base URL, bearer, namespace and an `Http`."""

    def __init__(
        self,
        endpoint: str | None = None,
        *,
        token: str | None = None,
        namespace: str | None = None,
        http: Http | None = None,
    ) -> None:
        """Resolves the endpoint, token and namespace once, at construction."""
        self.endpoint = resolve_endpoint(endpoint)
        self._token = resolve_token(token)
        self.namespace = namespace or os.environ.get(ENV_NAMESPACE) or None
        self._http: Http = http if http is not None else UrllibHttp()

    def _headers(self, accept: str, has_body: bool) -> dict[str, str]:
        headers = {"accept": accept, "user-agent": "loams-python/spark"}
        if has_body:
            headers["content-type"] = "application/json"
        if self._token:
            # Design 26 §9.2 and SDK runtime contract R1: the token travels in
            # Authorization and never in a URL.
            headers["authorization"] = f"Bearer {self._token}"
        if self.namespace:
            headers[NAMESPACE_HEADER] = self.namespace
        return headers

    def call(
        self,
        method: str,
        path: str,
        *,
        json_body: object | None = None,
        query: Mapping[str, str] | None = None,
        accept: str = "application/json",
    ) -> bytes:
        """One request; returns the body of a 2xx answer, raises otherwise."""
        url = self.endpoint + path
        if query:
            url += "?" + urllib.parse.urlencode(dict(query))
        body = None if json_body is None else json.dumps(json_body).encode("utf-8")
        status, _headers, payload = self._http.request(
            method, url, self._headers(accept, body is not None), body
        )
        if not 200 <= status < 300:
            raise error_for_status(status, payload, rpc=f"{method} {path}")
        return payload

    def json(
        self,
        method: str,
        path: str,
        *,
        json_body: object | None = None,
        query: Mapping[str, str] | None = None,
    ) -> object:
        """`call`, with the answer parsed as JSON (`None` for an empty body)."""
        payload = self.call(method, path, json_body=json_body, query=query)
        if not payload.strip():
            return None
        try:
            parsed: object = json.loads(payload)
        except ValueError as err:
            raise InternalError(
                f"{method} {path}: the answer is not JSON", code=Code.INTERNAL, rpc=path
            ) from err
        return parsed
