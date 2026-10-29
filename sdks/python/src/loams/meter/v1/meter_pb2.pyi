import datetime

from google.protobuf import timestamp_pb2 as _timestamp_pb2
from loams.options.v1 import options_pb2 as _options_pb2
from google.protobuf.internal import containers as _containers
from google.protobuf.internal import enum_type_wrapper as _enum_type_wrapper
from google.protobuf import descriptor as _descriptor
from google.protobuf import message as _message
from collections.abc import Iterable as _Iterable, Mapping as _Mapping
from typing import ClassVar as _ClassVar, Optional as _Optional, Union as _Union

DESCRIPTOR: _descriptor.FileDescriptor

class WindowSize(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    WINDOW_SIZE_UNSPECIFIED: _ClassVar[WindowSize]
    WINDOW_SIZE_MINUTE: _ClassVar[WindowSize]
    WINDOW_SIZE_HOUR: _ClassVar[WindowSize]
    WINDOW_SIZE_DAY: _ClassVar[WindowSize]
    WINDOW_SIZE_MONTH: _ClassVar[WindowSize]
WINDOW_SIZE_UNSPECIFIED: WindowSize
WINDOW_SIZE_MINUTE: WindowSize
WINDOW_SIZE_HOUR: WindowSize
WINDOW_SIZE_DAY: WindowSize
WINDOW_SIZE_MONTH: WindowSize

class DimensionFilter(_message.Message):
    __slots__ = ("dimension", "values")
    DIMENSION_FIELD_NUMBER: _ClassVar[int]
    VALUES_FIELD_NUMBER: _ClassVar[int]
    dimension: str
    values: _containers.RepeatedScalarFieldContainer[str]
    def __init__(self, dimension: _Optional[str] = ..., values: _Optional[_Iterable[str]] = ...) -> None: ...

class QueryMeterRequest(_message.Message):
    __slots__ = ("meter", "to", "window_size", "window_time_zone", "subjects", "group_by", "filters")
    METER_FIELD_NUMBER: _ClassVar[int]
    FROM_FIELD_NUMBER: _ClassVar[int]
    TO_FIELD_NUMBER: _ClassVar[int]
    WINDOW_SIZE_FIELD_NUMBER: _ClassVar[int]
    WINDOW_TIME_ZONE_FIELD_NUMBER: _ClassVar[int]
    SUBJECTS_FIELD_NUMBER: _ClassVar[int]
    GROUP_BY_FIELD_NUMBER: _ClassVar[int]
    FILTERS_FIELD_NUMBER: _ClassVar[int]
    meter: str
    to: _timestamp_pb2.Timestamp
    window_size: WindowSize
    window_time_zone: str
    subjects: _containers.RepeatedScalarFieldContainer[str]
    group_by: _containers.RepeatedScalarFieldContainer[str]
    filters: _containers.RepeatedCompositeFieldContainer[DimensionFilter]
    def __init__(self, meter: _Optional[str] = ..., to: _Optional[_Union[datetime.datetime, _timestamp_pb2.Timestamp, _Mapping]] = ..., window_size: _Optional[_Union[WindowSize, str]] = ..., window_time_zone: _Optional[str] = ..., subjects: _Optional[_Iterable[str]] = ..., group_by: _Optional[_Iterable[str]] = ..., filters: _Optional[_Iterable[_Union[DimensionFilter, _Mapping]]] = ..., **kwargs) -> None: ...

class DimensionValue(_message.Message):
    __slots__ = ("dimension", "value")
    DIMENSION_FIELD_NUMBER: _ClassVar[int]
    VALUE_FIELD_NUMBER: _ClassVar[int]
    dimension: str
    value: str
    def __init__(self, dimension: _Optional[str] = ..., value: _Optional[str] = ...) -> None: ...

class MeterQueryRow(_message.Message):
    __slots__ = ("value", "window_start", "window_end", "subject", "group_by", "provisional")
    VALUE_FIELD_NUMBER: _ClassVar[int]
    WINDOW_START_FIELD_NUMBER: _ClassVar[int]
    WINDOW_END_FIELD_NUMBER: _ClassVar[int]
    SUBJECT_FIELD_NUMBER: _ClassVar[int]
    GROUP_BY_FIELD_NUMBER: _ClassVar[int]
    PROVISIONAL_FIELD_NUMBER: _ClassVar[int]
    value: str
    window_start: _timestamp_pb2.Timestamp
    window_end: _timestamp_pb2.Timestamp
    subject: str
    group_by: _containers.RepeatedCompositeFieldContainer[DimensionValue]
    provisional: bool
    def __init__(self, value: _Optional[str] = ..., window_start: _Optional[_Union[datetime.datetime, _timestamp_pb2.Timestamp, _Mapping]] = ..., window_end: _Optional[_Union[datetime.datetime, _timestamp_pb2.Timestamp, _Mapping]] = ..., subject: _Optional[str] = ..., group_by: _Optional[_Iterable[_Union[DimensionValue, _Mapping]]] = ..., provisional: _Optional[bool] = ...) -> None: ...

class QueryMeterResponse(_message.Message):
    __slots__ = ("to", "window_size", "as_of", "rows")
    FROM_FIELD_NUMBER: _ClassVar[int]
    TO_FIELD_NUMBER: _ClassVar[int]
    WINDOW_SIZE_FIELD_NUMBER: _ClassVar[int]
    AS_OF_FIELD_NUMBER: _ClassVar[int]
    ROWS_FIELD_NUMBER: _ClassVar[int]
    to: _timestamp_pb2.Timestamp
    window_size: WindowSize
    as_of: _timestamp_pb2.Timestamp
    rows: _containers.RepeatedCompositeFieldContainer[MeterQueryRow]
    def __init__(self, to: _Optional[_Union[datetime.datetime, _timestamp_pb2.Timestamp, _Mapping]] = ..., window_size: _Optional[_Union[WindowSize, str]] = ..., as_of: _Optional[_Union[datetime.datetime, _timestamp_pb2.Timestamp, _Mapping]] = ..., rows: _Optional[_Iterable[_Union[MeterQueryRow, _Mapping]]] = ..., **kwargs) -> None: ...
