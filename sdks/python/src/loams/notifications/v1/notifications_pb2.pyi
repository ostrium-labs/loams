import datetime

from google.protobuf import timestamp_pb2 as _timestamp_pb2
from loams.devices.v1 import devices_pb2 as _devices_pb2
from google.protobuf.internal import containers as _containers
from google.protobuf import descriptor as _descriptor
from google.protobuf import message as _message
from collections.abc import Iterable as _Iterable, Mapping as _Mapping
from typing import ClassVar as _ClassVar, Optional as _Optional, Union as _Union

DESCRIPTOR: _descriptor.FileDescriptor

class Notification(_message.Message):
    __slots__ = ("id", "category", "cloudevent_type", "subject", "title", "body", "approval_id", "operation_id", "job_ref", "run_ref", "environment", "created_at", "read_at")
    ID_FIELD_NUMBER: _ClassVar[int]
    CATEGORY_FIELD_NUMBER: _ClassVar[int]
    CLOUDEVENT_TYPE_FIELD_NUMBER: _ClassVar[int]
    SUBJECT_FIELD_NUMBER: _ClassVar[int]
    TITLE_FIELD_NUMBER: _ClassVar[int]
    BODY_FIELD_NUMBER: _ClassVar[int]
    APPROVAL_ID_FIELD_NUMBER: _ClassVar[int]
    OPERATION_ID_FIELD_NUMBER: _ClassVar[int]
    JOB_REF_FIELD_NUMBER: _ClassVar[int]
    RUN_REF_FIELD_NUMBER: _ClassVar[int]
    ENVIRONMENT_FIELD_NUMBER: _ClassVar[int]
    CREATED_AT_FIELD_NUMBER: _ClassVar[int]
    READ_AT_FIELD_NUMBER: _ClassVar[int]
    id: str
    category: _devices_pb2.NotificationCategory
    cloudevent_type: str
    subject: str
    title: str
    body: str
    approval_id: str
    operation_id: str
    job_ref: str
    run_ref: str
    environment: str
    created_at: _timestamp_pb2.Timestamp
    read_at: _timestamp_pb2.Timestamp
    def __init__(self, id: _Optional[str] = ..., category: _Optional[_Union[_devices_pb2.NotificationCategory, str]] = ..., cloudevent_type: _Optional[str] = ..., subject: _Optional[str] = ..., title: _Optional[str] = ..., body: _Optional[str] = ..., approval_id: _Optional[str] = ..., operation_id: _Optional[str] = ..., job_ref: _Optional[str] = ..., run_ref: _Optional[str] = ..., environment: _Optional[str] = ..., created_at: _Optional[_Union[datetime.datetime, _timestamp_pb2.Timestamp, _Mapping]] = ..., read_at: _Optional[_Union[datetime.datetime, _timestamp_pb2.Timestamp, _Mapping]] = ...) -> None: ...

class ListNotificationsRequest(_message.Message):
    __slots__ = ("unread_only", "page_size", "page_token")
    UNREAD_ONLY_FIELD_NUMBER: _ClassVar[int]
    PAGE_SIZE_FIELD_NUMBER: _ClassVar[int]
    PAGE_TOKEN_FIELD_NUMBER: _ClassVar[int]
    unread_only: bool
    page_size: int
    page_token: str
    def __init__(self, unread_only: _Optional[bool] = ..., page_size: _Optional[int] = ..., page_token: _Optional[str] = ...) -> None: ...

class ListNotificationsResponse(_message.Message):
    __slots__ = ("notifications", "next_page_token")
    NOTIFICATIONS_FIELD_NUMBER: _ClassVar[int]
    NEXT_PAGE_TOKEN_FIELD_NUMBER: _ClassVar[int]
    notifications: _containers.RepeatedCompositeFieldContainer[Notification]
    next_page_token: str
    def __init__(self, notifications: _Optional[_Iterable[_Union[Notification, _Mapping]]] = ..., next_page_token: _Optional[str] = ...) -> None: ...

class WatchNotificationsRequest(_message.Message):
    __slots__ = ("resume_cursor",)
    RESUME_CURSOR_FIELD_NUMBER: _ClassVar[int]
    resume_cursor: str
    def __init__(self, resume_cursor: _Optional[str] = ...) -> None: ...

class WatchNotificationsResponse(_message.Message):
    __slots__ = ("snapshot", "upsert", "remove", "heartbeat", "cursor", "snapshot_reset")
    SNAPSHOT_FIELD_NUMBER: _ClassVar[int]
    UPSERT_FIELD_NUMBER: _ClassVar[int]
    REMOVE_FIELD_NUMBER: _ClassVar[int]
    HEARTBEAT_FIELD_NUMBER: _ClassVar[int]
    CURSOR_FIELD_NUMBER: _ClassVar[int]
    SNAPSHOT_RESET_FIELD_NUMBER: _ClassVar[int]
    snapshot: NotificationSnapshot
    upsert: Notification
    remove: str
    heartbeat: Heartbeat
    cursor: str
    snapshot_reset: bool
    def __init__(self, snapshot: _Optional[_Union[NotificationSnapshot, _Mapping]] = ..., upsert: _Optional[_Union[Notification, _Mapping]] = ..., remove: _Optional[str] = ..., heartbeat: _Optional[_Union[Heartbeat, _Mapping]] = ..., cursor: _Optional[str] = ..., snapshot_reset: _Optional[bool] = ...) -> None: ...

class NotificationSnapshot(_message.Message):
    __slots__ = ("notifications",)
    NOTIFICATIONS_FIELD_NUMBER: _ClassVar[int]
    notifications: _containers.RepeatedCompositeFieldContainer[Notification]
    def __init__(self, notifications: _Optional[_Iterable[_Union[Notification, _Mapping]]] = ...) -> None: ...

class Heartbeat(_message.Message):
    __slots__ = ()
    def __init__(self) -> None: ...

class MarkReadRequest(_message.Message):
    __slots__ = ("notification_ids", "all", "idempotency_key")
    NOTIFICATION_IDS_FIELD_NUMBER: _ClassVar[int]
    ALL_FIELD_NUMBER: _ClassVar[int]
    IDEMPOTENCY_KEY_FIELD_NUMBER: _ClassVar[int]
    notification_ids: _containers.RepeatedScalarFieldContainer[str]
    all: bool
    idempotency_key: str
    def __init__(self, notification_ids: _Optional[_Iterable[str]] = ..., all: _Optional[bool] = ..., idempotency_key: _Optional[str] = ...) -> None: ...

class MarkReadResponse(_message.Message):
    __slots__ = ("marked",)
    MARKED_FIELD_NUMBER: _ClassVar[int]
    marked: int
    def __init__(self, marked: _Optional[int] = ...) -> None: ...
