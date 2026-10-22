from loams.options.v1 import options_pb2 as _options_pb2
from google.protobuf.internal import containers as _containers
from google.protobuf.internal import enum_type_wrapper as _enum_type_wrapper
from google.protobuf import descriptor as _descriptor
from google.protobuf import message as _message
from collections.abc import Iterable as _Iterable, Mapping as _Mapping
from typing import ClassVar as _ClassVar, Optional as _Optional, Union as _Union

DESCRIPTOR: _descriptor.FileDescriptor

class EventStatus(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    EVENT_STATUS_UNSPECIFIED: _ClassVar[EventStatus]
    EVENT_STATUS_APPENDED: _ClassVar[EventStatus]
    EVENT_STATUS_DUPLICATE: _ClassVar[EventStatus]
    EVENT_STATUS_IN_FLIGHT: _ClassVar[EventStatus]
EVENT_STATUS_UNSPECIFIED: EventStatus
EVENT_STATUS_APPENDED: EventStatus
EVENT_STATUS_DUPLICATE: EventStatus
EVENT_STATUS_IN_FLIGHT: EventStatus

class Retention(_message.Message):
    __slots__ = ("max_age_ms", "max_bytes")
    MAX_AGE_MS_FIELD_NUMBER: _ClassVar[int]
    MAX_BYTES_FIELD_NUMBER: _ClassVar[int]
    max_age_ms: int
    max_bytes: int
    def __init__(self, max_age_ms: _Optional[int] = ..., max_bytes: _Optional[int] = ...) -> None: ...

class PartitionOffsets(_message.Message):
    __slots__ = ("partition", "log_start_offset", "high_watermark")
    PARTITION_FIELD_NUMBER: _ClassVar[int]
    LOG_START_OFFSET_FIELD_NUMBER: _ClassVar[int]
    HIGH_WATERMARK_FIELD_NUMBER: _ClassVar[int]
    partition: int
    log_start_offset: int
    high_watermark: int
    def __init__(self, partition: _Optional[int] = ..., log_start_offset: _Optional[int] = ..., high_watermark: _Optional[int] = ...) -> None: ...

class StreamInfo(_message.Message):
    __slots__ = ("namespace", "name", "id", "partitions", "retention", "offsets")
    NAMESPACE_FIELD_NUMBER: _ClassVar[int]
    NAME_FIELD_NUMBER: _ClassVar[int]
    ID_FIELD_NUMBER: _ClassVar[int]
    PARTITIONS_FIELD_NUMBER: _ClassVar[int]
    RETENTION_FIELD_NUMBER: _ClassVar[int]
    OFFSETS_FIELD_NUMBER: _ClassVar[int]
    namespace: str
    name: str
    id: int
    partitions: int
    retention: Retention
    offsets: _containers.RepeatedCompositeFieldContainer[PartitionOffsets]
    def __init__(self, namespace: _Optional[str] = ..., name: _Optional[str] = ..., id: _Optional[int] = ..., partitions: _Optional[int] = ..., retention: _Optional[_Union[Retention, _Mapping]] = ..., offsets: _Optional[_Iterable[_Union[PartitionOffsets, _Mapping]]] = ...) -> None: ...

class CreateStreamRequest(_message.Message):
    __slots__ = ("namespace", "name", "partitions", "retention")
    NAMESPACE_FIELD_NUMBER: _ClassVar[int]
    NAME_FIELD_NUMBER: _ClassVar[int]
    PARTITIONS_FIELD_NUMBER: _ClassVar[int]
    RETENTION_FIELD_NUMBER: _ClassVar[int]
    namespace: str
    name: str
    partitions: int
    retention: Retention
    def __init__(self, namespace: _Optional[str] = ..., name: _Optional[str] = ..., partitions: _Optional[int] = ..., retention: _Optional[_Union[Retention, _Mapping]] = ...) -> None: ...

class CreateStreamResponse(_message.Message):
    __slots__ = ("stream", "created")
    STREAM_FIELD_NUMBER: _ClassVar[int]
    CREATED_FIELD_NUMBER: _ClassVar[int]
    stream: StreamInfo
    created: bool
    def __init__(self, stream: _Optional[_Union[StreamInfo, _Mapping]] = ..., created: _Optional[bool] = ...) -> None: ...

class DescribeStreamRequest(_message.Message):
    __slots__ = ("namespace", "stream")
    NAMESPACE_FIELD_NUMBER: _ClassVar[int]
    STREAM_FIELD_NUMBER: _ClassVar[int]
    namespace: str
    stream: str
    def __init__(self, namespace: _Optional[str] = ..., stream: _Optional[str] = ...) -> None: ...

class DescribeStreamResponse(_message.Message):
    __slots__ = ("stream",)
    STREAM_FIELD_NUMBER: _ClassVar[int]
    stream: StreamInfo
    def __init__(self, stream: _Optional[_Union[StreamInfo, _Mapping]] = ...) -> None: ...

class ListStreamsRequest(_message.Message):
    __slots__ = ("namespace", "page_size", "page_token")
    NAMESPACE_FIELD_NUMBER: _ClassVar[int]
    PAGE_SIZE_FIELD_NUMBER: _ClassVar[int]
    PAGE_TOKEN_FIELD_NUMBER: _ClassVar[int]
    namespace: str
    page_size: int
    page_token: str
    def __init__(self, namespace: _Optional[str] = ..., page_size: _Optional[int] = ..., page_token: _Optional[str] = ...) -> None: ...

class ListStreamsResponse(_message.Message):
    __slots__ = ("streams", "next_page_token")
    STREAMS_FIELD_NUMBER: _ClassVar[int]
    NEXT_PAGE_TOKEN_FIELD_NUMBER: _ClassVar[int]
    streams: _containers.RepeatedCompositeFieldContainer[StreamInfo]
    next_page_token: str
    def __init__(self, streams: _Optional[_Iterable[_Union[StreamInfo, _Mapping]]] = ..., next_page_token: _Optional[str] = ...) -> None: ...

class ProduceRequest(_message.Message):
    __slots__ = ("namespace", "stream", "partition", "records", "producer_id", "producer_seq")
    NAMESPACE_FIELD_NUMBER: _ClassVar[int]
    STREAM_FIELD_NUMBER: _ClassVar[int]
    PARTITION_FIELD_NUMBER: _ClassVar[int]
    RECORDS_FIELD_NUMBER: _ClassVar[int]
    PRODUCER_ID_FIELD_NUMBER: _ClassVar[int]
    PRODUCER_SEQ_FIELD_NUMBER: _ClassVar[int]
    namespace: str
    stream: str
    partition: int
    records: _containers.RepeatedCompositeFieldContainer[Record]
    producer_id: str
    producer_seq: int
    def __init__(self, namespace: _Optional[str] = ..., stream: _Optional[str] = ..., partition: _Optional[int] = ..., records: _Optional[_Iterable[_Union[Record, _Mapping]]] = ..., producer_id: _Optional[str] = ..., producer_seq: _Optional[int] = ...) -> None: ...

class Record(_message.Message):
    __slots__ = ("key", "value", "headers", "timestamp_ms")
    KEY_FIELD_NUMBER: _ClassVar[int]
    VALUE_FIELD_NUMBER: _ClassVar[int]
    HEADERS_FIELD_NUMBER: _ClassVar[int]
    TIMESTAMP_MS_FIELD_NUMBER: _ClassVar[int]
    key: bytes
    value: bytes
    headers: _containers.RepeatedCompositeFieldContainer[Header]
    timestamp_ms: int
    def __init__(self, key: _Optional[bytes] = ..., value: _Optional[bytes] = ..., headers: _Optional[_Iterable[_Union[Header, _Mapping]]] = ..., timestamp_ms: _Optional[int] = ...) -> None: ...

class Header(_message.Message):
    __slots__ = ("key", "value")
    KEY_FIELD_NUMBER: _ClassVar[int]
    VALUE_FIELD_NUMBER: _ClassVar[int]
    key: str
    value: bytes
    def __init__(self, key: _Optional[str] = ..., value: _Optional[bytes] = ...) -> None: ...

class ProduceResponse(_message.Message):
    __slots__ = ("stream_id", "base_offset", "last_offset", "consistency_token", "duplicate")
    STREAM_ID_FIELD_NUMBER: _ClassVar[int]
    BASE_OFFSET_FIELD_NUMBER: _ClassVar[int]
    LAST_OFFSET_FIELD_NUMBER: _ClassVar[int]
    CONSISTENCY_TOKEN_FIELD_NUMBER: _ClassVar[int]
    DUPLICATE_FIELD_NUMBER: _ClassVar[int]
    stream_id: int
    base_offset: int
    last_offset: int
    consistency_token: str
    duplicate: bool
    def __init__(self, stream_id: _Optional[int] = ..., base_offset: _Optional[int] = ..., last_offset: _Optional[int] = ..., consistency_token: _Optional[str] = ..., duplicate: _Optional[bool] = ...) -> None: ...

class FetchRequest(_message.Message):
    __slots__ = ("namespace", "stream", "partition", "offset", "max_bytes", "max_records", "max_wait_ms")
    NAMESPACE_FIELD_NUMBER: _ClassVar[int]
    STREAM_FIELD_NUMBER: _ClassVar[int]
    PARTITION_FIELD_NUMBER: _ClassVar[int]
    OFFSET_FIELD_NUMBER: _ClassVar[int]
    MAX_BYTES_FIELD_NUMBER: _ClassVar[int]
    MAX_RECORDS_FIELD_NUMBER: _ClassVar[int]
    MAX_WAIT_MS_FIELD_NUMBER: _ClassVar[int]
    namespace: str
    stream: str
    partition: int
    offset: int
    max_bytes: int
    max_records: int
    max_wait_ms: int
    def __init__(self, namespace: _Optional[str] = ..., stream: _Optional[str] = ..., partition: _Optional[int] = ..., offset: _Optional[int] = ..., max_bytes: _Optional[int] = ..., max_records: _Optional[int] = ..., max_wait_ms: _Optional[int] = ...) -> None: ...

class FetchedRecord(_message.Message):
    __slots__ = ("offset", "key", "value", "headers", "timestamp_ms")
    OFFSET_FIELD_NUMBER: _ClassVar[int]
    KEY_FIELD_NUMBER: _ClassVar[int]
    VALUE_FIELD_NUMBER: _ClassVar[int]
    HEADERS_FIELD_NUMBER: _ClassVar[int]
    TIMESTAMP_MS_FIELD_NUMBER: _ClassVar[int]
    offset: int
    key: bytes
    value: bytes
    headers: _containers.RepeatedCompositeFieldContainer[Header]
    timestamp_ms: int
    def __init__(self, offset: _Optional[int] = ..., key: _Optional[bytes] = ..., value: _Optional[bytes] = ..., headers: _Optional[_Iterable[_Union[Header, _Mapping]]] = ..., timestamp_ms: _Optional[int] = ...) -> None: ...

class FetchResponse(_message.Message):
    __slots__ = ("records", "next_offset", "high_watermark", "log_start_offset")
    RECORDS_FIELD_NUMBER: _ClassVar[int]
    NEXT_OFFSET_FIELD_NUMBER: _ClassVar[int]
    HIGH_WATERMARK_FIELD_NUMBER: _ClassVar[int]
    LOG_START_OFFSET_FIELD_NUMBER: _ClassVar[int]
    records: _containers.RepeatedCompositeFieldContainer[FetchedRecord]
    next_offset: int
    high_watermark: int
    log_start_offset: int
    def __init__(self, records: _Optional[_Iterable[_Union[FetchedRecord, _Mapping]]] = ..., next_offset: _Optional[int] = ..., high_watermark: _Optional[int] = ..., log_start_offset: _Optional[int] = ...) -> None: ...

class ProduceCloudEventsRequest(_message.Message):
    __slots__ = ("namespace", "stream", "partition", "batch", "json_batch")
    NAMESPACE_FIELD_NUMBER: _ClassVar[int]
    STREAM_FIELD_NUMBER: _ClassVar[int]
    PARTITION_FIELD_NUMBER: _ClassVar[int]
    BATCH_FIELD_NUMBER: _ClassVar[int]
    JSON_BATCH_FIELD_NUMBER: _ClassVar[int]
    namespace: str
    stream: str
    partition: int
    batch: bytes
    json_batch: bytes
    def __init__(self, namespace: _Optional[str] = ..., stream: _Optional[str] = ..., partition: _Optional[int] = ..., batch: _Optional[bytes] = ..., json_batch: _Optional[bytes] = ...) -> None: ...

class EventResult(_message.Message):
    __slots__ = ("status", "partition", "offset", "retry_after_ms")
    STATUS_FIELD_NUMBER: _ClassVar[int]
    PARTITION_FIELD_NUMBER: _ClassVar[int]
    OFFSET_FIELD_NUMBER: _ClassVar[int]
    RETRY_AFTER_MS_FIELD_NUMBER: _ClassVar[int]
    status: EventStatus
    partition: int
    offset: int
    retry_after_ms: int
    def __init__(self, status: _Optional[_Union[EventStatus, str]] = ..., partition: _Optional[int] = ..., offset: _Optional[int] = ..., retry_after_ms: _Optional[int] = ...) -> None: ...

class ProduceCloudEventsResponse(_message.Message):
    __slots__ = ("results", "consistency_token")
    RESULTS_FIELD_NUMBER: _ClassVar[int]
    CONSISTENCY_TOKEN_FIELD_NUMBER: _ClassVar[int]
    results: _containers.RepeatedCompositeFieldContainer[EventResult]
    consistency_token: str
    def __init__(self, results: _Optional[_Iterable[_Union[EventResult, _Mapping]]] = ..., consistency_token: _Optional[str] = ...) -> None: ...

class FetchCloudEventsRequest(_message.Message):
    __slots__ = ("namespace", "stream", "partition", "offset", "max_bytes", "max_wait_ms")
    NAMESPACE_FIELD_NUMBER: _ClassVar[int]
    STREAM_FIELD_NUMBER: _ClassVar[int]
    PARTITION_FIELD_NUMBER: _ClassVar[int]
    OFFSET_FIELD_NUMBER: _ClassVar[int]
    MAX_BYTES_FIELD_NUMBER: _ClassVar[int]
    MAX_WAIT_MS_FIELD_NUMBER: _ClassVar[int]
    namespace: str
    stream: str
    partition: int
    offset: int
    max_bytes: int
    max_wait_ms: int
    def __init__(self, namespace: _Optional[str] = ..., stream: _Optional[str] = ..., partition: _Optional[int] = ..., offset: _Optional[int] = ..., max_bytes: _Optional[int] = ..., max_wait_ms: _Optional[int] = ...) -> None: ...

class FetchCloudEventsResponse(_message.Message):
    __slots__ = ("json_batch", "count", "next_offset", "high_watermark")
    JSON_BATCH_FIELD_NUMBER: _ClassVar[int]
    COUNT_FIELD_NUMBER: _ClassVar[int]
    NEXT_OFFSET_FIELD_NUMBER: _ClassVar[int]
    HIGH_WATERMARK_FIELD_NUMBER: _ClassVar[int]
    json_batch: bytes
    count: int
    next_offset: int
    high_watermark: int
    def __init__(self, json_batch: _Optional[bytes] = ..., count: _Optional[int] = ..., next_offset: _Optional[int] = ..., high_watermark: _Optional[int] = ...) -> None: ...
