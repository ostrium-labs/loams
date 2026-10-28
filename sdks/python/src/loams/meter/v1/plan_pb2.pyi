import datetime

from google.protobuf import timestamp_pb2 as _timestamp_pb2
from loams.meter.v1 import customer_pb2 as _customer_pb2
from loams.options.v1 import options_pb2 as _options_pb2
from google.protobuf.internal import containers as _containers
from google.protobuf import descriptor as _descriptor
from google.protobuf import message as _message
from collections.abc import Iterable as _Iterable, Mapping as _Mapping
from typing import ClassVar as _ClassVar, Optional as _Optional, Union as _Union

DESCRIPTOR: _descriptor.FileDescriptor

class Plan(_message.Message):
    __slots__ = ("name", "version", "yaml", "features", "digest", "create_time")
    NAME_FIELD_NUMBER: _ClassVar[int]
    VERSION_FIELD_NUMBER: _ClassVar[int]
    YAML_FIELD_NUMBER: _ClassVar[int]
    FEATURES_FIELD_NUMBER: _ClassVar[int]
    DIGEST_FIELD_NUMBER: _ClassVar[int]
    CREATE_TIME_FIELD_NUMBER: _ClassVar[int]
    name: str
    version: int
    yaml: str
    features: _containers.RepeatedScalarFieldContainer[str]
    digest: str
    create_time: _timestamp_pb2.Timestamp
    def __init__(self, name: _Optional[str] = ..., version: _Optional[int] = ..., yaml: _Optional[str] = ..., features: _Optional[_Iterable[str]] = ..., digest: _Optional[str] = ..., create_time: _Optional[_Union[datetime.datetime, _timestamp_pb2.Timestamp, _Mapping]] = ...) -> None: ...

class ApplyPlanRequest(_message.Message):
    __slots__ = ("yaml", "reason")
    YAML_FIELD_NUMBER: _ClassVar[int]
    REASON_FIELD_NUMBER: _ClassVar[int]
    yaml: str
    reason: str
    def __init__(self, yaml: _Optional[str] = ..., reason: _Optional[str] = ...) -> None: ...

class ApplyPlanResponse(_message.Message):
    __slots__ = ("plan", "changed")
    PLAN_FIELD_NUMBER: _ClassVar[int]
    CHANGED_FIELD_NUMBER: _ClassVar[int]
    plan: Plan
    changed: bool
    def __init__(self, plan: _Optional[_Union[Plan, _Mapping]] = ..., changed: _Optional[bool] = ...) -> None: ...

class ListPlansRequest(_message.Message):
    __slots__ = ("name", "all_versions")
    NAME_FIELD_NUMBER: _ClassVar[int]
    ALL_VERSIONS_FIELD_NUMBER: _ClassVar[int]
    name: str
    all_versions: bool
    def __init__(self, name: _Optional[str] = ..., all_versions: _Optional[bool] = ...) -> None: ...

class ListPlansResponse(_message.Message):
    __slots__ = ("plans",)
    PLANS_FIELD_NUMBER: _ClassVar[int]
    plans: _containers.RepeatedCompositeFieldContainer[Plan]
    def __init__(self, plans: _Optional[_Iterable[_Union[Plan, _Mapping]]] = ...) -> None: ...

class AssignPlanRequest(_message.Message):
    __slots__ = ("customer_id", "plan", "version", "reason")
    CUSTOMER_ID_FIELD_NUMBER: _ClassVar[int]
    PLAN_FIELD_NUMBER: _ClassVar[int]
    VERSION_FIELD_NUMBER: _ClassVar[int]
    REASON_FIELD_NUMBER: _ClassVar[int]
    customer_id: str
    plan: str
    version: int
    reason: str
    def __init__(self, customer_id: _Optional[str] = ..., plan: _Optional[str] = ..., version: _Optional[int] = ..., reason: _Optional[str] = ...) -> None: ...

class AssignPlanResponse(_message.Message):
    __slots__ = ("customer", "entitlements_changed")
    CUSTOMER_FIELD_NUMBER: _ClassVar[int]
    ENTITLEMENTS_CHANGED_FIELD_NUMBER: _ClassVar[int]
    customer: _customer_pb2.Customer
    entitlements_changed: int
    def __init__(self, customer: _Optional[_Union[_customer_pb2.Customer, _Mapping]] = ..., entitlements_changed: _Optional[int] = ...) -> None: ...

class UnassignPlanRequest(_message.Message):
    __slots__ = ("customer_id", "reason")
    CUSTOMER_ID_FIELD_NUMBER: _ClassVar[int]
    REASON_FIELD_NUMBER: _ClassVar[int]
    customer_id: str
    reason: str
    def __init__(self, customer_id: _Optional[str] = ..., reason: _Optional[str] = ...) -> None: ...

class UnassignPlanResponse(_message.Message):
    __slots__ = ("customer",)
    CUSTOMER_FIELD_NUMBER: _ClassVar[int]
    customer: _customer_pb2.Customer
    def __init__(self, customer: _Optional[_Union[_customer_pb2.Customer, _Mapping]] = ...) -> None: ...
