"""Replays the SDKP app scenario (`sdks/fixtures/recorded/app/`) through the
**public facade**, sync and async, against the fixture server
(`sdks/fixtures/app-scenario.md` says the same in prose).

For each step: the facade call it names (`documents.write`, `vector.search`) is
made on a `Loams` (or `AsyncLoams`) client with the request the recording sent,
built with the builders when the step names a builder case, so the replay proves
the builder writes the recorded request. The fixture server compares the body
it receives with the recording, so a facade that sent anything else fails here.
The answer is compared with the recorded one through the message's JSON, or,
for a refusal, the typed error's reason and code with the recorded ones.
"""

from __future__ import annotations

import asyncio
import json
import pathlib
import re
from typing import Any

from google.protobuf import json_format
from google.protobuf.message import Message

from loams import AsyncLoams, Loams
from loams._gen.facade import MODULES, METHODS
from loams.runtime.errors import LoamsError
from loams.runtime.options import CallOptions

from test_builders import build_schema, build_search, evaluate

FIXTURES = pathlib.Path(__file__).resolve().parent.parent.parent / "fixtures"
BUILDERS = FIXTURES / "builders"


def app_fixture_names() -> list[str]:
    """The app fixtures the manifest names, in order."""
    manifest = json.loads((FIXTURES / "manifest.json").read_text())
    return [f["name"] for f in manifest["fixtures"] if f["name"].startswith("app_")]


def _fixture(name: str) -> dict[str, Any]:
    data: dict[str, Any] = json.loads((FIXTURES / "recorded" / "app" / f"{name}.json").read_text())
    return data


def _builder_case(name: str) -> dict[str, Any]:
    data: dict[str, Any] = json.loads((BUILDERS / f"{name}.json").read_text())
    return data


def _request_json(step: dict[str, Any]) -> dict[str, Any]:
    """The request JSON a step sends, built with the builders where it names one."""
    body: dict[str, Any] = json.loads(step["request"].get("body") or "{}")
    builder = step.get("builder")
    if builder is None:
        return body
    case = _builder_case(builder)
    if case["builder"] == "search":
        # The builder writes no namespace: the client's default fills it (R11).
        built: dict[str, Any] = build_search(case["spec"]).to_json()
        return built
    if case["builder"] == "schema":
        return {**body, "schema": build_schema(case["spec"])}
    return {**body, "where": evaluate(case["spec"])}


def _snake(name: str) -> str:
    return re.sub(r"([A-Z])", lambda m: "_" + m.group(1).lower(), name)


def _binding(call: str) -> tuple[str, Any]:
    module_name, _, call_name = call.rpartition(".")
    for module in MODULES:
        if module.name != module_name:
            continue
        for binding in module.calls:
            # The corpus spells calls in snake_case; an older annotation may
            # spell its name in camel case, so both are accepted.
            if call_name in (binding.proto_name, binding.name, _snake(binding.proto_name)):
                return module_name, binding
    raise AssertionError(f"the facade has no {call}")


def _code_name(error: LoamsError) -> str:
    code = error.code
    return str(getattr(code, "value", code)).lower()


class _Step:
    """One step, resolved: the facade call, its request and what it expects."""

    def __init__(self, name: str, at: int, step: dict[str, Any]) -> None:
        self.where = f"{name} step {at} ({step['call']})"
        self.module, self.binding = _binding(step["call"])
        method = METHODS.get(self.binding.rpc)
        if method is None:
            raise AssertionError(f"{self.where}: no generated method for {self.binding.rpc}")
        self.output = method.output
        self.request: Message = json_format.ParseDict(_request_json(step), method.input())
        self.options = CallOptions(
            headers={"loams-fixture-name": name, "loams-fixture-step": str(at)}
        )
        self.ok = step["expect"]["status"] == 200
        self.recorded = step["response"].get("body")
        self.reason = step["expect"].get("reason")

    def check_answer(self, answer: Any) -> None:
        got = json_format.MessageToDict(answer)
        want = json_format.MessageToDict(json_format.ParseDict(self.recorded or {}, self.output()))
        if got != want:
            raise AssertionError(f"{self.where}: answered {got!r:.300}, recorded {want!r:.300}")

    def check_refusal(self, error: BaseException) -> None:
        if not isinstance(error, LoamsError):
            raise AssertionError(f"{self.where}: expected a refusal, got {error!r}")
        if error.reason != self.reason:
            raise AssertionError(f"{self.where}: reason {error.reason}, recorded {self.reason}")
        recorded_code = (self.recorded or {}).get("code")
        if recorded_code is not None and _code_name(error) != recorded_code:
            raise AssertionError(f"{self.where}: code {_code_name(error)}, recorded {recorded_code}")


def _steps(name: str) -> list[_Step]:
    return [_Step(name, at, step) for at, step in enumerate(_fixture(name)["steps"])]


def replay_app_fixture(endpoint: str, name: str, *, namespace: str = "app") -> None:
    """One app fixture through the sync client; raises on the first step that
    does not hold, naming it."""
    with Loams(endpoint, namespace=namespace, proto_json=True, max_retries=0) as client:
        for step in _steps(name):
            fn = getattr(client.modules[step.module], step.binding.name)
            if step.ok:
                try:
                    answer = fn(step.request, step.options)
                except LoamsError as error:
                    raise AssertionError(f"{step.where}: {error}") from error
                step.check_answer(answer)
                continue
            try:
                fn(step.request, step.options)
            except BaseException as error:  # noqa: BLE001 - checked below
                step.check_refusal(error)
                continue
            raise AssertionError(f"{step.where}: expected a refusal, got an answer")


def replay_app_fixture_async(endpoint: str, name: str, *, namespace: str = "app") -> None:
    """The same, through `AsyncLoams`."""

    async def run() -> None:
        async with AsyncLoams(endpoint, namespace=namespace, proto_json=True, max_retries=0) as client:
            for step in _steps(name):
                fn = getattr(client.modules[step.module], step.binding.name)
                if step.ok:
                    try:
                        answer = await fn(step.request, step.options)
                    except LoamsError as error:
                        raise AssertionError(f"{step.where}: {error}") from error
                    step.check_answer(answer)
                    continue
                try:
                    await fn(step.request, step.options)
                except BaseException as error:  # noqa: BLE001 - checked below
                    step.check_refusal(error)
                    continue
                raise AssertionError(f"{step.where}: expected a refusal, got an answer")

    asyncio.run(run())


def drive_app_fixtures(endpoint: str) -> list[str]:
    """Every app fixture, both clients; the names of the ones that held."""
    ran: list[str] = []
    for name in app_fixture_names():
        replay_app_fixture(endpoint, name)
        replay_app_fixture_async(endpoint, name)
        ran.append(name)
    return ran
