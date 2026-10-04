from loams.live.v1 import value_pb2 as _value_pb2
from loams.options.v1 import options_pb2 as _options_pb2
from google.protobuf.internal import containers as _containers
from google.protobuf.internal import enum_type_wrapper as _enum_type_wrapper
from google.protobuf import descriptor as _descriptor
from google.protobuf import message as _message
from collections.abc import Iterable as _Iterable, Mapping as _Mapping
from typing import ClassVar as _ClassVar, Optional as _Optional, Union as _Union

DESCRIPTOR: _descriptor.FileDescriptor

class ErrorCode(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    ERROR_CODE_UNSPECIFIED: _ClassVar[ErrorCode]
    ERROR_CODE_INVALID_ARGUMENT: _ClassVar[ErrorCode]
    ERROR_CODE_NOT_FOUND: _ClassVar[ErrorCode]
    ERROR_CODE_FAILED_PRECONDITION: _ClassVar[ErrorCode]
    ERROR_CODE_RESOURCE_EXHAUSTED: _ClassVar[ErrorCode]
    ERROR_CODE_FUNCTION_ERROR: _ClassVar[ErrorCode]
    ERROR_CODE_FUNCTION_TIMEOUT: _ClassVar[ErrorCode]
    ERROR_CODE_FUNCTION_OUT_OF_MEMORY: _ClassVar[ErrorCode]
    ERROR_CODE_UNAVAILABLE: _ClassVar[ErrorCode]
    ERROR_CODE_INTERNAL: _ClassVar[ErrorCode]
ERROR_CODE_UNSPECIFIED: ErrorCode
ERROR_CODE_INVALID_ARGUMENT: ErrorCode
ERROR_CODE_NOT_FOUND: ErrorCode
ERROR_CODE_FAILED_PRECONDITION: ErrorCode
ERROR_CODE_RESOURCE_EXHAUSTED: ErrorCode
ERROR_CODE_FUNCTION_ERROR: ErrorCode
ERROR_CODE_FUNCTION_TIMEOUT: ErrorCode
ERROR_CODE_FUNCTION_OUT_OF_MEMORY: ErrorCode
ERROR_CODE_UNAVAILABLE: ErrorCode
ERROR_CODE_INTERNAL: ErrorCode

class StateVersion(_message.Message):
    __slots__ = ("query_set", "identity", "ts")
    QUERY_SET_FIELD_NUMBER: _ClassVar[int]
    IDENTITY_FIELD_NUMBER: _ClassVar[int]
    TS_FIELD_NUMBER: _ClassVar[int]
    query_set: int
    identity: int
    ts: int
    def __init__(self, query_set: _Optional[int] = ..., identity: _Optional[int] = ..., ts: _Optional[int] = ...) -> None: ...

class WatchRequest(_message.Message):
    __slots__ = ("initial", "resume")
    INITIAL_FIELD_NUMBER: _ClassVar[int]
    RESUME_FIELD_NUMBER: _ClassVar[int]
    initial: QuerySet
    resume: Resume
    def __init__(self, initial: _Optional[_Union[QuerySet, _Mapping]] = ..., resume: _Optional[_Union[Resume, _Mapping]] = ...) -> None: ...

class Resume(_message.Message):
    __slots__ = ("last_version", "query_set")
    LAST_VERSION_FIELD_NUMBER: _ClassVar[int]
    QUERY_SET_FIELD_NUMBER: _ClassVar[int]
    last_version: StateVersion
    query_set: QuerySet
    def __init__(self, last_version: _Optional[_Union[StateVersion, _Mapping]] = ..., query_set: _Optional[_Union[QuerySet, _Mapping]] = ...) -> None: ...

class QuerySet(_message.Message):
    __slots__ = ("version", "queries")
    VERSION_FIELD_NUMBER: _ClassVar[int]
    QUERIES_FIELD_NUMBER: _ClassVar[int]
    version: int
    queries: _containers.RepeatedCompositeFieldContainer[QuerySpec]
    def __init__(self, version: _Optional[int] = ..., queries: _Optional[_Iterable[_Union[QuerySpec, _Mapping]]] = ...) -> None: ...

class QuerySpec(_message.Message):
    __slots__ = ("query_id", "function", "args")
    QUERY_ID_FIELD_NUMBER: _ClassVar[int]
    FUNCTION_FIELD_NUMBER: _ClassVar[int]
    ARGS_FIELD_NUMBER: _ClassVar[int]
    query_id: int
    function: str
    args: _value_pb2.Value
    def __init__(self, query_id: _Optional[int] = ..., function: _Optional[str] = ..., args: _Optional[_Union[_value_pb2.Value, _Mapping]] = ...) -> None: ...

class ModifyQuerySetRequest(_message.Message):
    __slots__ = ("session_id", "base_version", "new_version", "changes")
    SESSION_ID_FIELD_NUMBER: _ClassVar[int]
    BASE_VERSION_FIELD_NUMBER: _ClassVar[int]
    NEW_VERSION_FIELD_NUMBER: _ClassVar[int]
    CHANGES_FIELD_NUMBER: _ClassVar[int]
    session_id: str
    base_version: int
    new_version: int
    changes: _containers.RepeatedCompositeFieldContainer[QuerySetChange]
    def __init__(self, session_id: _Optional[str] = ..., base_version: _Optional[int] = ..., new_version: _Optional[int] = ..., changes: _Optional[_Iterable[_Union[QuerySetChange, _Mapping]]] = ...) -> None: ...

class QuerySetChange(_message.Message):
    __slots__ = ("add", "remove")
    ADD_FIELD_NUMBER: _ClassVar[int]
    REMOVE_FIELD_NUMBER: _ClassVar[int]
    add: QuerySpec
    remove: int
    def __init__(self, add: _Optional[_Union[QuerySpec, _Mapping]] = ..., remove: _Optional[int] = ...) -> None: ...

class ModifyQuerySetResponse(_message.Message):
    __slots__ = ()
    def __init__(self) -> None: ...

class Transition(_message.Message):
    __slots__ = ("session_id", "start", "end", "updates", "more")
    SESSION_ID_FIELD_NUMBER: _ClassVar[int]
    START_FIELD_NUMBER: _ClassVar[int]
    END_FIELD_NUMBER: _ClassVar[int]
    UPDATES_FIELD_NUMBER: _ClassVar[int]
    MORE_FIELD_NUMBER: _ClassVar[int]
    session_id: str
    start: StateVersion
    end: StateVersion
    updates: _containers.RepeatedCompositeFieldContainer[QueryUpdate]
    more: bool
    def __init__(self, session_id: _Optional[str] = ..., start: _Optional[_Union[StateVersion, _Mapping]] = ..., end: _Optional[_Union[StateVersion, _Mapping]] = ..., updates: _Optional[_Iterable[_Union[QueryUpdate, _Mapping]]] = ..., more: _Optional[bool] = ...) -> None: ...

class QueryUpdate(_message.Message):
    __slots__ = ("query_id", "value", "error", "removed")
    QUERY_ID_FIELD_NUMBER: _ClassVar[int]
    VALUE_FIELD_NUMBER: _ClassVar[int]
    ERROR_FIELD_NUMBER: _ClassVar[int]
    REMOVED_FIELD_NUMBER: _ClassVar[int]
    query_id: int
    value: _value_pb2.Value
    error: LiveError
    removed: Removed
    def __init__(self, query_id: _Optional[int] = ..., value: _Optional[_Union[_value_pb2.Value, _Mapping]] = ..., error: _Optional[_Union[LiveError, _Mapping]] = ..., removed: _Optional[_Union[Removed, _Mapping]] = ...) -> None: ...

class Removed(_message.Message):
    __slots__ = ()
    def __init__(self) -> None: ...

class QueryRequest(_message.Message):
    __slots__ = ("function", "args", "ts")
    FUNCTION_FIELD_NUMBER: _ClassVar[int]
    ARGS_FIELD_NUMBER: _ClassVar[int]
    TS_FIELD_NUMBER: _ClassVar[int]
    function: str
    args: _value_pb2.Value
    ts: int
    def __init__(self, function: _Optional[str] = ..., args: _Optional[_Union[_value_pb2.Value, _Mapping]] = ..., ts: _Optional[int] = ...) -> None: ...

class QueryResponse(_message.Message):
    __slots__ = ("ts", "result")
    TS_FIELD_NUMBER: _ClassVar[int]
    RESULT_FIELD_NUMBER: _ClassVar[int]
    ts: int
    result: _value_pb2.Value
    def __init__(self, ts: _Optional[int] = ..., result: _Optional[_Union[_value_pb2.Value, _Mapping]] = ...) -> None: ...

class MutateRequest(_message.Message):
    __slots__ = ("function", "args", "idempotency_key")
    FUNCTION_FIELD_NUMBER: _ClassVar[int]
    ARGS_FIELD_NUMBER: _ClassVar[int]
    IDEMPOTENCY_KEY_FIELD_NUMBER: _ClassVar[int]
    function: str
    args: _value_pb2.Value
    idempotency_key: str
    def __init__(self, function: _Optional[str] = ..., args: _Optional[_Union[_value_pb2.Value, _Mapping]] = ..., idempotency_key: _Optional[str] = ...) -> None: ...

class MutateResponse(_message.Message):
    __slots__ = ("commit_ts", "result")
    COMMIT_TS_FIELD_NUMBER: _ClassVar[int]
    RESULT_FIELD_NUMBER: _ClassVar[int]
    commit_ts: int
    result: _value_pb2.Value
    def __init__(self, commit_ts: _Optional[int] = ..., result: _Optional[_Union[_value_pb2.Value, _Mapping]] = ...) -> None: ...

class DeployRequest(_message.Message):
    __slots__ = ("bundle", "schema")
    BUNDLE_FIELD_NUMBER: _ClassVar[int]
    SCHEMA_FIELD_NUMBER: _ClassVar[int]
    bundle: bytes
    schema: Schema
    def __init__(self, bundle: _Optional[bytes] = ..., schema: _Optional[_Union[Schema, _Mapping]] = ...) -> None: ...

class DeployResponse(_message.Message):
    __slots__ = ("deployment_id",)
    DEPLOYMENT_ID_FIELD_NUMBER: _ClassVar[int]
    deployment_id: str
    def __init__(self, deployment_id: _Optional[str] = ...) -> None: ...

class Schema(_message.Message):
    __slots__ = ("tables",)
    TABLES_FIELD_NUMBER: _ClassVar[int]
    tables: _containers.RepeatedCompositeFieldContainer[TableSchema]
    def __init__(self, tables: _Optional[_Iterable[_Union[TableSchema, _Mapping]]] = ...) -> None: ...

class TableSchema(_message.Message):
    __slots__ = ("name", "indexes")
    NAME_FIELD_NUMBER: _ClassVar[int]
    INDEXES_FIELD_NUMBER: _ClassVar[int]
    name: str
    indexes: _containers.RepeatedCompositeFieldContainer[IndexSchema]
    def __init__(self, name: _Optional[str] = ..., indexes: _Optional[_Iterable[_Union[IndexSchema, _Mapping]]] = ...) -> None: ...

class IndexSchema(_message.Message):
    __slots__ = ("name", "fields")
    NAME_FIELD_NUMBER: _ClassVar[int]
    FIELDS_FIELD_NUMBER: _ClassVar[int]
    name: str
    fields: _containers.RepeatedScalarFieldContainer[str]
    def __init__(self, name: _Optional[str] = ..., fields: _Optional[_Iterable[str]] = ...) -> None: ...

class LiveError(_message.Message):
    __slots__ = ("code", "message")
    CODE_FIELD_NUMBER: _ClassVar[int]
    MESSAGE_FIELD_NUMBER: _ClassVar[int]
    code: ErrorCode
    message: str
    def __init__(self, code: _Optional[_Union[ErrorCode, str]] = ..., message: _Optional[str] = ...) -> None: ...
