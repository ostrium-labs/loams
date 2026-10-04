"""Shared fixtures for the Python SDK's tests.

The conformance corpus is served by the same Node fixture server the other
language SDKs use, so every SDK is replaying identical bytes rather than each
reimplementing the corpus. `LOAMS_TEST_ENDPOINT` points the tests at an already
running instance instead, which is how the suite runs against a live server.
"""

from __future__ import annotations

import json
import os
import pathlib
import shutil
import subprocess
import sys
from collections.abc import Iterator

import pytest

from loams import Loams

CONFORMANCE = pathlib.Path(__file__).resolve().parent.parent.parent / "conformance"
FIXTURES = pathlib.Path(__file__).resolve().parent.parent.parent / "fixtures"
SERVER_JS = CONFORMANCE / "fixture-server.mjs"


def _endpoint_from_live() -> str | None:
    live = os.environ.get("LOAMS_TEST_ENDPOINT", "").rstrip("/")
    return live or None


def _endpoint_from_node() -> tuple[str, subprocess.Popen[str]] | None:
    """Run the shared fixture server on an ephemeral port and read its URL.

    Returns the process too, so the caller can tear it down. Leaving the pipe
    open makes pytest's unraisable-exception check fail the run on a
    ResourceWarning for the socket, which says nothing about the SDK.
    """
    node = shutil.which("node")
    if node is None or not SERVER_JS.is_file():
        return None
    process = subprocess.Popen(
        [node, str(SERVER_JS), "--fixtures", str(FIXTURES), "--port", "0"],
        stdout=subprocess.PIPE,
        stderr=None,
        text=True,
    )
    assert process.stdout is not None
    # The server prints one JSON line describing what it bound, not a bare URL.
    line = process.stdout.readline().strip()
    try:
        url = json.loads(line)["url"]
    except (json.JSONDecodeError, KeyError):
        process.kill()
        return None
    return (url, process) if url else None


@pytest.fixture(scope="session")
def endpoint() -> Iterator[str]:
    live = _endpoint_from_live()
    if live is not None:
        yield live
        return
    started = _endpoint_from_node()
    if started is None:
        pytest.skip("no fixture server: set LOAMS_TEST_ENDPOINT or install node")
    url, process = started
    try:
        yield url
    finally:
        process.terminate()
        try:
            process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            process.kill()
        if process.stdout is not None:
            process.stdout.close()


@pytest.fixture
def client(endpoint: str) -> Iterator[Loams]:
    with Loams(endpoint) as sdk:
        yield sdk