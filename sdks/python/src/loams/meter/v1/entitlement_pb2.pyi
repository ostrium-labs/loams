import datetime

from google.protobuf import timestamp_pb2 as _timestamp_pb2
from loams.meter.v1 import customer_pb2 as _customer_pb2
from loams.options.v1 import options_pb2 as _options_pb2
from google.protobuf.internal import containers as _containers
from google.protobuf.internal import enum_type_wrapper as _enum_type_wrapper
from google.protobuf import descriptor as _descriptor
from google.protobuf import message as _message
from collections.abc import Iterable as _Iterable, Mapping as _Mapping
from typing import ClassVar as _ClassVar, Optional as _Optional, Union as _Union

DESCRIPTOR: _descriptor.FileDescriptor

class EntitlementMode(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    ENTITLEMENT_MODE_UNSPECIFIED: _ClassVar[EntitlementMode]
    ENTITLEMENT_MODE_HARD_LIMIT: _ClassVar[EntitlementMode]
    ENTITLEMENT_MODE_SOFT_LIMIT: _ClassVar[EntitlementMode]
    ENTITLEMENT_MODE_OVERAGE_ALLOWED: _ClassVar[EntitlementMode]

class Interval(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    INTERVAL_UNSPECIFIED: _ClassVar[Interval]
    INTERVAL_HOUR: _ClassVar[Interval]
    INTERVAL_DAY: _ClassVar[Interval]
    INTERVAL_WEEK: _ClassVar[Interval]
    INTERVAL_MONTH: _ClassVar[Interval]
    INTERVAL_YEAR: _ClassVar[Interval]

class ExpiryUnit(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    EXPIRY_UNIT_UNSPECIFIED: _ClassVar[ExpiryUnit]
    EXPIRY_UNIT_HOUR: _ClassVar[ExpiryUnit]
    EXPIRY_UNIT_DAY: _ClassVar[ExpiryUnit]
    EXPIRY_UNIT_WEEK: _ClassVar[ExpiryUnit]
    EXPIRY_UNIT_MONTH: _ClassVar[ExpiryUnit]
    EXPIRY_UNIT_YEAR: _ClassVar[ExpiryUnit]

class GrantKind(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    GRANT_KIND_UNSPECIFIED: _ClassVar[GrantKind]
    GRANT_KIND_PROMOTIONAL: _ClassVar[GrantKind]
    GRANT_KIND_INCLUDED: _ClassVar[GrantKind]
    GRANT_KIND_PREPAID: _ClassVar[GrantKind]
ENTITLEMENT_MODE_UNSPECIFIED: EntitlementMode
ENTITLEMENT_MODE_HARD_LIMIT: EntitlementMode
ENTITLEMENT_MODE_SOFT_LIMIT: EntitlementMode
ENTITLEMENT_MODE_OVERAGE_ALLOWED: EntitlementMode
INTERVAL_UNSPECIFIED: Interval
INTERVAL_HOUR: Interval
INTERVAL_DAY: Interval
INTERVAL_WEEK: Interval
INTERVAL_MONTH: Interval
INTERVAL_YEAR: Interval
EXPIRY_UNIT_UNSPECIFIED: ExpiryUnit
EXPIRY_UNIT_HOUR: ExpiryUnit
EXPIRY_UNIT_DAY: ExpiryUnit
EXPIRY_UNIT_WEEK: ExpiryUnit
EXPIRY_UNIT_MONTH: ExpiryUnit
EXPIRY_UNIT_YEAR: ExpiryUnit
GRANT_KIND_UNSPECIFIED: GrantKind
GRANT_KIND_PROMOTIONAL: GrantKind
GRANT_KIND_INCLUDED: GrantKind
GRANT_KIND_PREPAID: GrantKind

class UsagePeriod(_message.Message):
    __slots__ = ("interval", "anchor")
    INTERVAL_FIELD_NUMBER: _ClassVar[int]
    ANCHOR_FIELD_NUMBER: _ClassVar[int]
    interval: Interval
    anchor: _timestamp_pb2.Timestamp
    def __init__(self, interval: _Optional[_Union[Interval, str]] = ..., anchor: _Optional[_Union[datetime.datetime, _timestamp_pb2.Timestamp, _Mapping]] = ...) -> None: ...

class MeteredEntitlement(_message.Message):
    __slots__ = ("usage_period", "mode", "issue_after_reset", "preserve_overage_at_reset", "overage_price", "spend_cap")
    USAGE_PERIOD_FIELD_NUMBER: _ClassVar[int]
    MODE_FIELD_NUMBER: _ClassVar[int]
    ISSUE_AFTER_RESET_FIELD_NUMBER: _ClassVar[int]
    PRESERVE_OVERAGE_AT_RESET_FIELD_NUMBER: _ClassVar[int]
    OVERAGE_PRICE_FIELD_NUMBER: _ClassVar[int]
    SPEND_CAP_FIELD_NUMBER: _ClassVar[int]
    usage_period: UsagePeriod
    mode: EntitlementMode
    issue_after_reset: str
    preserve_overage_at_reset: bool
    overage_price: _customer_pb2.Money
    spend_cap: _customer_pb2.Money
    def __init__(self, usage_period: _Optional[_Union[UsagePeriod, _Mapping]] = ..., mode: _Optional[_Union[EntitlementMode, str]] = ..., issue_after_reset: _Optional[str] = ..., preserve_overage_at_reset: _Optional[bool] = ..., overage_price: _Optional[_Union[_customer_pb2.Money, _Mapping]] = ..., spend_cap: _Optional[_Union[_customer_pb2.Money, _Mapping]] = ...) -> None: ...

class BooleanEntitlement(_message.Message):
    __slots__ = ("enabled",)
    ENABLED_FIELD_NUMBER: _ClassVar[int]
    enabled: bool
    def __init__(self, enabled: _Optional[bool] = ...) -> None: ...

class StaticEntitlement(_message.Message):
    __slots__ = ("config_json",)
    CONFIG_JSON_FIELD_NUMBER: _ClassVar[int]
    config_json: str
    def __init__(self, config_json: _Optional[str] = ...) -> None: ...

class Entitlement(_message.Message):
    __slots__ = ("id", "subject", "feature", "metered", "toggle", "static_config", "create_time", "plan", "plan_version", "created_by")
    ID_FIELD_NUMBER: _ClassVar[int]
    SUBJECT_FIELD_NUMBER: _ClassVar[int]
    FEATURE_FIELD_NUMBER: _ClassVar[int]
    METERED_FIELD_NUMBER: _ClassVar[int]
    TOGGLE_FIELD_NUMBER: _ClassVar[int]
    STATIC_CONFIG_FIELD_NUMBER: _ClassVar[int]
    CREATE_TIME_FIELD_NUMBER: _ClassVar[int]
    PLAN_FIELD_NUMBER: _ClassVar[int]
    PLAN_VERSION_FIELD_NUMBER: _ClassVar[int]
    CREATED_BY_FIELD_NUMBER: _ClassVar[int]
    id: str
    subject: str
    feature: str
    metered: MeteredEntitlement
    toggle: BooleanEntitlement
    static_config: StaticEntitlement
    create_time: _timestamp_pb2.Timestamp
    plan: str
    plan_version: int
    created_by: str
    def __init__(self, id: _Optional[str] = ..., subject: _Optional[str] = ..., feature: _Optional[str] = ..., metered: _Optional[_Union[MeteredEntitlement, _Mapping]] = ..., toggle: _Optional[_Union[BooleanEntitlement, _Mapping]] = ..., static_config: _Optional[_Union[StaticEntitlement, _Mapping]] = ..., create_time: _Optional[_Union[datetime.datetime, _timestamp_pb2.Timestamp, _Mapping]] = ..., plan: _Optional[str] = ..., plan_version: _Optional[int] = ..., created_by: _Optional[str] = ...) -> None: ...

class Grant(_message.Message):
    __slots__ = ("id", "entitlement_id", "amount", "priority", "effective_time", "expiry_unit", "expiry_count", "rollover_max", "rollover_min", "recurrence", "kind", "paid", "void_time", "create_time")
    ID_FIELD_NUMBER: _ClassVar[int]
    ENTITLEMENT_ID_FIELD_NUMBER: _ClassVar[int]
    AMOUNT_FIELD_NUMBER: _ClassVar[int]
    PRIORITY_FIELD_NUMBER: _ClassVar[int]
    EFFECTIVE_TIME_FIELD_NUMBER: _ClassVar[int]
    EXPIRY_UNIT_FIELD_NUMBER: _ClassVar[int]
    EXPIRY_COUNT_FIELD_NUMBER: _ClassVar[int]
    ROLLOVER_MAX_FIELD_NUMBER: _ClassVar[int]
    ROLLOVER_MIN_FIELD_NUMBER: _ClassVar[int]
    RECURRENCE_FIELD_NUMBER: _ClassVar[int]
    KIND_FIELD_NUMBER: _ClassVar[int]
    PAID_FIELD_NUMBER: _ClassVar[int]
    VOID_TIME_FIELD_NUMBER: _ClassVar[int]
    CREATE_TIME_FIELD_NUMBER: _ClassVar[int]
    id: str
    entitlement_id: str
    amount: str
    priority: int
    effective_time: _timestamp_pb2.Timestamp
    expiry_unit: ExpiryUnit
    expiry_count: int
    rollover_max: str
    rollover_min: str
    recurrence: UsagePeriod
    kind: GrantKind
    paid: _customer_pb2.Money
    void_time: _timestamp_pb2.Timestamp
    create_time: _timestamp_pb2.Timestamp
    def __init__(self, id: _Optional[str] = ..., entitlement_id: _Optional[str] = ..., amount: _Optional[str] = ..., priority: _Optional[int] = ..., effective_time: _Optional[_Union[datetime.datetime, _timestamp_pb2.Timestamp, _Mapping]] = ..., expiry_unit: _Optional[_Union[ExpiryUnit, str]] = ..., expiry_count: _Optional[int] = ..., rollover_max: _Optional[str] = ..., rollover_min: _Optional[str] = ..., recurrence: _Optional[_Union[UsagePeriod, _Mapping]] = ..., kind: _Optional[_Union[GrantKind, str]] = ..., paid: _Optional[_Union[_customer_pb2.Money, _Mapping]] = ..., void_time: _Optional[_Union[datetime.datetime, _timestamp_pb2.Timestamp, _Mapping]] = ..., create_time: _Optional[_Union[datetime.datetime, _timestamp_pb2.Timestamp, _Mapping]] = ...) -> None: ...

class EntitlementValue(_message.Message):
    __slots__ = ("has_access", "balance", "usage", "overage", "total_available", "period_start", "period_end", "as_of")
    HAS_ACCESS_FIELD_NUMBER: _ClassVar[int]
    BALANCE_FIELD_NUMBER: _ClassVar[int]
    USAGE_FIELD_NUMBER: _ClassVar[int]
    OVERAGE_FIELD_NUMBER: _ClassVar[int]
    TOTAL_AVAILABLE_FIELD_NUMBER: _ClassVar[int]
    PERIOD_START_FIELD_NUMBER: _ClassVar[int]
    PERIOD_END_FIELD_NUMBER: _ClassVar[int]
    AS_OF_FIELD_NUMBER: _ClassVar[int]
    has_access: bool
    balance: str
    usage: str
    overage: str
    total_available: str
    period_start: _timestamp_pb2.Timestamp
    period_end: _timestamp_pb2.Timestamp
    as_of: str
    def __init__(self, has_access: _Optional[bool] = ..., balance: _Optional[str] = ..., usage: _Optional[str] = ..., overage: _Optional[str] = ..., total_available: _Optional[str] = ..., period_start: _Optional[_Union[datetime.datetime, _timestamp_pb2.Timestamp, _Mapping]] = ..., period_end: _Optional[_Union[datetime.datetime, _timestamp_pb2.Timestamp, _Mapping]] = ..., as_of: _Optional[str] = ...) -> None: ...

class CreateEntitlementRequest(_message.Message):
    __slots__ = ("entitlement", "reason", "idempotency_key")
    ENTITLEMENT_FIELD_NUMBER: _ClassVar[int]
    REASON_FIELD_NUMBER: _ClassVar[int]
    IDEMPOTENCY_KEY_FIELD_NUMBER: _ClassVar[int]
    entitlement: Entitlement
    reason: str
    idempotency_key: str
    def __init__(self, entitlement: _Optional[_Union[Entitlement, _Mapping]] = ..., reason: _Optional[str] = ..., idempotency_key: _Optional[str] = ...) -> None: ...

class CreateEntitlementResponse(_message.Message):
    __slots__ = ("entitlement",)
    ENTITLEMENT_FIELD_NUMBER: _ClassVar[int]
    entitlement: Entitlement
    def __init__(self, entitlement: _Optional[_Union[Entitlement, _Mapping]] = ...) -> None: ...

class ListEntitlementsRequest(_message.Message):
    __slots__ = ("subject", "feature", "page_size", "page_token")
    SUBJECT_FIELD_NUMBER: _ClassVar[int]
    FEATURE_FIELD_NUMBER: _ClassVar[int]
    PAGE_SIZE_FIELD_NUMBER: _ClassVar[int]
    PAGE_TOKEN_FIELD_NUMBER: _ClassVar[int]
    subject: str
    feature: str
    page_size: int
    page_token: str
    def __init__(self, subject: _Optional[str] = ..., feature: _Optional[str] = ..., page_size: _Optional[int] = ..., page_token: _Optional[str] = ...) -> None: ...

class ListEntitlementsResponse(_message.Message):
    __slots__ = ("entitlements", "next_page_token")
    ENTITLEMENTS_FIELD_NUMBER: _ClassVar[int]
    NEXT_PAGE_TOKEN_FIELD_NUMBER: _ClassVar[int]
    entitlements: _containers.RepeatedCompositeFieldContainer[Entitlement]
    next_page_token: str
    def __init__(self, entitlements: _Optional[_Iterable[_Union[Entitlement, _Mapping]]] = ..., next_page_token: _Optional[str] = ...) -> None: ...

class GetEntitlementValueRequest(_message.Message):
    __slots__ = ("subject", "feature")
    SUBJECT_FIELD_NUMBER: _ClassVar[int]
    FEATURE_FIELD_NUMBER: _ClassVar[int]
    subject: str
    feature: str
    def __init__(self, subject: _Optional[str] = ..., feature: _Optional[str] = ...) -> None: ...

class GetEntitlementValueResponse(_message.Message):
    __slots__ = ("value",)
    VALUE_FIELD_NUMBER: _ClassVar[int]
    value: EntitlementValue
    def __init__(self, value: _Optional[_Union[EntitlementValue, _Mapping]] = ...) -> None: ...

class AccessCheck(_message.Message):
    __slots__ = ("subject", "feature")
    SUBJECT_FIELD_NUMBER: _ClassVar[int]
    FEATURE_FIELD_NUMBER: _ClassVar[int]
    subject: str
    feature: str
    def __init__(self, subject: _Optional[str] = ..., feature: _Optional[str] = ...) -> None: ...

class AccessResult(_message.Message):
    __slots__ = ("subject", "feature", "has_access", "reason")
    SUBJECT_FIELD_NUMBER: _ClassVar[int]
    FEATURE_FIELD_NUMBER: _ClassVar[int]
    HAS_ACCESS_FIELD_NUMBER: _ClassVar[int]
    REASON_FIELD_NUMBER: _ClassVar[int]
    subject: str
    feature: str
    has_access: bool
    reason: str
    def __init__(self, subject: _Optional[str] = ..., feature: _Optional[str] = ..., has_access: _Optional[bool] = ..., reason: _Optional[str] = ...) -> None: ...

class CheckAccessRequest(_message.Message):
    __slots__ = ("checks",)
    CHECKS_FIELD_NUMBER: _ClassVar[int]
    checks: _containers.RepeatedCompositeFieldContainer[AccessCheck]
    def __init__(self, checks: _Optional[_Iterable[_Union[AccessCheck, _Mapping]]] = ...) -> None: ...

class CheckAccessResponse(_message.Message):
    __slots__ = ("results",)
    RESULTS_FIELD_NUMBER: _ClassVar[int]
    results: _containers.RepeatedCompositeFieldContainer[AccessResult]
    def __init__(self, results: _Optional[_Iterable[_Union[AccessResult, _Mapping]]] = ...) -> None: ...

class CreateGrantRequest(_message.Message):
    __slots__ = ("grant", "reason", "idempotency_key")
    GRANT_FIELD_NUMBER: _ClassVar[int]
    REASON_FIELD_NUMBER: _ClassVar[int]
    IDEMPOTENCY_KEY_FIELD_NUMBER: _ClassVar[int]
    grant: Grant
    reason: str
    idempotency_key: str
    def __init__(self, grant: _Optional[_Union[Grant, _Mapping]] = ..., reason: _Optional[str] = ..., idempotency_key: _Optional[str] = ...) -> None: ...

class CreateGrantResponse(_message.Message):
    __slots__ = ("grant",)
    GRANT_FIELD_NUMBER: _ClassVar[int]
    grant: Grant
    def __init__(self, grant: _Optional[_Union[Grant, _Mapping]] = ...) -> None: ...

class VoidGrantRequest(_message.Message):
    __slots__ = ("id", "reason")
    ID_FIELD_NUMBER: _ClassVar[int]
    REASON_FIELD_NUMBER: _ClassVar[int]
    id: str
    reason: str
    def __init__(self, id: _Optional[str] = ..., reason: _Optional[str] = ...) -> None: ...

class VoidGrantResponse(_message.Message):
    __slots__ = ("grant",)
    GRANT_FIELD_NUMBER: _ClassVar[int]
    grant: Grant
    def __init__(self, grant: _Optional[_Union[Grant, _Mapping]] = ...) -> None: ...

class ListGrantsRequest(_message.Message):
    __slots__ = ("entitlement_id", "include_voided", "page_size", "page_token")
    ENTITLEMENT_ID_FIELD_NUMBER: _ClassVar[int]
    INCLUDE_VOIDED_FIELD_NUMBER: _ClassVar[int]
    PAGE_SIZE_FIELD_NUMBER: _ClassVar[int]
    PAGE_TOKEN_FIELD_NUMBER: _ClassVar[int]
    entitlement_id: str
    include_voided: bool
    page_size: int
    page_token: str
    def __init__(self, entitlement_id: _Optional[str] = ..., include_voided: _Optional[bool] = ..., page_size: _Optional[int] = ..., page_token: _Optional[str] = ...) -> None: ...

class ListGrantsResponse(_message.Message):
    __slots__ = ("grants", "next_page_token")
    GRANTS_FIELD_NUMBER: _ClassVar[int]
    NEXT_PAGE_TOKEN_FIELD_NUMBER: _ClassVar[int]
    grants: _containers.RepeatedCompositeFieldContainer[Grant]
    next_page_token: str
    def __init__(self, grants: _Optional[_Iterable[_Union[Grant, _Mapping]]] = ..., next_page_token: _Optional[str] = ...) -> None: ...

class ResetEntitlementRequest(_message.Message):
    __slots__ = ("id", "effective_time", "reason")
    ID_FIELD_NUMBER: _ClassVar[int]
    EFFECTIVE_TIME_FIELD_NUMBER: _ClassVar[int]
    REASON_FIELD_NUMBER: _ClassVar[int]
    id: str
    effective_time: _timestamp_pb2.Timestamp
    reason: str
    def __init__(self, id: _Optional[str] = ..., effective_time: _Optional[_Union[datetime.datetime, _timestamp_pb2.Timestamp, _Mapping]] = ..., reason: _Optional[str] = ...) -> None: ...

class ResetEntitlementResponse(_message.Message):
    __slots__ = ("entitlement",)
    ENTITLEMENT_FIELD_NUMBER: _ClassVar[int]
    entitlement: Entitlement
    def __init__(self, entitlement: _Optional[_Union[Entitlement, _Mapping]] = ...) -> None: ...

class GetBalanceHistoryRequest(_message.Message):
    __slots__ = ("id", "to")
    ID_FIELD_NUMBER: _ClassVar[int]
    FROM_FIELD_NUMBER: _ClassVar[int]
    TO_FIELD_NUMBER: _ClassVar[int]
    id: str
    to: _timestamp_pb2.Timestamp
    def __init__(self, id: _Optional[str] = ..., to: _Optional[_Union[datetime.datetime, _timestamp_pb2.Timestamp, _Mapping]] = ..., **kwargs) -> None: ...

class BalancePoint(_message.Message):
    __slots__ = ("time", "usage", "balance")
    TIME_FIELD_NUMBER: _ClassVar[int]
    USAGE_FIELD_NUMBER: _ClassVar[int]
    BALANCE_FIELD_NUMBER: _ClassVar[int]
    time: _timestamp_pb2.Timestamp
    usage: str
    balance: str
    def __init__(self, time: _Optional[_Union[datetime.datetime, _timestamp_pb2.Timestamp, _Mapping]] = ..., usage: _Optional[str] = ..., balance: _Optional[str] = ...) -> None: ...

class GetBalanceHistoryResponse(_message.Message):
    __slots__ = ("points",)
    POINTS_FIELD_NUMBER: _ClassVar[int]
    points: _containers.RepeatedCompositeFieldContainer[BalancePoint]
    def __init__(self, points: _Optional[_Iterable[_Union[BalancePoint, _Mapping]]] = ...) -> None: ...
