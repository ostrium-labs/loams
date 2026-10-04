import datetime

from google.protobuf import timestamp_pb2 as _timestamp_pb2
from loams.instance.v1 import instance_pb2 as _instance_pb2
from google.protobuf.internal import containers as _containers
from google.protobuf.internal import enum_type_wrapper as _enum_type_wrapper
from google.protobuf import descriptor as _descriptor
from google.protobuf import message as _message
from collections.abc import Iterable as _Iterable, Mapping as _Mapping
from typing import ClassVar as _ClassVar, Optional as _Optional, Union as _Union

DESCRIPTOR: _descriptor.FileDescriptor

class Risk(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    RISK_UNSPECIFIED: _ClassVar[Risk]
    RISK_LOW: _ClassVar[Risk]
    RISK_MEDIUM: _ClassVar[Risk]
    RISK_HIGH: _ClassVar[Risk]
    RISK_DESTRUCTIVE: _ClassVar[Risk]

class ApprovalState(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    APPROVAL_STATE_UNSPECIFIED: _ClassVar[ApprovalState]
    APPROVAL_STATE_PENDING: _ClassVar[ApprovalState]
    APPROVAL_STATE_APPROVED: _ClassVar[ApprovalState]
    APPROVAL_STATE_REJECTED: _ClassVar[ApprovalState]
    APPROVAL_STATE_EXPIRED: _ClassVar[ApprovalState]
    APPROVAL_STATE_CANCELED: _ClassVar[ApprovalState]

class StepUp(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    STEP_UP_UNSPECIFIED: _ClassVar[StepUp]
    STEP_UP_DEVICE: _ClassVar[StepUp]
    STEP_UP_SESSION: _ClassVar[StepUp]
    STEP_UP_NONE: _ClassVar[StepUp]

class DecisionKind(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    DECISION_KIND_UNSPECIFIED: _ClassVar[DecisionKind]
    DECISION_KIND_APPROVE: _ClassVar[DecisionKind]
    DECISION_KIND_REJECT: _ClassVar[DecisionKind]
RISK_UNSPECIFIED: Risk
RISK_LOW: Risk
RISK_MEDIUM: Risk
RISK_HIGH: Risk
RISK_DESTRUCTIVE: Risk
APPROVAL_STATE_UNSPECIFIED: ApprovalState
APPROVAL_STATE_PENDING: ApprovalState
APPROVAL_STATE_APPROVED: ApprovalState
APPROVAL_STATE_REJECTED: ApprovalState
APPROVAL_STATE_EXPIRED: ApprovalState
APPROVAL_STATE_CANCELED: ApprovalState
STEP_UP_UNSPECIFIED: StepUp
STEP_UP_DEVICE: StepUp
STEP_UP_SESSION: StepUp
STEP_UP_NONE: StepUp
DECISION_KIND_UNSPECIFIED: DecisionKind
DECISION_KIND_APPROVE: DecisionKind
DECISION_KIND_REJECT: DecisionKind

class Approval(_message.Message):
    __slots__ = ("id", "revision", "operation_id", "promise_id", "kind", "environment", "requested_by", "actor_chain", "summary", "detail_lines", "target", "risk", "policy", "decisions", "state", "created_at", "expires_at")
    class TargetEntry(_message.Message):
        __slots__ = ("key", "value")
        KEY_FIELD_NUMBER: _ClassVar[int]
        VALUE_FIELD_NUMBER: _ClassVar[int]
        key: str
        value: str
        def __init__(self, key: _Optional[str] = ..., value: _Optional[str] = ...) -> None: ...
    ID_FIELD_NUMBER: _ClassVar[int]
    REVISION_FIELD_NUMBER: _ClassVar[int]
    OPERATION_ID_FIELD_NUMBER: _ClassVar[int]
    PROMISE_ID_FIELD_NUMBER: _ClassVar[int]
    KIND_FIELD_NUMBER: _ClassVar[int]
    ENVIRONMENT_FIELD_NUMBER: _ClassVar[int]
    REQUESTED_BY_FIELD_NUMBER: _ClassVar[int]
    ACTOR_CHAIN_FIELD_NUMBER: _ClassVar[int]
    SUMMARY_FIELD_NUMBER: _ClassVar[int]
    DETAIL_LINES_FIELD_NUMBER: _ClassVar[int]
    TARGET_FIELD_NUMBER: _ClassVar[int]
    RISK_FIELD_NUMBER: _ClassVar[int]
    POLICY_FIELD_NUMBER: _ClassVar[int]
    DECISIONS_FIELD_NUMBER: _ClassVar[int]
    STATE_FIELD_NUMBER: _ClassVar[int]
    CREATED_AT_FIELD_NUMBER: _ClassVar[int]
    EXPIRES_AT_FIELD_NUMBER: _ClassVar[int]
    id: str
    revision: int
    operation_id: str
    promise_id: str
    kind: str
    environment: _instance_pb2.Environment
    requested_by: _instance_pb2.Principal
    actor_chain: _containers.RepeatedCompositeFieldContainer[_instance_pb2.Principal]
    summary: str
    detail_lines: _containers.RepeatedScalarFieldContainer[str]
    target: _containers.ScalarMap[str, str]
    risk: Risk
    policy: ApprovalPolicy
    decisions: _containers.RepeatedCompositeFieldContainer[Decision]
    state: ApprovalState
    created_at: _timestamp_pb2.Timestamp
    expires_at: _timestamp_pb2.Timestamp
    def __init__(self, id: _Optional[str] = ..., revision: _Optional[int] = ..., operation_id: _Optional[str] = ..., promise_id: _Optional[str] = ..., kind: _Optional[str] = ..., environment: _Optional[_Union[_instance_pb2.Environment, _Mapping]] = ..., requested_by: _Optional[_Union[_instance_pb2.Principal, _Mapping]] = ..., actor_chain: _Optional[_Iterable[_Union[_instance_pb2.Principal, _Mapping]]] = ..., summary: _Optional[str] = ..., detail_lines: _Optional[_Iterable[str]] = ..., target: _Optional[_Mapping[str, str]] = ..., risk: _Optional[_Union[Risk, str]] = ..., policy: _Optional[_Union[ApprovalPolicy, _Mapping]] = ..., decisions: _Optional[_Iterable[_Union[Decision, _Mapping]]] = ..., state: _Optional[_Union[ApprovalState, str]] = ..., created_at: _Optional[_Union[datetime.datetime, _timestamp_pb2.Timestamp, _Mapping]] = ..., expires_at: _Optional[_Union[datetime.datetime, _timestamp_pb2.Timestamp, _Mapping]] = ...) -> None: ...

class ApprovalPolicy(_message.Message):
    __slots__ = ("required_approvals", "approver_roles", "requester_may_approve", "step_up")
    REQUIRED_APPROVALS_FIELD_NUMBER: _ClassVar[int]
    APPROVER_ROLES_FIELD_NUMBER: _ClassVar[int]
    REQUESTER_MAY_APPROVE_FIELD_NUMBER: _ClassVar[int]
    STEP_UP_FIELD_NUMBER: _ClassVar[int]
    required_approvals: int
    approver_roles: _containers.RepeatedScalarFieldContainer[str]
    requester_may_approve: bool
    step_up: StepUp
    def __init__(self, required_approvals: _Optional[int] = ..., approver_roles: _Optional[_Iterable[str]] = ..., requester_may_approve: _Optional[bool] = ..., step_up: _Optional[_Union[StepUp, str]] = ...) -> None: ...

class Decision(_message.Message):
    __slots__ = ("by", "decision", "reason", "at", "device")
    BY_FIELD_NUMBER: _ClassVar[int]
    DECISION_FIELD_NUMBER: _ClassVar[int]
    REASON_FIELD_NUMBER: _ClassVar[int]
    AT_FIELD_NUMBER: _ClassVar[int]
    DEVICE_FIELD_NUMBER: _ClassVar[int]
    by: _instance_pb2.Principal
    decision: DecisionKind
    reason: str
    at: _timestamp_pb2.Timestamp
    device: _instance_pb2.DeviceRef
    def __init__(self, by: _Optional[_Union[_instance_pb2.Principal, _Mapping]] = ..., decision: _Optional[_Union[DecisionKind, str]] = ..., reason: _Optional[str] = ..., at: _Optional[_Union[datetime.datetime, _timestamp_pb2.Timestamp, _Mapping]] = ..., device: _Optional[_Union[_instance_pb2.DeviceRef, _Mapping]] = ...) -> None: ...

class DecisionClaims(_message.Message):
    __slots__ = ("approval_id", "revision", "decision", "iat", "jti")
    APPROVAL_ID_FIELD_NUMBER: _ClassVar[int]
    REVISION_FIELD_NUMBER: _ClassVar[int]
    DECISION_FIELD_NUMBER: _ClassVar[int]
    IAT_FIELD_NUMBER: _ClassVar[int]
    JTI_FIELD_NUMBER: _ClassVar[int]
    approval_id: str
    revision: int
    decision: DecisionKind
    iat: int
    jti: str
    def __init__(self, approval_id: _Optional[str] = ..., revision: _Optional[int] = ..., decision: _Optional[_Union[DecisionKind, str]] = ..., iat: _Optional[int] = ..., jti: _Optional[str] = ...) -> None: ...

class ListApprovalsRequest(_message.Message):
    __slots__ = ("environments", "states", "page_size", "page_token")
    ENVIRONMENTS_FIELD_NUMBER: _ClassVar[int]
    STATES_FIELD_NUMBER: _ClassVar[int]
    PAGE_SIZE_FIELD_NUMBER: _ClassVar[int]
    PAGE_TOKEN_FIELD_NUMBER: _ClassVar[int]
    environments: _containers.RepeatedScalarFieldContainer[str]
    states: _containers.RepeatedScalarFieldContainer[ApprovalState]
    page_size: int
    page_token: str
    def __init__(self, environments: _Optional[_Iterable[str]] = ..., states: _Optional[_Iterable[_Union[ApprovalState, str]]] = ..., page_size: _Optional[int] = ..., page_token: _Optional[str] = ...) -> None: ...

class ListApprovalsResponse(_message.Message):
    __slots__ = ("approvals", "next_page_token")
    APPROVALS_FIELD_NUMBER: _ClassVar[int]
    NEXT_PAGE_TOKEN_FIELD_NUMBER: _ClassVar[int]
    approvals: _containers.RepeatedCompositeFieldContainer[Approval]
    next_page_token: str
    def __init__(self, approvals: _Optional[_Iterable[_Union[Approval, _Mapping]]] = ..., next_page_token: _Optional[str] = ...) -> None: ...

class GetApprovalRequest(_message.Message):
    __slots__ = ("approval_id",)
    APPROVAL_ID_FIELD_NUMBER: _ClassVar[int]
    approval_id: str
    def __init__(self, approval_id: _Optional[str] = ...) -> None: ...

class GetApprovalResponse(_message.Message):
    __slots__ = ("approval",)
    APPROVAL_FIELD_NUMBER: _ClassVar[int]
    approval: Approval
    def __init__(self, approval: _Optional[_Union[Approval, _Mapping]] = ...) -> None: ...

class WatchApprovalsRequest(_message.Message):
    __slots__ = ("environments", "states", "resume_cursor")
    ENVIRONMENTS_FIELD_NUMBER: _ClassVar[int]
    STATES_FIELD_NUMBER: _ClassVar[int]
    RESUME_CURSOR_FIELD_NUMBER: _ClassVar[int]
    environments: _containers.RepeatedScalarFieldContainer[str]
    states: _containers.RepeatedScalarFieldContainer[ApprovalState]
    resume_cursor: str
    def __init__(self, environments: _Optional[_Iterable[str]] = ..., states: _Optional[_Iterable[_Union[ApprovalState, str]]] = ..., resume_cursor: _Optional[str] = ...) -> None: ...

class WatchApprovalsResponse(_message.Message):
    __slots__ = ("snapshot", "upsert", "remove", "heartbeat", "cursor", "snapshot_reset")
    SNAPSHOT_FIELD_NUMBER: _ClassVar[int]
    UPSERT_FIELD_NUMBER: _ClassVar[int]
    REMOVE_FIELD_NUMBER: _ClassVar[int]
    HEARTBEAT_FIELD_NUMBER: _ClassVar[int]
    CURSOR_FIELD_NUMBER: _ClassVar[int]
    SNAPSHOT_RESET_FIELD_NUMBER: _ClassVar[int]
    snapshot: ApprovalSnapshot
    upsert: Approval
    remove: str
    heartbeat: Heartbeat
    cursor: str
    snapshot_reset: bool
    def __init__(self, snapshot: _Optional[_Union[ApprovalSnapshot, _Mapping]] = ..., upsert: _Optional[_Union[Approval, _Mapping]] = ..., remove: _Optional[str] = ..., heartbeat: _Optional[_Union[Heartbeat, _Mapping]] = ..., cursor: _Optional[str] = ..., snapshot_reset: _Optional[bool] = ...) -> None: ...

class ApprovalSnapshot(_message.Message):
    __slots__ = ("approvals",)
    APPROVALS_FIELD_NUMBER: _ClassVar[int]
    approvals: _containers.RepeatedCompositeFieldContainer[Approval]
    def __init__(self, approvals: _Optional[_Iterable[_Union[Approval, _Mapping]]] = ...) -> None: ...

class Heartbeat(_message.Message):
    __slots__ = ()
    def __init__(self) -> None: ...

class DecideApprovalRequest(_message.Message):
    __slots__ = ("approval_id", "revision", "decision", "reason", "decision_proof", "idempotency_key")
    APPROVAL_ID_FIELD_NUMBER: _ClassVar[int]
    REVISION_FIELD_NUMBER: _ClassVar[int]
    DECISION_FIELD_NUMBER: _ClassVar[int]
    REASON_FIELD_NUMBER: _ClassVar[int]
    DECISION_PROOF_FIELD_NUMBER: _ClassVar[int]
    IDEMPOTENCY_KEY_FIELD_NUMBER: _ClassVar[int]
    approval_id: str
    revision: int
    decision: DecisionKind
    reason: str
    decision_proof: str
    idempotency_key: str
    def __init__(self, approval_id: _Optional[str] = ..., revision: _Optional[int] = ..., decision: _Optional[_Union[DecisionKind, str]] = ..., reason: _Optional[str] = ..., decision_proof: _Optional[str] = ..., idempotency_key: _Optional[str] = ...) -> None: ...

class DecideApprovalResponse(_message.Message):
    __slots__ = ("approval",)
    APPROVAL_FIELD_NUMBER: _ClassVar[int]
    approval: Approval
    def __init__(self, approval: _Optional[_Union[Approval, _Mapping]] = ...) -> None: ...
