from loams.live.v1 import value_pb2 as _value_pb2
from google.protobuf import descriptor as _descriptor
from google.protobuf import message as _message
from collections.abc import Mapping as _Mapping
from typing import ClassVar as _ClassVar, Optional as _Optional, Union as _Union

DESCRIPTOR: _descriptor.FileDescriptor

class IdempotencyRecord(_message.Message):
    __slots__ = ("format", "result", "expires_ms", "function", "start_ts", "args_hash")
    FORMAT_FIELD_NUMBER: _ClassVar[int]
    RESULT_FIELD_NUMBER: _ClassVar[int]
    EXPIRES_MS_FIELD_NUMBER: _ClassVar[int]
    FUNCTION_FIELD_NUMBER: _ClassVar[int]
    START_TS_FIELD_NUMBER: _ClassVar[int]
    ARGS_HASH_FIELD_NUMBER: _ClassVar[int]
    format: int
    result: _value_pb2.Value
    expires_ms: int
    function: str
    start_ts: int
    args_hash: bytes
    def __init__(self, format: _Optional[int] = ..., result: _Optional[_Union[_value_pb2.Value, _Mapping]] = ..., expires_ms: _Optional[int] = ..., function: _Optional[str] = ..., start_ts: _Optional[int] = ..., args_hash: _Optional[bytes] = ...) -> None: ...
