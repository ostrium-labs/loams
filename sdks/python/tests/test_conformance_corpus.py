"""SDK2 Task 2's `python_conformance_all_required_fixtures`.

Drives the recorded corpus through the SDK and reports which required fixtures
were actually **run**, because `required.mjs` is explicit that `ran` is the only
thing that counts: "a test that passes without touching a required fixture has
not run it."

All 28 required fixtures are exercised through the Connect runtime and the
typed error hierarchy, meeting the 100% bar of design §44 §10.4 (D617).
"""

from __future__ import annotations

import base64
import json
import pathlib
from collections.abc import Iterator

import pytest
from google.protobuf import json_format
from google.protobuf.json_format import MessageToDict
from connectrpc.client import ConnectClientSync
from connectrpc.method import IdempotencyLevel, MethodInfo
from connectrpc.protocol import ProtocolType

from loams import Loams, LoamsError
from loams._gen.facade import METHODS, MODULES
from loams.instance.v1.instance_pb2 import (
    GetInstanceRequest,
    GetInstanceResponse,
    WhoAmIRequest,
    WhoAmIResponse,
)
from loams.live.v1.live_pb2 import (
    QueryRequest,
    QueryResponse,
    WatchRequest,
    Transition,
)
from loams.approvals.v1.approvals_pb2 import (
    DecideApprovalRequest,
    DecideApprovalResponse,
    WatchApprovalsRequest,
    WatchApprovalsResponse,
    ListApprovalsRequest,
    ListApprovalsResponse,
)
from loams.devices.v1.devices_pb2 import (
    SendTestNotificationRequest,
    SendTestNotificationResponse,
)
from loams.runtime.errors import to_loams_error

FIXTURES = pathlib.Path(__file__).resolve().parent.parent.parent / "fixtures"

#: The content types the corpus records, mapped to what the client must ask for.
ENCODINGS = {
    "application/proto": ("connect", False),
    "application/json": ("connect", True),
    "application/grpc-web+proto": ("grpc-web", False),
    "application/grpc-web+json": ("grpc-web", True),
    "application/connect+proto": ("connect", False),
    "application/connect+json": ("connect", True),
}

#: The required fixtures this SDK runs, named literally so
#: `check-languages.mjs` can see them.
REACHABLE_FIXTURES = (
    "instance_get_instance_grpc_web",
    "instance_get_instance_grpc_web_json",
    "instance_get_instance_json",
    "instance_get_instance_proto",
    "instance_who_am_i_grpc_web",
    "instance_who_am_i_grpc_web_json",
    "instance_who_am_i_json",
    "instance_who_am_i_proto",
    "live_query_grpc_web",
    "live_query_grpc_web_json",
    "live_query_json",
    "live_query_proto",
    "live_watch",
    "mock_error_approval_already_decided",
    "mock_error_approval_expired",
    "mock_error_approval_stale_revision",
    "mock_error_encodings",
    "mock_error_not_implemented",
    "mock_error_reason_required",
    "mock_error_requester_cannot_approve",
    "mock_error_step_up_required",
    "mock_state_idempotent_decide",
    "mock_state_stream_heartbeat",
    "mock_state_stream_resume",
    "mock_state_stream_resume_remove",
    "mock_state_stream_snapshot_reset",
    "mock_status_get_instance",
    "mock_status_unauthenticated",
)

RPC_INFOS: dict[str, tuple[MethodInfo, bool]] = {
    "/loams.instance.v1.InstanceService/GetInstance": (
        MethodInfo(
            name="GetInstance",
            service_name="loams.instance.v1.InstanceService",
            input=GetInstanceRequest,
            output=GetInstanceResponse,
            idempotency_level=IdempotencyLevel.NO_SIDE_EFFECTS,
        ),
        False,
    ),
    "/loams.instance.v1.InstanceService/WhoAmI": (
        MethodInfo(
            name="WhoAmI",
            service_name="loams.instance.v1.InstanceService",
            input=WhoAmIRequest,
            output=WhoAmIResponse,
            idempotency_level=IdempotencyLevel.NO_SIDE_EFFECTS,
        ),
        False,
    ),
    "/loams.live.v1.LiveService/Query": (
        MethodInfo(
            name="Query",
            service_name="loams.live.v1.LiveService",
            input=QueryRequest,
            output=QueryResponse,
            idempotency_level=IdempotencyLevel.NO_SIDE_EFFECTS,
        ),
        False,
    ),
    "/loams.live.v1.LiveService/Watch": (
        MethodInfo(
            name="Watch",
            service_name="loams.live.v1.LiveService",
            input=WatchRequest,
            output=Transition,
            idempotency_level=IdempotencyLevel.UNKNOWN,
        ),
        True,
    ),
    "/loams.approvals.v1.ApprovalService/DecideApproval": (
        MethodInfo(
            name="DecideApproval",
            service_name="loams.approvals.v1.ApprovalService",
            input=DecideApprovalRequest,
            output=DecideApprovalResponse,
            idempotency_level=IdempotencyLevel.UNKNOWN,
        ),
        False,
    ),
    "/loams.approvals.v1.ApprovalService/WatchApprovals": (
        MethodInfo(
            name="WatchApprovals",
            service_name="loams.approvals.v1.ApprovalService",
            input=WatchApprovalsRequest,
            output=WatchApprovalsResponse,
            idempotency_level=IdempotencyLevel.UNKNOWN,
        ),
        True,
    ),
    "/loams.approvals.v1.ApprovalService/ListApprovals": (
        MethodInfo(
            name="ListApprovals",
            service_name="loams.approvals.v1.ApprovalService",
            input=ListApprovalsRequest,
            output=ListApprovalsResponse,
            idempotency_level=IdempotencyLevel.NO_SIDE_EFFECTS,
        ),
        False,
    ),
    "/loams.devices.v1.DeviceService/SendTestNotification": (
        MethodInfo(
            name="SendTestNotification",
            service_name="loams.devices.v1.DeviceService",
            input=SendTestNotificationRequest,
            output=SendTestNotificationResponse,
            idempotency_level=IdempotencyLevel.UNKNOWN,
        ),
        False,
    ),
}


class _CompactProtoJSONCodec:
    """Proto3 JSON encoder that formats without whitespace between tokens."""

    def name(self) -> str:
        return "json"

    def encode(self, message: object) -> bytes:
        d = MessageToDict(message, use_integers_for_enums=False)  # type: ignore[arg-type]
        return json.dumps(d, separators=(",", ":")).encode("utf-8")

    def decode(self, data: bytes | bytearray, message: object) -> object:
        json_format.Parse(data, message, ignore_unknown_fields=True)  # type: ignore[arg-type]
        return message


def _decode_req(msg_cls: type, req_dict: dict, is_stream: bool) -> object:
    ct = (
        req_dict.get("headers", {})
        .get("content-type", "")
        .split(";")[0]
        .strip()
        .lower()
    )
    raw = b""
    if "bodyBase64" in req_dict and req_dict["bodyBase64"]:
        raw = base64.b64decode(req_dict["bodyBase64"])
    elif "body" in req_dict:
        b = req_dict["body"]
        raw = (
            b.encode("utf-8")
            if isinstance(b, str)
            else json.dumps(b).encode("utf-8")
        )
    is_framed = ("grpc-web" in ct) or ("connect" in ct and is_stream)
    payload = raw[5:] if is_framed and len(raw) >= 5 else raw
    msg = msg_cls()
    if ct.endswith("json"):
        if payload:
            json_format.Parse(payload.decode("utf-8"), msg, ignore_unknown_fields=True)
    else:
        if payload:
            msg.ParseFromString(payload)
    return msg


def _required() -> list[str]:
    manifest = json.loads((FIXTURES / "manifest.json").read_text())
    return [f["name"] for f in manifest["fixtures"] if f.get("required")]


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


def test_python_conformance_all_required_fixtures(endpoint: str) -> None:
    """The required test: drive every recorded fixture and write the results report."""
    clients: dict[tuple[ProtocolType, bool], ConnectClientSync] = {}

    def get_client(protocol: ProtocolType, proto_json: bool) -> ConnectClientSync:
        key = (protocol, proto_json)
        if key not in clients:
            c = ConnectClientSync(
                endpoint,
                protocol=protocol,
                proto_json=proto_json,
                send_compression=None,
            )
            if proto_json:
                c._codec = _CompactProtoJSONCodec()  # type: ignore[assignment]
            clients[key] = c
        return clients[key]

    required_names = _required()
    ran: list[str] = []
    failures: list[str] = []

    for fix_name in required_names:
        fix_data = _fixture(fix_name)
        steps = _steps(fix_data)
        answers: list[bytes | None] = []
        fixture_failed = False

        for idx, step in enumerate(steps):
            rpc_path = step["request"]["path"]
            if rpc_path not in RPC_INFOS:
                failures.append(f"{fix_name} step {idx}: unmapped RPC {rpc_path}")
                fixture_failed = True
                continue

            method_info, is_stream = RPC_INFOS[rpc_path]
            ct = (
                step["request"]["headers"]
                .get("content-type", "")
                .split(";")[0]
                .strip()
                .lower()
            )
            prot = ProtocolType.GRPC_WEB if "grpc-web" in ct else ProtocolType.CONNECT
            is_json = ct.endswith("json")
            client = get_client(prot, is_json)
            req_msg = _decode_req(method_info.input, step["request"], is_stream)

            headers = {
                "loams-fixture-name": fix_name,
                "loams-fixture-step": str(idx),
            }
            if "authorization" in step["request"].get("headers", {}):
                headers["authorization"] = step["request"]["headers"]["authorization"]

            expect = step.get("expect", {})
            try:
                if is_stream:
                    msgs = list(
                        client.execute_server_stream(
                            request=req_msg,  # type: ignore[arg-type]
                            method=method_info,
                            headers=headers,
                        )
                    )
                    answers.append(b"".join(m.SerializeToString() for m in msgs))
                    if "frames" in expect and len(msgs) != expect["frames"]:
                        failures.append(
                            f"{fix_name} step {idx}: expected {expect['frames']} frames, got {len(msgs)}"
                        )
                        fixture_failed = True
                else:
                    resp = client.execute_unary(
                        request=req_msg,  # type: ignore[arg-type]
                        method=method_info,
                        headers=headers,
                    )
                    answers.append(resp.SerializeToString())
                    if "reason" in expect and expect["reason"] is not None:
                        failures.append(
                            f"{fix_name} step {idx}: expected error reason {expect['reason']}, but succeeded"
                        )
                        fixture_failed = True
            except Exception as e:
                loams_err = to_loams_error(e, rpc_path)
                answers.append(None)
                if "reason" in expect:
                    want_reason = expect["reason"]
                    if want_reason is not None and loams_err.reason != want_reason:
                        failures.append(
                            f"{fix_name} step {idx}: expected reason {want_reason}, got {loams_err.reason}"
                        )
                        fixture_failed = True
                else:
                    if is_stream and step.get("response", {}).get("truncated", False):
                        pass
                    else:
                        failures.append(f"{fix_name} step {idx}: unexpected error {e}")
                        fixture_failed = True

            if not fixture_failed and "identicalToStep" in expect:
                earlier = answers[expect["identicalToStep"]]
                if earlier != answers[-1]:
                    failures.append(
                        f"{fix_name} step {idx}: identicalToStep {expect['identicalToStep']} mismatch"
                    )
                    fixture_failed = True

        if not fixture_failed:
            ran.append(fix_name)

    if failures:
        pytest.fail(f"Conformance run failed with {len(failures)} issue(s):\n  " + "\n  ".join(failures))

    assert len(ran) == len(required_names), (
        f"Ran {len(ran)} of {len(required_names)} required fixtures. Missing: {set(required_names) - set(ran)}"
    )

    # Write results report (D640)
    results_dir = FIXTURES / "results"
    results_dir.mkdir(parents=True, exist_ok=True)
    report_file = results_dir / "python.json"
    report_data = {
        "about": "What this SDK's suite ran.",
        "language": "python",
        "transport": "connect",
        "live": False,
        "endpoint": endpoint,
        "tests": [
            "python_conformance_all_required_fixtures",
            "python_retry_reuses_idempotency_key",
            "python_error_reason_mapping",
            "python_stream_resume_with_cursor",
            "python_token_source_refresh",
            "python_pagination_iterator",
        ],
        "ran": sorted(ran),
        "skipped": [],
    }
    report_file.write_text(json.dumps(report_data, indent=2) + "\n")


def test_the_named_lists_match_the_manifest() -> None:
    """Keeps the literal lists above honest, in both directions."""
    required = set(_required())
    named = set(REACHABLE_FIXTURES)
    assert named == required, (
        f"REACHABLE_FIXTURES mismatch with required set: "
        f"extra: {named - required}, missing: {required - named}"
    )
