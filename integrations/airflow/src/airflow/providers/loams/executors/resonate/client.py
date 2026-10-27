# SPDX-License-Identifier: Apache-2.0
"""A small synchronous client for the Resonate server protocol (version 2026-04-01).

It speaks the same envelope the Resonate SDKs do: every request is
``POST {url}/`` with ``{"kind", "head": {"corrId", "version", "auth"?}, "data"}``
and every response is ``{"kind", "head": {"corrId", "status"}, "data"}``.
Workers receive ``execute`` messages as Server-Sent Events from
``GET {url}/poll/{group}/{pid}`` (the ``poll://`` scheme).

The client is deliberately narrow: the operations the executor and the worker
need, typed results, and two error classes:

- :class:`ResonateError` is an in-band answer (``head.status >= 400``) other
  than the ones each call documents as a normal outcome (404, 409).
- :class:`Unavailable` means the exchange did not complete. The request may or
  may not have been applied, so callers retry only idempotent operations.

Everything here is protocol-shaped; nothing is Airflow-specific, so a test can
replace :class:`HttpResonateClient` with a fake that satisfies
:class:`ResonateClient`.
"""

from __future__ import annotations

import base64
import json
import logging
import uuid
from collections.abc import Callable, Iterator
from dataclasses import dataclass, field
from typing import Any, Protocol

import httpx

logger = logging.getLogger(__name__)

PROTOCOL_VERSION = "2026-04-01"

#: Promise states, as the server spells them.
PENDING = "pending"
RESOLVED = "resolved"
REJECTED = "rejected"
REJECTED_CANCELED = "rejected_canceled"
REJECTED_TIMEDOUT = "rejected_timedout"
SETTLED_STATES = frozenset({RESOLVED, REJECTED, REJECTED_CANCELED, REJECTED_TIMEDOUT})

#: Task states.
TASK_PENDING = "pending"
TASK_ACQUIRED = "acquired"
TASK_SUSPENDED = "suspended"
TASK_HALTED = "halted"
TASK_FULFILLED = "fulfilled"


class ResonateError(Exception):
    """The server answered, and the answer was an error status."""

    def __init__(self, status: int, message: str, kind: str = "") -> None:
        super().__init__(f"{kind or 'resonate'}: {status} {message}")
        self.status = status
        self.message = message
        self.kind = kind


class Unavailable(Exception):
    """The exchange did not complete; the request may have been applied."""


# --------------------------------------------------------------------------
# Values and records
# --------------------------------------------------------------------------


def encode_value(payload: Any) -> dict[str, Any]:
    """A promise value on the wire: ``{"data": base64(json)}``."""
    if payload is None:
        return {}
    raw = json.dumps(payload, separators=(",", ":"), sort_keys=True).encode("utf-8")
    return {"data": base64.b64encode(raw).decode("ascii")}


def decode_value(value: dict[str, Any] | None) -> Any:
    """Invert :func:`encode_value`; ``None`` when the value carries no data."""
    if not value:
        return None
    data = value.get("data")
    if data is None:
        return None
    try:
        return json.loads(base64.b64decode(data))
    except (ValueError, TypeError) as exc:
        msg = "promise value is not base64-encoded JSON"
        raise ValueError(msg) from exc


@dataclass(frozen=True)
class PromiseRecord:
    id: str
    state: str
    param: dict[str, Any] = field(default_factory=dict)
    value: dict[str, Any] = field(default_factory=dict)
    tags: dict[str, str] = field(default_factory=dict)
    timeout_at: int = 0
    created_at: int = 0
    settled_at: int | None = None

    @property
    def pending(self) -> bool:
        return self.state == PENDING

    @property
    def settled(self) -> bool:
        return self.state in SETTLED_STATES

    def param_json(self) -> Any:
        return decode_value(self.param)

    def value_json(self) -> Any:
        return decode_value(self.value)

    @classmethod
    def from_wire(cls, raw: dict[str, Any]) -> PromiseRecord:
        return cls(
            id=raw["id"],
            state=raw["state"],
            param=raw.get("param") or {},
            value=raw.get("value") or {},
            tags=raw.get("tags") or {},
            timeout_at=int(raw.get("timeoutAt") or 0),
            created_at=int(raw.get("createdAt") or 0),
            settled_at=raw.get("settledAt"),
        )


@dataclass(frozen=True)
class TaskRecord:
    id: str
    state: str
    version: int
    pid: str | None = None
    ttl: int | None = None

    @classmethod
    def from_wire(cls, raw: dict[str, Any]) -> TaskRecord:
        return cls(
            id=raw["id"],
            state=raw["state"],
            version=int(raw.get("version") or 0),
            pid=raw.get("pid"),
            ttl=raw.get("ttl"),
        )


@dataclass(frozen=True)
class Acquired:
    """A claimed task: its record (with the new fencing version) and promise."""

    task: TaskRecord
    promise: PromiseRecord


@dataclass(frozen=True)
class ExecuteMessage:
    """An ``execute`` message pushed to a worker."""

    task_id: str
    version: int


# --------------------------------------------------------------------------
# The port
# --------------------------------------------------------------------------


class ResonateClient(Protocol):
    """The Resonate operations the executor and the worker use."""

    def promise_create(self, id: str, timeout_at: int, param: Any, tags: dict[str, str]) -> PromiseRecord:
        """Create a promise; idempotent: an existing promise comes back as is."""

    def promise_get(self, id: str) -> PromiseRecord | None:
        """The promise, or ``None`` if there is none."""

    def promise_settle(self, id: str, state: str, value: Any) -> PromiseRecord | None:
        """Settle a pending promise; a settled one comes back unchanged."""

    def task_get(self, id: str) -> TaskRecord | None:
        """The task of a promise, or ``None`` if there is none."""

    def task_acquire(self, id: str, version: int, pid: str, ttl_ms: int) -> Acquired | None:
        """Claim a task at ``version``; ``None`` if this attempt lost (409/404)."""

    def task_heartbeat(self, pid: str, task_id: str, version: int) -> None:
        """Extend the lease. Always succeeds by protocol design."""

    def task_fulfill(self, id: str, version: int, state: str, value: Any) -> bool:
        """Settle through the lease; ``False`` when the lease was lost (409)."""

    def task_release(self, id: str, version: int) -> bool:
        """Give the task back without settling it."""

    def task_halt(self, id: str) -> bool:
        """Stop the task's delivery; ``False`` when it was already fulfilled."""

    def poll(self, group: str, pid: str, stop: Callable[[], bool]) -> Iterator[ExecuteMessage]:
        """Yield ``execute`` messages for ``group`` until ``stop()`` is true."""

    def close(self) -> None:
        """Release connections."""


# --------------------------------------------------------------------------
# SSE framing
# --------------------------------------------------------------------------


class SseFramer:
    """Bytes in, ``data:`` payloads out; events end at a blank line."""

    def __init__(self) -> None:
        self._buf = ""

    def feed(self, chunk: str) -> list[str]:
        self._buf += chunk.replace("\r\n", "\n")
        out: list[str] = []
        while "\n\n" in self._buf:
            block, self._buf = self._buf.split("\n\n", 1)
            for line in block.split("\n"):
                if line.startswith("data:"):
                    payload = line[len("data:") :].strip()
                    if payload:
                        out.append(payload)
        return out


def parse_message(raw: str) -> ExecuteMessage | None:
    """An ``execute`` message, or ``None`` for anything else (``unblock``, junk)."""
    try:
        msg = json.loads(raw)
    except ValueError:
        logger.warning("dropping a push message that is not JSON")
        return None
    if not isinstance(msg, dict) or msg.get("kind") != "execute":
        return None
    task = (msg.get("data") or {}).get("task") or {}
    task_id = task.get("id")
    if not isinstance(task_id, str) or not task_id:
        return None
    return ExecuteMessage(task_id=task_id, version=int(task.get("version") or 0))


# --------------------------------------------------------------------------
# HTTP implementation
# --------------------------------------------------------------------------


class HttpResonateClient:
    """:class:`ResonateClient` over HTTP (``POST /`` and ``GET /poll``)."""

    def __init__(
        self,
        url: str,
        *,
        token: str | None = None,
        timeout: float = 10.0,
        transport: httpx.BaseTransport | None = None,
        corr_id: Callable[[], str] | None = None,
    ) -> None:
        self._url = url.rstrip("/")
        self._token = token or None
        headers = {"Content-Type": "application/json"}
        if self._token:
            headers["Authorization"] = f"Bearer {self._token}"
        self._http = httpx.Client(
            headers=headers,
            timeout=httpx.Timeout(timeout, read=None),
            transport=transport,
        )
        self._request_timeout = timeout
        self._corr_id = corr_id or (lambda: f"af-{uuid.uuid4().hex}")

    # -- envelope -----------------------------------------------------------

    def _send(self, kind: str, data: dict[str, Any]) -> tuple[int, Any]:
        corr = self._corr_id()
        head: dict[str, Any] = {"corrId": corr, "version": PROTOCOL_VERSION}
        if self._token:
            head["auth"] = self._token
        body = json.dumps({"kind": kind, "head": head, "data": data}, separators=(",", ":"))
        try:
            resp = self._http.post(f"{self._url}/", content=body, timeout=self._request_timeout)
        except httpx.HTTPError as exc:
            raise Unavailable(f"{kind}: {exc}") from exc
        if resp.status_code in (502, 503, 504):
            raise Unavailable(f"{kind}: HTTP {resp.status_code}")
        try:
            envelope = resp.json()
        except ValueError as exc:
            if resp.status_code >= 400:
                raise ResonateError(resp.status_code, resp.text[:200], kind) from exc
            raise Unavailable(f"{kind}: response is not JSON") from exc
        if not isinstance(envelope, dict):
            raise ResonateError(resp.status_code, "response is not an envelope", kind)
        head_in = envelope.get("head") or {}
        status = int(head_in.get("status") or resp.status_code)
        if envelope.get("kind") not in (kind, None, "") and status < 400:
            raise ResonateError(500, f"response kind {envelope.get('kind')!r}", kind)
        if head_in.get("corrId") not in (corr, None, ""):
            raise ResonateError(500, "response corrId mismatch", kind)
        return status, envelope.get("data")

    @staticmethod
    def _error(kind: str, status: int, data: Any) -> ResonateError:
        message = ""
        if isinstance(data, dict):
            message = str(data.get("message") or data.get("error") or "")
        elif isinstance(data, str):
            message = data
        return ResonateError(status, message, kind)

    # -- promises -----------------------------------------------------------

    def promise_create(self, id: str, timeout_at: int, param: Any, tags: dict[str, str]) -> PromiseRecord:
        data = {"id": id, "timeoutAt": timeout_at, "param": encode_value(param), "tags": tags}
        status, resp = self._send("promise.create", data)
        if status == 409:
            existing = self.promise_get(id)
            if existing is not None:
                return existing
        if status >= 400:
            raise self._error("promise.create", status, resp)
        return PromiseRecord.from_wire(resp["promise"])

    def promise_get(self, id: str) -> PromiseRecord | None:
        status, resp = self._send("promise.get", {"id": id})
        if status == 404:
            return None
        if status >= 400:
            raise self._error("promise.get", status, resp)
        return PromiseRecord.from_wire(resp["promise"])

    def promise_settle(self, id: str, state: str, value: Any) -> PromiseRecord | None:
        data = {"id": id, "state": state, "value": encode_value(value)}
        status, resp = self._send("promise.settle", data)
        if status == 404:
            return None
        if status >= 400:
            raise self._error("promise.settle", status, resp)
        return PromiseRecord.from_wire(resp["promise"])

    # -- tasks --------------------------------------------------------------

    def task_get(self, id: str) -> TaskRecord | None:
        status, resp = self._send("task.get", {"id": id})
        if status == 404:
            return None
        if status >= 400:
            raise self._error("task.get", status, resp)
        return TaskRecord.from_wire(resp["task"])

    def task_acquire(self, id: str, version: int, pid: str, ttl_ms: int) -> Acquired | None:
        data = {"id": id, "version": version, "pid": pid, "ttl": ttl_ms}
        status, resp = self._send("task.acquire", data)
        if status in (404, 409):
            return None
        if status >= 400:
            raise self._error("task.acquire", status, resp)
        return Acquired(
            task=TaskRecord.from_wire(resp["task"]),
            promise=PromiseRecord.from_wire(resp["promise"]),
        )

    def task_heartbeat(self, pid: str, task_id: str, version: int) -> None:
        data = {"pid": pid, "tasks": [{"id": task_id, "version": version}]}
        status, resp = self._send("task.heartbeat", data)
        if status >= 400:
            raise self._error("task.heartbeat", status, resp)

    def task_fulfill(self, id: str, version: int, state: str, value: Any) -> bool:
        action = {
            "kind": "promise.settle",
            "head": {"corrId": self._corr_id(), "version": PROTOCOL_VERSION},
            "data": {"id": id, "state": state, "value": encode_value(value)},
        }
        status, resp = self._send("task.fulfill", {"id": id, "version": version, "action": action})
        if status in (404, 409):
            return False
        if status >= 400:
            raise self._error("task.fulfill", status, resp)
        return True

    def task_release(self, id: str, version: int) -> bool:
        status, resp = self._send("task.release", {"id": id, "version": version})
        if status in (404, 409):
            return False
        if status >= 400:
            raise self._error("task.release", status, resp)
        return True

    def task_halt(self, id: str) -> bool:
        status, resp = self._send("task.halt", {"id": id})
        if status in (404, 409):
            return False
        if status >= 400:
            raise self._error("task.halt", status, resp)
        return True

    # -- push ---------------------------------------------------------------

    def poll(self, group: str, pid: str, stop: Callable[[], bool]) -> Iterator[ExecuteMessage]:
        """One SSE connection; returns when the server closes it or ``stop()``.

        Reconnection and backoff belong to the caller, which knows whether it
        is shutting down.
        """
        url = f"{self._url}/poll/{group}/{pid}"
        try:
            with self._http.stream("GET", url, headers={"Accept": "text/event-stream"}) as resp:
                if resp.status_code >= 400:
                    raise Unavailable(f"poll {group}: HTTP {resp.status_code}")
                framer = SseFramer()
                for chunk in resp.iter_text():
                    for payload in framer.feed(chunk):
                        msg = parse_message(payload)
                        if msg is not None:
                            yield msg
                    if stop():
                        return
        except httpx.HTTPError as exc:
            raise Unavailable(f"poll {group}: {exc}") from exc

    def close(self) -> None:
        self._http.close()
