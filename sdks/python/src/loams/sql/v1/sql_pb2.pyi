from google.protobuf import struct_pb2 as _struct_pb2
from loams.collection.v1 import document_pb2 as _document_pb2
from loams.options.v1 import options_pb2 as _options_pb2
from google.protobuf.internal import containers as _containers
from google.protobuf import descriptor as _descriptor
from google.protobuf import message as _message
from collections.abc import Iterable as _Iterable, Mapping as _Mapping
from typing import ClassVar as _ClassVar, Optional as _Optional, Union as _Union

DESCRIPTOR: _descriptor.FileDescriptor

class QueryRequest(_message.Message):
    __slots__ = ("namespace", "sql", "params", "consistency", "max_rows")
    NAMESPACE_FIELD_NUMBER: _ClassVar[int]
    SQL_FIELD_NUMBER: _ClassVar[int]
    PARAMS_FIELD_NUMBER: _ClassVar[int]
    CONSISTENCY_FIELD_NUMBER: _ClassVar[int]
    MAX_ROWS_FIELD_NUMBER: _ClassVar[int]
    namespace: str
    sql: str
    params: _containers.RepeatedCompositeFieldContainer[_struct_pb2.Value]
    consistency: _document_pb2.Consistency
    max_rows: int
    def __init__(self, namespace: _Optional[str] = ..., sql: _Optional[str] = ..., params: _Optional[_Iterable[_Union[_struct_pb2.Value, _Mapping]]] = ..., consistency: _Optional[_Union[_document_pb2.Consistency, _Mapping]] = ..., max_rows: _Optional[int] = ...) -> None: ...

class Column(_message.Message):
    __slots__ = ("name", "type")
    NAME_FIELD_NUMBER: _ClassVar[int]
    TYPE_FIELD_NUMBER: _ClassVar[int]
    name: str
    type: str
    def __init__(self, name: _Optional[str] = ..., type: _Optional[str] = ...) -> None: ...

class QueryResponse(_message.Message):
    __slots__ = ("columns", "rows", "truncated", "consistency_token")
    COLUMNS_FIELD_NUMBER: _ClassVar[int]
    ROWS_FIELD_NUMBER: _ClassVar[int]
    TRUNCATED_FIELD_NUMBER: _ClassVar[int]
    CONSISTENCY_TOKEN_FIELD_NUMBER: _ClassVar[int]
    columns: _containers.RepeatedCompositeFieldContainer[Column]
    rows: _containers.RepeatedCompositeFieldContainer[_struct_pb2.ListValue]
    truncated: bool
    consistency_token: str
    def __init__(self, columns: _Optional[_Iterable[_Union[Column, _Mapping]]] = ..., rows: _Optional[_Iterable[_Union[_struct_pb2.ListValue, _Mapping]]] = ..., truncated: _Optional[bool] = ..., consistency_token: _Optional[str] = ...) -> None: ...

class QueryArrowRequest(_message.Message):
    __slots__ = ("namespace", "sql", "params", "consistency", "max_rows")
    NAMESPACE_FIELD_NUMBER: _ClassVar[int]
    SQL_FIELD_NUMBER: _ClassVar[int]
    PARAMS_FIELD_NUMBER: _ClassVar[int]
    CONSISTENCY_FIELD_NUMBER: _ClassVar[int]
    MAX_ROWS_FIELD_NUMBER: _ClassVar[int]
    namespace: str
    sql: str
    params: _containers.RepeatedCompositeFieldContainer[_struct_pb2.Value]
    consistency: _document_pb2.Consistency
    max_rows: int
    def __init__(self, namespace: _Optional[str] = ..., sql: _Optional[str] = ..., params: _Optional[_Iterable[_Union[_struct_pb2.Value, _Mapping]]] = ..., consistency: _Optional[_Union[_document_pb2.Consistency, _Mapping]] = ..., max_rows: _Optional[int] = ...) -> None: ...

class QueryArrowResponse(_message.Message):
    __slots__ = ("arrow_ipc", "truncated", "consistency_token")
    ARROW_IPC_FIELD_NUMBER: _ClassVar[int]
    TRUNCATED_FIELD_NUMBER: _ClassVar[int]
    CONSISTENCY_TOKEN_FIELD_NUMBER: _ClassVar[int]
    arrow_ipc: bytes
    truncated: bool
    consistency_token: str
    def __init__(self, arrow_ipc: _Optional[bytes] = ..., truncated: _Optional[bool] = ..., consistency_token: _Optional[str] = ...) -> None: ...
