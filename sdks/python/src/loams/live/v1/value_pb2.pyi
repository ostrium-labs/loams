from google.protobuf.internal import containers as _containers
from google.protobuf import descriptor as _descriptor
from google.protobuf import message as _message
from collections.abc import Iterable as _Iterable, Mapping as _Mapping
from typing import ClassVar as _ClassVar, Optional as _Optional, Union as _Union

DESCRIPTOR: _descriptor.FileDescriptor

class Value(_message.Message):
    __slots__ = ("null_value", "int64_value", "double_value", "bool_value", "string_value", "bytes_value", "array_value", "object_value")
    NULL_VALUE_FIELD_NUMBER: _ClassVar[int]
    INT64_VALUE_FIELD_NUMBER: _ClassVar[int]
    DOUBLE_VALUE_FIELD_NUMBER: _ClassVar[int]
    BOOL_VALUE_FIELD_NUMBER: _ClassVar[int]
    STRING_VALUE_FIELD_NUMBER: _ClassVar[int]
    BYTES_VALUE_FIELD_NUMBER: _ClassVar[int]
    ARRAY_VALUE_FIELD_NUMBER: _ClassVar[int]
    OBJECT_VALUE_FIELD_NUMBER: _ClassVar[int]
    null_value: Null
    int64_value: int
    double_value: float
    bool_value: bool
    string_value: str
    bytes_value: bytes
    array_value: Array
    object_value: Object
    def __init__(self, null_value: _Optional[_Union[Null, _Mapping]] = ..., int64_value: _Optional[int] = ..., double_value: _Optional[float] = ..., bool_value: _Optional[bool] = ..., string_value: _Optional[str] = ..., bytes_value: _Optional[bytes] = ..., array_value: _Optional[_Union[Array, _Mapping]] = ..., object_value: _Optional[_Union[Object, _Mapping]] = ...) -> None: ...

class Null(_message.Message):
    __slots__ = ()
    def __init__(self) -> None: ...

class Array(_message.Message):
    __slots__ = ("values",)
    VALUES_FIELD_NUMBER: _ClassVar[int]
    values: _containers.RepeatedCompositeFieldContainer[Value]
    def __init__(self, values: _Optional[_Iterable[_Union[Value, _Mapping]]] = ...) -> None: ...

class Object(_message.Message):
    __slots__ = ("fields",)
    class FieldsEntry(_message.Message):
        __slots__ = ("key", "value")
        KEY_FIELD_NUMBER: _ClassVar[int]
        VALUE_FIELD_NUMBER: _ClassVar[int]
        key: str
        value: Value
        def __init__(self, key: _Optional[str] = ..., value: _Optional[_Union[Value, _Mapping]] = ...) -> None: ...
    FIELDS_FIELD_NUMBER: _ClassVar[int]
    fields: _containers.MessageMap[str, Value]
    def __init__(self, fields: _Optional[_Mapping[str, Value]] = ...) -> None: ...

class DocumentRecord(_message.Message):
    __slots__ = ("format", "creation_ms", "fields")
    class FieldsEntry(_message.Message):
        __slots__ = ("key", "value")
        KEY_FIELD_NUMBER: _ClassVar[int]
        VALUE_FIELD_NUMBER: _ClassVar[int]
        key: str
        value: Value
        def __init__(self, key: _Optional[str] = ..., value: _Optional[_Union[Value, _Mapping]] = ...) -> None: ...
    FORMAT_FIELD_NUMBER: _ClassVar[int]
    CREATION_MS_FIELD_NUMBER: _ClassVar[int]
    FIELDS_FIELD_NUMBER: _ClassVar[int]
    format: int
    creation_ms: int
    fields: _containers.MessageMap[str, Value]
    def __init__(self, format: _Optional[int] = ..., creation_ms: _Optional[int] = ..., fields: _Optional[_Mapping[str, Value]] = ...) -> None: ...
