"""A fake engine for the fixtures' own tests: GetInstance and the collection
REST API, in memory (UP1 Task 19). `LOAMS_UPGRADE_ENGINE` (with
`LOAMS_API_KEY`) names a real stack for `write_then_check_on_same_release_passes`."""

from __future__ import annotations

import json
import pathlib
import socket
import sys
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

import pytest

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))


class FakeEngine:
    def __init__(self, api_versions):
        self.api_versions = api_versions
        self.docs: dict[int, dict] = {}

    def handle(self, path, body):
        if path == "/loams.instance.v1.InstanceService/GetInstance":
            return 200, {"serverVersion": "0.1.0-preview.2", "apiVersions": self.api_versions, "features": {}}
        if path == "/v1/namespaces" or path.endswith("/collections"):
            return 201, {"id": 1}
        if path.endswith("/documents"):
            for op in body["ops"]:
                self.docs[op["upsert"]["id"]] = op["upsert"]["source"]
            return 200, {}
        if path.endswith("/documents/get"):
            return 200, {"documents": [{"id": i, "source": self.docs[i]} for i in body["ids"] if i in self.docs]}
        if path.endswith("/query"):
            text = body["retrievers"][0]["text"]["query"]["match"]["text"]
            return 200, {"hits": [{"pk": i} for i, d in self.docs.items() if text in d["body"]]}
        return 404, {"error": path}


@pytest.fixture
def engine():
    fake = FakeEngine(["loams.instance.v1", "loams.collection.v1"])

    class Handler(BaseHTTPRequestHandler):
        def do_POST(self):
            raw = self.rfile.read(int(self.headers.get("content-length") or 0))
            status, reply = fake.handle(self.path, json.loads(raw or b"{}"))
            data = json.dumps(reply).encode()
            self.send_response(status)
            self.send_header("content-type", "application/json")
            self.send_header("content-length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)

        def log_message(self, *args):
            pass

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    fake.url = f"http://127.0.0.1:{server.server_address[1]}"
    yield fake
    server.shutdown()


@pytest.fixture
def closed_port():
    """A local address nothing listens on."""
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return f"http://127.0.0.1:{s.getsockname()[1]}"
