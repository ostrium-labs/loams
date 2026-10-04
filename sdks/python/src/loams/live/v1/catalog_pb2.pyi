from google.protobuf.internal import containers as _containers
from google.protobuf import descriptor as _descriptor
from google.protobuf import message as _message
from collections.abc import Iterable as _Iterable, Mapping as _Mapping
from typing import ClassVar as _ClassVar, Optional as _Optional, Union as _Union

DESCRIPTOR: _descriptor.FileDescriptor

class TableDef(_message.Message):
    __slots__ = ("format", "id", "name", "indexes", "next_index_id")
    FORMAT_FIELD_NUMBER: _ClassVar[int]
    ID_FIELD_NUMBER: _ClassVar[int]
    NAME_FIELD_NUMBER: _ClassVar[int]
    INDEXES_FIELD_NUMBER: _ClassVar[int]
    NEXT_INDEX_ID_FIELD_NUMBER: _ClassVar[int]
    format: int
    id: int
    name: str
    indexes: _containers.RepeatedCompositeFieldContainer[IndexDef]
    next_index_id: int
    def __init__(self, format: _Optional[int] = ..., id: _Optional[int] = ..., name: _Optional[str] = ..., indexes: _Optional[_Iterable[_Union[IndexDef, _Mapping]]] = ..., next_index_id: _Optional[int] = ...) -> None: ...

class IndexDef(_message.Message):
    __slots__ = ("id", "name", "fields")
    ID_FIELD_NUMBER: _ClassVar[int]
    NAME_FIELD_NUMBER: _ClassVar[int]
    FIELDS_FIELD_NUMBER: _ClassVar[int]
    id: int
    name: str
    fields: _containers.RepeatedScalarFieldContainer[str]
    def __init__(self, id: _Optional[int] = ..., name: _Optional[str] = ..., fields: _Optional[_Iterable[str]] = ...) -> None: ...

class AppDef(_message.Message):
    __slots__ = ("format", "journal_shards")
    FORMAT_FIELD_NUMBER: _ClassVar[int]
    JOURNAL_SHARDS_FIELD_NUMBER: _ClassVar[int]
    format: int
    journal_shards: int
    def __init__(self, format: _Optional[int] = ..., journal_shards: _Optional[int] = ...) -> None: ...
