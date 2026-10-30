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

class NotificationChannel(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    NOTIFICATION_CHANNEL_UNSPECIFIED: _ClassVar[NotificationChannel]
    NOTIFICATION_CHANNEL_EMAIL: _ClassVar[NotificationChannel]
    NOTIFICATION_CHANNEL_CONSOLE: _ClassVar[NotificationChannel]

class DeliveryStatus(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    DELIVERY_STATUS_UNSPECIFIED: _ClassVar[DeliveryStatus]
    DELIVERY_STATUS_DELIVERED: _ClassVar[DeliveryStatus]
    DELIVERY_STATUS_RETRYING: _ClassVar[DeliveryStatus]
    DELIVERY_STATUS_FAILED: _ClassVar[DeliveryStatus]
NOTIFICATION_CHANNEL_UNSPECIFIED: NotificationChannel
NOTIFICATION_CHANNEL_EMAIL: NotificationChannel
NOTIFICATION_CHANNEL_CONSOLE: NotificationChannel
DELIVERY_STATUS_UNSPECIFIED: DeliveryStatus
DELIVERY_STATUS_DELIVERED: DeliveryStatus
DELIVERY_STATUS_RETRYING: DeliveryStatus
DELIVERY_STATUS_FAILED: DeliveryStatus

class Rule(_message.Message):
    __slots__ = ("id", "customer_id", "feature", "thresholds", "spend_cap", "channels", "emails")
    ID_FIELD_NUMBER: _ClassVar[int]
    CUSTOMER_ID_FIELD_NUMBER: _ClassVar[int]
    FEATURE_FIELD_NUMBER: _ClassVar[int]
    THRESHOLDS_FIELD_NUMBER: _ClassVar[int]
    SPEND_CAP_FIELD_NUMBER: _ClassVar[int]
    CHANNELS_FIELD_NUMBER: _ClassVar[int]
    EMAILS_FIELD_NUMBER: _ClassVar[int]
    id: str
    customer_id: str
    feature: str
    thresholds: _containers.RepeatedScalarFieldContainer[int]
    spend_cap: bool
    channels: _containers.RepeatedScalarFieldContainer[NotificationChannel]
    emails: _containers.RepeatedScalarFieldContainer[str]
    def __init__(self, id: _Optional[str] = ..., customer_id: _Optional[str] = ..., feature: _Optional[str] = ..., thresholds: _Optional[_Iterable[int]] = ..., spend_cap: _Optional[bool] = ..., channels: _Optional[_Iterable[_Union[NotificationChannel, str]]] = ..., emails: _Optional[_Iterable[str]] = ...) -> None: ...

class Webhook(_message.Message):
    __slots__ = ("id", "customer_id", "url", "types", "create_time")
    ID_FIELD_NUMBER: _ClassVar[int]
    CUSTOMER_ID_FIELD_NUMBER: _ClassVar[int]
    URL_FIELD_NUMBER: _ClassVar[int]
    TYPES_FIELD_NUMBER: _ClassVar[int]
    CREATE_TIME_FIELD_NUMBER: _ClassVar[int]
    id: str
    customer_id: str
    url: str
    types: _containers.RepeatedScalarFieldContainer[str]
    create_time: _timestamp_pb2.Timestamp
    def __init__(self, id: _Optional[str] = ..., customer_id: _Optional[str] = ..., url: _Optional[str] = ..., types: _Optional[_Iterable[str]] = ..., create_time: _Optional[_Union[datetime.datetime, _timestamp_pb2.Timestamp, _Mapping]] = ...) -> None: ...

class Delivery(_message.Message):
    __slots__ = ("webhook_id", "event_id", "event_type", "time", "status", "attempt", "last_outcome", "next_attempt_time")
    WEBHOOK_ID_FIELD_NUMBER: _ClassVar[int]
    EVENT_ID_FIELD_NUMBER: _ClassVar[int]
    EVENT_TYPE_FIELD_NUMBER: _ClassVar[int]
    TIME_FIELD_NUMBER: _ClassVar[int]
    STATUS_FIELD_NUMBER: _ClassVar[int]
    ATTEMPT_FIELD_NUMBER: _ClassVar[int]
    LAST_OUTCOME_FIELD_NUMBER: _ClassVar[int]
    NEXT_ATTEMPT_TIME_FIELD_NUMBER: _ClassVar[int]
    webhook_id: str
    event_id: str
    event_type: str
    time: _timestamp_pb2.Timestamp
    status: DeliveryStatus
    attempt: int
    last_outcome: str
    next_attempt_time: _timestamp_pb2.Timestamp
    def __init__(self, webhook_id: _Optional[str] = ..., event_id: _Optional[str] = ..., event_type: _Optional[str] = ..., time: _Optional[_Union[datetime.datetime, _timestamp_pb2.Timestamp, _Mapping]] = ..., status: _Optional[_Union[DeliveryStatus, str]] = ..., attempt: _Optional[int] = ..., last_outcome: _Optional[str] = ..., next_attempt_time: _Optional[_Union[datetime.datetime, _timestamp_pb2.Timestamp, _Mapping]] = ...) -> None: ...

class CreateRuleRequest(_message.Message):
    __slots__ = ("rule",)
    RULE_FIELD_NUMBER: _ClassVar[int]
    rule: Rule
    def __init__(self, rule: _Optional[_Union[Rule, _Mapping]] = ...) -> None: ...

class CreateRuleResponse(_message.Message):
    __slots__ = ("rule",)
    RULE_FIELD_NUMBER: _ClassVar[int]
    rule: Rule
    def __init__(self, rule: _Optional[_Union[Rule, _Mapping]] = ...) -> None: ...

class ListRulesRequest(_message.Message):
    __slots__ = ("customer_id",)
    CUSTOMER_ID_FIELD_NUMBER: _ClassVar[int]
    customer_id: str
    def __init__(self, customer_id: _Optional[str] = ...) -> None: ...

class ListRulesResponse(_message.Message):
    __slots__ = ("rules",)
    RULES_FIELD_NUMBER: _ClassVar[int]
    rules: _containers.RepeatedCompositeFieldContainer[Rule]
    def __init__(self, rules: _Optional[_Iterable[_Union[Rule, _Mapping]]] = ...) -> None: ...

class CreateWebhookRequest(_message.Message):
    __slots__ = ("customer_id", "url", "types")
    CUSTOMER_ID_FIELD_NUMBER: _ClassVar[int]
    URL_FIELD_NUMBER: _ClassVar[int]
    TYPES_FIELD_NUMBER: _ClassVar[int]
    customer_id: str
    url: str
    types: _containers.RepeatedScalarFieldContainer[str]
    def __init__(self, customer_id: _Optional[str] = ..., url: _Optional[str] = ..., types: _Optional[_Iterable[str]] = ...) -> None: ...

class CreateWebhookResponse(_message.Message):
    __slots__ = ("webhook", "secret")
    WEBHOOK_FIELD_NUMBER: _ClassVar[int]
    SECRET_FIELD_NUMBER: _ClassVar[int]
    webhook: Webhook
    secret: str
    def __init__(self, webhook: _Optional[_Union[Webhook, _Mapping]] = ..., secret: _Optional[str] = ...) -> None: ...

class RotateWebhookSecretRequest(_message.Message):
    __slots__ = ("id",)
    ID_FIELD_NUMBER: _ClassVar[int]
    id: str
    def __init__(self, id: _Optional[str] = ...) -> None: ...

class RotateWebhookSecretResponse(_message.Message):
    __slots__ = ("webhook", "secret")
    WEBHOOK_FIELD_NUMBER: _ClassVar[int]
    SECRET_FIELD_NUMBER: _ClassVar[int]
    webhook: Webhook
    secret: str
    def __init__(self, webhook: _Optional[_Union[Webhook, _Mapping]] = ..., secret: _Optional[str] = ...) -> None: ...

class ListDeliveriesRequest(_message.Message):
    __slots__ = ("customer_id", "page_size")
    CUSTOMER_ID_FIELD_NUMBER: _ClassVar[int]
    PAGE_SIZE_FIELD_NUMBER: _ClassVar[int]
    customer_id: str
    page_size: int
    def __init__(self, customer_id: _Optional[str] = ..., page_size: _Optional[int] = ...) -> None: ...

class ListDeliveriesResponse(_message.Message):
    __slots__ = ("deliveries",)
    DELIVERIES_FIELD_NUMBER: _ClassVar[int]
    deliveries: _containers.RepeatedCompositeFieldContainer[Delivery]
    def __init__(self, deliveries: _Optional[_Iterable[_Union[Delivery, _Mapping]]] = ...) -> None: ...

class ConsoleNotification(_message.Message):
    __slots__ = ("id", "customer_id", "event_type", "feature", "title", "body", "banner", "create_time")
    ID_FIELD_NUMBER: _ClassVar[int]
    CUSTOMER_ID_FIELD_NUMBER: _ClassVar[int]
    EVENT_TYPE_FIELD_NUMBER: _ClassVar[int]
    FEATURE_FIELD_NUMBER: _ClassVar[int]
    TITLE_FIELD_NUMBER: _ClassVar[int]
    BODY_FIELD_NUMBER: _ClassVar[int]
    BANNER_FIELD_NUMBER: _ClassVar[int]
    CREATE_TIME_FIELD_NUMBER: _ClassVar[int]
    id: str
    customer_id: str
    event_type: str
    feature: str
    title: str
    body: str
    banner: bool
    create_time: _timestamp_pb2.Timestamp
    def __init__(self, id: _Optional[str] = ..., customer_id: _Optional[str] = ..., event_type: _Optional[str] = ..., feature: _Optional[str] = ..., title: _Optional[str] = ..., body: _Optional[str] = ..., banner: _Optional[bool] = ..., create_time: _Optional[_Union[datetime.datetime, _timestamp_pb2.Timestamp, _Mapping]] = ...) -> None: ...

class ListConsoleNotificationsRequest(_message.Message):
    __slots__ = ("customer_id", "page_size")
    CUSTOMER_ID_FIELD_NUMBER: _ClassVar[int]
    PAGE_SIZE_FIELD_NUMBER: _ClassVar[int]
    customer_id: str
    page_size: int
    def __init__(self, customer_id: _Optional[str] = ..., page_size: _Optional[int] = ...) -> None: ...

class ListConsoleNotificationsResponse(_message.Message):
    __slots__ = ("notifications",)
    NOTIFICATIONS_FIELD_NUMBER: _ClassVar[int]
    notifications: _containers.RepeatedCompositeFieldContainer[ConsoleNotification]
    def __init__(self, notifications: _Optional[_Iterable[_Union[ConsoleNotification, _Mapping]]] = ...) -> None: ...
