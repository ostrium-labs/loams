import datetime

from google.protobuf import struct_pb2 as _struct_pb2
from google.protobuf import timestamp_pb2 as _timestamp_pb2
from google.protobuf.internal import containers as _containers
from google.protobuf.internal import enum_type_wrapper as _enum_type_wrapper
from google.protobuf import descriptor as _descriptor
from google.protobuf import message as _message
from collections.abc import Iterable as _Iterable, Mapping as _Mapping
from typing import ClassVar as _ClassVar, Optional as _Optional, Union as _Union

DESCRIPTOR: _descriptor.FileDescriptor

class OperationState(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    OPERATION_STATE_UNSPECIFIED: _ClassVar[OperationState]
    OPERATION_STATE_PENDING: _ClassVar[OperationState]
    OPERATION_STATE_AWAITING_APPROVAL: _ClassVar[OperationState]
    OPERATION_STATE_RUNNING: _ClassVar[OperationState]
    OPERATION_STATE_SUCCEEDED: _ClassVar[OperationState]
    OPERATION_STATE_FAILED: _ClassVar[OperationState]
    OPERATION_STATE_CANCELED: _ClassVar[OperationState]
OPERATION_STATE_UNSPECIFIED: OperationState
OPERATION_STATE_PENDING: OperationState
OPERATION_STATE_AWAITING_APPROVAL: OperationState
OPERATION_STATE_RUNNING: OperationState
OPERATION_STATE_SUCCEEDED: OperationState
OPERATION_STATE_FAILED: OperationState
OPERATION_STATE_CANCELED: OperationState

class Operation(_message.Message):
    __slots__ = ("id", "kind", "namespace", "target", "state", "progress", "created_at", "updated_at", "result", "error", "approval_id")
    class TargetEntry(_message.Message):
        __slots__ = ("key", "value")
        KEY_FIELD_NUMBER: _ClassVar[int]
        VALUE_FIELD_NUMBER: _ClassVar[int]
        key: str
        value: str
        def __init__(self, key: _Optional[str] = ..., value: _Optional[str] = ...) -> None: ...
    ID_FIELD_NUMBER: _ClassVar[int]
    KIND_FIELD_NUMBER: _ClassVar[int]
    NAMESPACE_FIELD_NUMBER: _ClassVar[int]
    TARGET_FIELD_NUMBER: _ClassVar[int]
    STATE_FIELD_NUMBER: _ClassVar[int]
    PROGRESS_FIELD_NUMBER: _ClassVar[int]
    CREATED_AT_FIELD_NUMBER: _ClassVar[int]
    UPDATED_AT_FIELD_NUMBER: _ClassVar[int]
    RESULT_FIELD_NUMBER: _ClassVar[int]
    ERROR_FIELD_NUMBER: _ClassVar[int]
    APPROVAL_ID_FIELD_NUMBER: _ClassVar[int]
    id: str
    kind: str
    namespace: str
    target: _containers.ScalarMap[str, str]
    state: OperationState
    progress: Progress
    created_at: _timestamp_pb2.Timestamp
    updated_at: _timestamp_pb2.Timestamp
    result: _struct_pb2.Struct
    error: OperationError
    approval_id: str
    def __init__(self, id: _Optional[str] = ..., kind: _Optional[str] = ..., namespace: _Optional[str] = ..., target: _Optional[_Mapping[str, str]] = ..., state: _Optional[_Union[OperationState, str]] = ..., progress: _Optional[_Union[Progress, _Mapping]] = ..., created_at: _Optional[_Union[datetime.datetime, _timestamp_pb2.Timestamp, _Mapping]] = ..., updated_at: _Optional[_Union[datetime.datetime, _timestamp_pb2.Timestamp, _Mapping]] = ..., result: _Optional[_Union[_struct_pb2.Struct, _Mapping]] = ..., error: _Optional[_Union[OperationError, _Mapping]] = ..., approval_id: _Optional[str] = ...) -> None: ...

class Progress(_message.Message):
    __slots__ = ("done", "total", "unit", "phase")
    DONE_FIELD_NUMBER: _ClassVar[int]
    TOTAL_FIELD_NUMBER: _ClassVar[int]
    UNIT_FIELD_NUMBER: _ClassVar[int]
    PHASE_FIELD_NUMBER: _ClassVar[int]
    done: int
    total: int
    unit: str
    phase: str
    def __init__(self, done: _Optional[int] = ..., total: _Optional[int] = ..., unit: _Optional[str] = ..., phase: _Optional[str] = ...) -> None: ...

class OperationError(_message.Message):
    __slots__ = ("code", "message")
    CODE_FIELD_NUMBER: _ClassVar[int]
    MESSAGE_FIELD_NUMBER: _ClassVar[int]
    code: str
    message: str
    def __init__(self, code: _Optional[str] = ..., message: _Optional[str] = ...) -> None: ...

class GetOperationRequest(_message.Message):
    __slots__ = ("operation_id",)
    OPERATION_ID_FIELD_NUMBER: _ClassVar[int]
    operation_id: str
    def __init__(self, operation_id: _Optional[str] = ...) -> None: ...

class GetOperationResponse(_message.Message):
    __slots__ = ("operation",)
    OPERATION_FIELD_NUMBER: _ClassVar[int]
    operation: Operation
    def __init__(self, operation: _Optional[_Union[Operation, _Mapping]] = ...) -> None: ...

class ListOperationsRequest(_message.Message):
    __slots__ = ("namespace", "states", "page_size", "page_token")
    NAMESPACE_FIELD_NUMBER: _ClassVar[int]
    STATES_FIELD_NUMBER: _ClassVar[int]
    PAGE_SIZE_FIELD_NUMBER: _ClassVar[int]
    PAGE_TOKEN_FIELD_NUMBER: _ClassVar[int]
    namespace: str
    states: _containers.RepeatedScalarFieldContainer[OperationState]
    page_size: int
    page_token: str
    def __init__(self, namespace: _Optional[str] = ..., states: _Optional[_Iterable[_Union[OperationState, str]]] = ..., page_size: _Optional[int] = ..., page_token: _Optional[str] = ...) -> None: ...

class ListOperationsResponse(_message.Message):
    __slots__ = ("operations", "next_page_token")
    OPERATIONS_FIELD_NUMBER: _ClassVar[int]
    NEXT_PAGE_TOKEN_FIELD_NUMBER: _ClassVar[int]
    operations: _containers.RepeatedCompositeFieldContainer[Operation]
    next_page_token: str
    def __init__(self, operations: _Optional[_Iterable[_Union[Operation, _Mapping]]] = ..., next_page_token: _Optional[str] = ...) -> None: ...

class WatchOperationsRequest(_message.Message):
    __slots__ = ("namespace", "states", "resume_cursor")
    NAMESPACE_FIELD_NUMBER: _ClassVar[int]
    STATES_FIELD_NUMBER: _ClassVar[int]
    RESUME_CURSOR_FIELD_NUMBER: _ClassVar[int]
    namespace: str
    states: _containers.RepeatedScalarFieldContainer[OperationState]
    resume_cursor: str
    def __init__(self, namespace: _Optional[str] = ..., states: _Optional[_Iterable[_Union[OperationState, str]]] = ..., resume_cursor: _Optional[str] = ...) -> None: ...

class WatchOperationsResponse(_message.Message):
    __slots__ = ("snapshot", "upsert", "remove", "heartbeat", "cursor", "snapshot_reset")
    SNAPSHOT_FIELD_NUMBER: _ClassVar[int]
    UPSERT_FIELD_NUMBER: _ClassVar[int]
    REMOVE_FIELD_NUMBER: _ClassVar[int]
    HEARTBEAT_FIELD_NUMBER: _ClassVar[int]
    CURSOR_FIELD_NUMBER: _ClassVar[int]
    SNAPSHOT_RESET_FIELD_NUMBER: _ClassVar[int]
    snapshot: OperationSnapshot
    upsert: Operation
    remove: str
    heartbeat: Heartbeat
    cursor: str
    snapshot_reset: bool
    def __init__(self, snapshot: _Optional[_Union[OperationSnapshot, _Mapping]] = ..., upsert: _Optional[_Union[Operation, _Mapping]] = ..., remove: _Optional[str] = ..., heartbeat: _Optional[_Union[Heartbeat, _Mapping]] = ..., cursor: _Optional[str] = ..., snapshot_reset: _Optional[bool] = ...) -> None: ...

class OperationSnapshot(_message.Message):
    __slots__ = ("operations",)
    OPERATIONS_FIELD_NUMBER: _ClassVar[int]
    operations: _containers.RepeatedCompositeFieldContainer[Operation]
    def __init__(self, operations: _Optional[_Iterable[_Union[Operation, _Mapping]]] = ...) -> None: ...

class Heartbeat(_message.Message):
    __slots__ = ()
    def __init__(self) -> None: ...

class CancelOperationRequest(_message.Message):
    __slots__ = ("operation_id", "idempotency_key")
    OPERATION_ID_FIELD_NUMBER: _ClassVar[int]
    IDEMPOTENCY_KEY_FIELD_NUMBER: _ClassVar[int]
    operation_id: str
    idempotency_key: str
    def __init__(self, operation_id: _Optional[str] = ..., idempotency_key: _Optional[str] = ...) -> None: ...

class CancelOperationResponse(_message.Message):
    __slots__ = ("operation",)
    OPERATION_FIELD_NUMBER: _ClassVar[int]
    operation: Operation
    def __init__(self, operation: _Optional[_Union[Operation, _Mapping]] = ...) -> None: ...
