from google.protobuf import descriptor_pb2 as _descriptor_pb2
from google.protobuf import descriptor as _descriptor
from google.protobuf import message as _message
from typing import ClassVar as _ClassVar, Optional as _Optional

DESCRIPTOR: _descriptor.FileDescriptor
MODULE_FIELD_NUMBER: _ClassVar[int]
module: _descriptor.FieldDescriptor
FACADE_FIELD_NUMBER: _ClassVar[int]
facade: _descriptor.FieldDescriptor

class ModuleOptions(_message.Message):
    __slots__ = ("name", "summary", "unstable")
    NAME_FIELD_NUMBER: _ClassVar[int]
    SUMMARY_FIELD_NUMBER: _ClassVar[int]
    UNSTABLE_FIELD_NUMBER: _ClassVar[int]
    name: str
    summary: str
    unstable: bool
    def __init__(self, name: _Optional[str] = ..., summary: _Optional[str] = ..., unstable: _Optional[bool] = ...) -> None: ...

class FacadeOptions(_message.Message):
    __slots__ = ("module", "name", "retry_safe", "pagination")
    MODULE_FIELD_NUMBER: _ClassVar[int]
    NAME_FIELD_NUMBER: _ClassVar[int]
    RETRY_SAFE_FIELD_NUMBER: _ClassVar[int]
    PAGINATION_FIELD_NUMBER: _ClassVar[int]
    module: str
    name: str
    retry_safe: bool
    pagination: str
    def __init__(self, module: _Optional[str] = ..., name: _Optional[str] = ..., retry_safe: _Optional[bool] = ..., pagination: _Optional[str] = ...) -> None: ...
