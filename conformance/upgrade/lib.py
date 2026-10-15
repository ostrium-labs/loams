"""What every upgrade fixture shares (UP1 Task 19, design §54 §14): the
stack's endpoints, a small HTTP and Connect client on the standard library,
and polling.

The fixtures run where the engine's loopback listeners are reachable: the
gate starts them in a container that shares the `loams` container's network
namespace (`--network container:<loams>`), so every default below is the
engine's own default address.
"""

from __future__ import annotations

import base64
import json
import struct
import time
import urllib.error
import urllib.request
from dataclasses import dataclass, field

# The namespace and names every fixture writes under.
NS = "up1fx"


class Skip(Exception):
    """The product is not in this release (or not in this stack): recorded, not failed."""


@dataclass
class Http:
    """A JSON-over-HTTP client with the stack's API key."""

    base: str
    key: str | None = None
    timeout: float = 30.0

    def request(self, method: str, path: str, body=None, headers=None, auth=True):
        data = None if body is None else json.dumps(body).encode()
        req = urllib.request.Request(self.base.rstrip("/") + path, data=data, method=method)
        req.add_header("content-type", "application/json")
        if auth and self.key:
            req.add_header("authorization", f"Bearer {self.key}")
            # The Qdrant API's own header.
            req.add_header("api-key", self.key)
        for k, v in (headers or {}).items():
            req.add_header(k, v)
        try:
            with urllib.request.urlopen(req, timeout=self.timeout) as resp:
                raw = resp.read()
                return resp.status, (json.loads(raw) if raw.strip() else None)
        except urllib.error.HTTPError as e:
            raw = e.read()
            try:
                return e.code, json.loads(raw) if raw.strip() else None
            except ValueError:
                return e.code, raw.decode(errors="replace")

    def get(self, path, **kw):
        return self.request("GET", path, **kw)

    def post(self, path, body=None, **kw):
        return self.request("POST", path, body if body is not None else {}, **kw)

    def put(self, path, body=None, **kw):
        return self.request("PUT", path, body if body is not None else {}, **kw)

    def ok(self, method, path, body=None, accept=(200, 201)):
        status, reply = self.request(method, path, body)
        if status not in accept:
            raise AssertionError(f"{method} {path}: {status} {reply}")
        return reply

    def connect(self, rpc: str, body: dict):
        """A unary Connect call in JSON (`/<package>.<Service>/<Method>`)."""
        return self.request("POST", f"/{rpc}", body)

    def connect_stream(self, rpc: str, body: dict):
        """A server-streaming Connect call in JSON: the messages, then the end-of-stream error."""
        payload = json.dumps(body).encode()
        frame = struct.pack(">BI", 0, len(payload)) + payload
        req = urllib.request.Request(self.base.rstrip("/") + f"/{rpc}", data=frame, method="POST")
        req.add_header("content-type", "application/connect+json")
        if self.key:
            req.add_header("authorization", f"Bearer {self.key}")
        with urllib.request.urlopen(req, timeout=self.timeout) as resp:
            raw = resp.read()
        messages, end, at = [], None, 0
        while at + 5 <= len(raw):
            flags, size = struct.unpack(">BI", raw[at : at + 5])
            chunk = json.loads(raw[at + 5 : at + 5 + size] or b"{}")
            at += 5 + size
            if flags & 0x02:
                end = chunk.get("error")
            else:
                messages.append(chunk)
        return messages, end

    def reachable(self) -> bool:
        try:
            self.request("GET", "/", auth=False)
            return True
        except (urllib.error.URLError, OSError):
            return False


@dataclass
class Stack:
    """Where the stack answers. Defaults are the engine's own loopback addresses."""

    engine: str = "http://127.0.0.1:8080"
    qdrant: str = "http://127.0.0.1:6333"
    es: str = "http://127.0.0.1:9200"
    durable: str = "http://127.0.0.1:8001"
    tidb: str = "tidb:4000"
    pg: str | None = None
    key: str | None = None
    instance: dict = field(default_factory=dict)

    def http(self, base: str | None = None) -> Http:
        return Http(base or self.engine, self.key)

    def serves(self, api: str) -> bool:
        return api in self.instance.get("apiVersions", [])

    def feature(self, name: str) -> bool:
        return bool(self.instance.get("features", {}).get(name))

    def read_instance(self):
        status, info = self.http().connect("loams.instance.v1.InstanceService/GetInstance", {})
        if status != 200:
            raise AssertionError(f"GetInstance: {status} {info}")
        self.instance = info
        return info


def until(what, check, within=60.0, every=0.5):
    """Polls `check()` until it returns a truthy value; that value."""
    deadline = time.monotonic() + within
    last = None
    while time.monotonic() < deadline:
        try:
            last = check()
            if last:
                return last
        except AssertionError as e:
            last = e
        time.sleep(every)
    raise AssertionError(f"timed out waiting until {what} (last: {last})")


def b64(text: str) -> str:
    return base64.b64encode(text.encode()).decode()


def unb64(text: str) -> str:
    return base64.b64decode(text).decode()
