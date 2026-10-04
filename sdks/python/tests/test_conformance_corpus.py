"""SDK2 Task 2's `python_conformance_all_required_fixtures`.

Drives the recorded corpus through the SDK and reports which required fixtures
were actually **run**, because `required.mjs` is explicit that `ran` is the only
thing that counts: "a test that passes without touching a required fixture has
not run it."

The interesting result here is not the pass count, it is the split. Fourteen of
the twenty-eight required fixtures target RPCs no SDK binds -- `DecideApproval`,
`SendTestNotification`, `WatchApprovals`, `ListApprovals` -- because only
instance and live carry the `loams.options.v1.module` / `.facade` options, so the
generator correctly emits no bindings for approvals, devices or notifications.
The Go facade binds the same seven methods and no more, and its `required.mjs`
entry is `verified: false` too, so this is not a Python gap.

So this test runs everything reachable, and pins the unreachable set exactly.
That second half is the point: when approvals/devices/notifications gain their
options, or the required list is scoped down, this test **fails** and says so,
rather than the gap quietly becoming permanent. It is also why no
`sdks/fixtures/results/python.json` is written -- the report would have to claim
14 fixtures that cannot run, and a skipped fixture is a failure to the runner
anyway.
"""

from __future__ import annotations

import json
import pathlib
from collections.abc import Iterator

import pytest

from loams import Loams, LoamsError
from loams._gen.facade import METHODS, MODULES

FIXTURES = pathlib.Path(__file__).resolve().parent.parent.parent / "fixtures"

#: The content types the corpus records, mapped to what the client must ask for.
#: `family()` in fixture-server.mjs groups both Connect encodings into one
#: family, so the encoding is the caller's choice even where the recording is
#: shared; the four encodings exist to catch an SDK that asks for the wrong one.
ENCODINGS = {
    "application/proto": ("connect", False),
    "application/json": ("connect", True),
    "application/grpc-web+proto": ("grpc-web", False),
    "application/grpc-web+json": ("grpc-web", True),
    "application/connect+proto": ("connect", False),
    "application/connect+json": ("connect", True),
}

#: Fixtures the SDK cannot call, and why. Pinned so the gap is visible rather
#: than inferred from a count.
UNREACHABLE_RPCS = {
    "loams.approvals.v1.ApprovalService/DecideApproval",
    "loams.approvals.v1.ApprovalService/WatchApprovals",
    "loams.approvals.v1.ApprovalService/ListApprovals",
    "loams.devices.v1.DeviceService/SendTestNotification",
}


def _required() -> list[str]:
    manifest = json.loads((FIXTURES / "manifest.json").read_text())
    return list(manifest["required"])


def _fixture(name: str) -> dict:
    """A recorded case, from either directory the corpus keeps them in."""
    for directory in ("", "apps-mock"):
        path = FIXTURES / "recorded" / directory / f"{name}.json"
        if path.is_file():
            return json.loads(path.read_text())
    raise AssertionError(f"no recorded fixture named {name}")


def _steps(fixture: dict) -> list[dict]:
    """A case's steps, or the single request/response pair a flat case has."""
    if fixture.get("steps"):
        return fixture["steps"]
    return [fixture]


def _bound_modules() -> dict[str, tuple[str, str]]:
    """`rpc path` -> `(module, call)` for every call the generated facade binds."""
    bound: dict[str, tuple[str, str]] = {}
    for module in MODULES:
        for call in module.calls:
            bound[call.rpc] = (module.name, call.name)
    return bound


#: Recorded cases this SDK cannot consume, and why. Each is a defect in the
#: recording, reported rather than edited -- see the commit message.
KNOWN_DEFECTIVE = {
    "live_watch": (
        "records a JSON end-stream body labelled application/connect+proto; "
        "family() hands that same case to proto clients, which cannot parse JSON "
        "as a proto EndStreamResponse"
    ),
}


def _consume(result: object) -> object:
    """A server stream is lazy, so calling it opens nothing until it is read."""
    if isinstance(result, Iterator):
        return list(result)
    return result


def _reachable() -> list[str]:
    """Required fixtures whose RPC the SDK binds."""
    bound = _bound_modules()
    reachable = []
    for name in _required():
        steps = _steps(_fixture(name))
        paths = {step["request"]["path"].lstrip("/") for step in steps}
        if paths & set(bound):
            reachable.append(name)
    return reachable


def test_python_conformance_all_required_fixtures(endpoint: str) -> None:
    """The required test: drive every recorded fixture the SDK can reach."""
    bound = _bound_modules()
    ran: set[str] = set()
    defective: list[str] = []
    unreachable: dict[str, set[str]] = {}

    for name in _required():
        fixture = _fixture(name)
        steps = _steps(fixture)
        paths = {step["request"]["path"].lstrip("/") for step in steps}
        if not (paths & set(bound)):
            unreachable[name] = paths
            continue

        if name in KNOWN_DEFECTIVE:
            defective.append(name)
            continue

        for step in steps:
            rpc = step["request"]["path"].lstrip("/")
            if rpc not in bound:
                continue
            module_name, call_name = bound[rpc]
            method = METHODS[rpc]
            content_type = step["request"]["headers"].get("content-type", "")
            assert content_type in ENCODINGS, (
                f"{name} records content-type {content_type!r}, which no client can ask for"
            )
            protocol, proto_json = ENCODINGS[content_type]
            request = method.input()
            expect = step.get("expect", {})
            want_reason = expect.get("reason")
            with Loams(endpoint, protocol=protocol, proto_json=proto_json) as client:
                module = getattr(client, module_name)
                if want_reason is None:
                    _consume(getattr(module, call_name)(request))
                else:
                    # A recorded refusal is the case working, not the case
                    # failing: `WhoAmI` is recorded as unimplemented and
                    # `Query` as not-in-variant precisely so an SDK has to read
                    # the reason rather than assume a body.
                    try:
                        _consume(getattr(module, call_name)(request))
                    except LoamsError as thrown:
                        caught = thrown
                    else:
                        raise AssertionError(
                            f"{name} ({content_type}, {call_name}) recorded "
                            f"{want_reason!r} but the call answered"
                        )
                    assert caught.reason == want_reason, (
                        f"{name} recorded reason {want_reason!r} but the SDK reported "
                        f"{caught.reason!r} ({type(caught).__name__})"
                    )
            ran.add(name)

    assert ran, "no required fixture was exercised, so this proves nothing"
    assert sorted(defective) == sorted(KNOWN_DEFECTIVE), (
        f"unexpected defective fixtures: {sorted(set(defective) - set(KNOWN_DEFECTIVE))}"
    )
    expected = {name for name in _reachable() if name not in KNOWN_DEFECTIVE}
    assert ran == expected, f"ran {sorted(ran)} but the reachable set is {sorted(expected)}"
    # `ran` is what a results report would carry. It is asserted rather than
    # written out, because writing it would claim coverage the SDK does not have.


def test_the_unreachable_fixtures_are_exactly_the_unbound_ones() -> None:
    """Pins the gap, so closing it fails this test instead of passing silently."""
    bound = _bound_modules()
    unreachable: dict[str, set[str]] = {}
    for name in _required():
        paths = {step["request"]["path"].lstrip("/") for step in _steps(_fixture(name))}
        if not (paths & set(bound)):
            unreachable[name] = paths

    assert unreachable, "every required fixture is now reachable; delete this test"
    offending: dict[str, set[str]] = {}
    for name, paths in unreachable.items():
        if not paths <= UNREACHABLE_RPCS:
            offending[name] = paths - UNREACHABLE_RPCS
    assert not offending, (
        f"required fixtures are unreachable for a reason this test does not know: {offending}. "
        "Either bind those RPCs or record why they cannot run."
    )


def test_no_results_report_is_written_while_fixtures_are_unreachable() -> None:
    """The report must not exist yet, and says why.

    `required.mjs` fails a fixture the language reports as skipped for any
    reason, so a report listing the 14 unreachable fixtures as skipped would fail
    the runner anyway -- and one claiming them as `ran` would be a lie.
    """
    report = FIXTURES / "results" / "python.json"
    bound = _bound_modules()
    unreachable = [
        name
        for name in _required()
        if not ({s["request"]["path"].lstrip("/") for s in _steps(_fixture(name))} & set(bound))
    ]
    if not unreachable:
        pytest.fail(
            f"every required fixture is reachable but {report} does not exist; "
            "write it, list all 28 in `ran`, and flip required.mjs to verified: true"
        )
    assert not report.exists(), (
        f"{report} exists while {len(unreachable)} required fixtures cannot run: "
        f"{unreachable}. A report must not claim them."
    )
