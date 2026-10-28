import datetime

from google.protobuf import field_mask_pb2 as _field_mask_pb2
from google.protobuf import timestamp_pb2 as _timestamp_pb2
from loams.options.v1 import options_pb2 as _options_pb2
from google.protobuf.internal import containers as _containers
from google.protobuf.internal import enum_type_wrapper as _enum_type_wrapper
from google.protobuf import descriptor as _descriptor
from google.protobuf import message as _message
from collections.abc import Iterable as _Iterable, Mapping as _Mapping
from typing import ClassVar as _ClassVar, Optional as _Optional, Union as _Union

DESCRIPTOR: _descriptor.FileDescriptor

class PaymentState(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    PAYMENT_STATE_UNSPECIFIED: _ClassVar[PaymentState]
    PAYMENT_STATE_NONE: _ClassVar[PaymentState]
    PAYMENT_STATE_CARD: _ClassVar[PaymentState]
    PAYMENT_STATE_INVOICE: _ClassVar[PaymentState]
PAYMENT_STATE_UNSPECIFIED: PaymentState
PAYMENT_STATE_NONE: PaymentState
PAYMENT_STATE_CARD: PaymentState
PAYMENT_STATE_INVOICE: PaymentState

class Money(_message.Message):
    __slots__ = ("micros", "currency")
    MICROS_FIELD_NUMBER: _ClassVar[int]
    CURRENCY_FIELD_NUMBER: _ClassVar[int]
    micros: int
    currency: str
    def __init__(self, micros: _Optional[int] = ..., currency: _Optional[str] = ...) -> None: ...

class SpendControls(_message.Message):
    __slots__ = ("overage_enabled", "spend_cap", "auto_top_up", "top_up_amount", "top_up_threshold")
    OVERAGE_ENABLED_FIELD_NUMBER: _ClassVar[int]
    SPEND_CAP_FIELD_NUMBER: _ClassVar[int]
    AUTO_TOP_UP_FIELD_NUMBER: _ClassVar[int]
    TOP_UP_AMOUNT_FIELD_NUMBER: _ClassVar[int]
    TOP_UP_THRESHOLD_FIELD_NUMBER: _ClassVar[int]
    overage_enabled: bool
    spend_cap: Money
    auto_top_up: bool
    top_up_amount: Money
    top_up_threshold: Money
    def __init__(self, overage_enabled: _Optional[bool] = ..., spend_cap: _Optional[_Union[Money, _Mapping]] = ..., auto_top_up: _Optional[bool] = ..., top_up_amount: _Optional[_Union[Money, _Mapping]] = ..., top_up_threshold: _Optional[_Union[Money, _Mapping]] = ...) -> None: ...

class Customer(_message.Message):
    __slots__ = ("id", "org", "display_name", "subjects", "payment_state", "spend_controls", "stripe_customer_id", "currency", "plan", "plan_version", "create_time", "update_time")
    ID_FIELD_NUMBER: _ClassVar[int]
    ORG_FIELD_NUMBER: _ClassVar[int]
    DISPLAY_NAME_FIELD_NUMBER: _ClassVar[int]
    SUBJECTS_FIELD_NUMBER: _ClassVar[int]
    PAYMENT_STATE_FIELD_NUMBER: _ClassVar[int]
    SPEND_CONTROLS_FIELD_NUMBER: _ClassVar[int]
    STRIPE_CUSTOMER_ID_FIELD_NUMBER: _ClassVar[int]
    CURRENCY_FIELD_NUMBER: _ClassVar[int]
    PLAN_FIELD_NUMBER: _ClassVar[int]
    PLAN_VERSION_FIELD_NUMBER: _ClassVar[int]
    CREATE_TIME_FIELD_NUMBER: _ClassVar[int]
    UPDATE_TIME_FIELD_NUMBER: _ClassVar[int]
    id: str
    org: str
    display_name: str
    subjects: _containers.RepeatedScalarFieldContainer[str]
    payment_state: PaymentState
    spend_controls: SpendControls
    stripe_customer_id: str
    currency: str
    plan: str
    plan_version: int
    create_time: _timestamp_pb2.Timestamp
    update_time: _timestamp_pb2.Timestamp
    def __init__(self, id: _Optional[str] = ..., org: _Optional[str] = ..., display_name: _Optional[str] = ..., subjects: _Optional[_Iterable[str]] = ..., payment_state: _Optional[_Union[PaymentState, str]] = ..., spend_controls: _Optional[_Union[SpendControls, _Mapping]] = ..., stripe_customer_id: _Optional[str] = ..., currency: _Optional[str] = ..., plan: _Optional[str] = ..., plan_version: _Optional[int] = ..., create_time: _Optional[_Union[datetime.datetime, _timestamp_pb2.Timestamp, _Mapping]] = ..., update_time: _Optional[_Union[datetime.datetime, _timestamp_pb2.Timestamp, _Mapping]] = ...) -> None: ...

class CreateCustomerRequest(_message.Message):
    __slots__ = ("org", "display_name", "subjects", "currency", "stripe_customer_id", "idempotency_key")
    ORG_FIELD_NUMBER: _ClassVar[int]
    DISPLAY_NAME_FIELD_NUMBER: _ClassVar[int]
    SUBJECTS_FIELD_NUMBER: _ClassVar[int]
    CURRENCY_FIELD_NUMBER: _ClassVar[int]
    STRIPE_CUSTOMER_ID_FIELD_NUMBER: _ClassVar[int]
    IDEMPOTENCY_KEY_FIELD_NUMBER: _ClassVar[int]
    org: str
    display_name: str
    subjects: _containers.RepeatedScalarFieldContainer[str]
    currency: str
    stripe_customer_id: str
    idempotency_key: str
    def __init__(self, org: _Optional[str] = ..., display_name: _Optional[str] = ..., subjects: _Optional[_Iterable[str]] = ..., currency: _Optional[str] = ..., stripe_customer_id: _Optional[str] = ..., idempotency_key: _Optional[str] = ...) -> None: ...

class CreateCustomerResponse(_message.Message):
    __slots__ = ("customer",)
    CUSTOMER_FIELD_NUMBER: _ClassVar[int]
    customer: Customer
    def __init__(self, customer: _Optional[_Union[Customer, _Mapping]] = ...) -> None: ...

class GetCustomerRequest(_message.Message):
    __slots__ = ("id",)
    ID_FIELD_NUMBER: _ClassVar[int]
    id: str
    def __init__(self, id: _Optional[str] = ...) -> None: ...

class GetCustomerResponse(_message.Message):
    __slots__ = ("customer",)
    CUSTOMER_FIELD_NUMBER: _ClassVar[int]
    customer: Customer
    def __init__(self, customer: _Optional[_Union[Customer, _Mapping]] = ...) -> None: ...

class ListCustomersRequest(_message.Message):
    __slots__ = ("org", "page_size", "page_token")
    ORG_FIELD_NUMBER: _ClassVar[int]
    PAGE_SIZE_FIELD_NUMBER: _ClassVar[int]
    PAGE_TOKEN_FIELD_NUMBER: _ClassVar[int]
    org: str
    page_size: int
    page_token: str
    def __init__(self, org: _Optional[str] = ..., page_size: _Optional[int] = ..., page_token: _Optional[str] = ...) -> None: ...

class ListCustomersResponse(_message.Message):
    __slots__ = ("customers", "next_page_token")
    CUSTOMERS_FIELD_NUMBER: _ClassVar[int]
    NEXT_PAGE_TOKEN_FIELD_NUMBER: _ClassVar[int]
    customers: _containers.RepeatedCompositeFieldContainer[Customer]
    next_page_token: str
    def __init__(self, customers: _Optional[_Iterable[_Union[Customer, _Mapping]]] = ..., next_page_token: _Optional[str] = ...) -> None: ...

class UpdateCustomerRequest(_message.Message):
    __slots__ = ("id", "customer", "update_mask", "reason")
    ID_FIELD_NUMBER: _ClassVar[int]
    CUSTOMER_FIELD_NUMBER: _ClassVar[int]
    UPDATE_MASK_FIELD_NUMBER: _ClassVar[int]
    REASON_FIELD_NUMBER: _ClassVar[int]
    id: str
    customer: Customer
    update_mask: _field_mask_pb2.FieldMask
    reason: str
    def __init__(self, id: _Optional[str] = ..., customer: _Optional[_Union[Customer, _Mapping]] = ..., update_mask: _Optional[_Union[_field_mask_pb2.FieldMask, _Mapping]] = ..., reason: _Optional[str] = ...) -> None: ...

class UpdateCustomerResponse(_message.Message):
    __slots__ = ("customer",)
    CUSTOMER_FIELD_NUMBER: _ClassVar[int]
    customer: Customer
    def __init__(self, customer: _Optional[_Union[Customer, _Mapping]] = ...) -> None: ...

class SetPaymentStateRequest(_message.Message):
    __slots__ = ("id", "stripe_customer_id", "payment_state", "reason")
    ID_FIELD_NUMBER: _ClassVar[int]
    STRIPE_CUSTOMER_ID_FIELD_NUMBER: _ClassVar[int]
    PAYMENT_STATE_FIELD_NUMBER: _ClassVar[int]
    REASON_FIELD_NUMBER: _ClassVar[int]
    id: str
    stripe_customer_id: str
    payment_state: PaymentState
    reason: str
    def __init__(self, id: _Optional[str] = ..., stripe_customer_id: _Optional[str] = ..., payment_state: _Optional[_Union[PaymentState, str]] = ..., reason: _Optional[str] = ...) -> None: ...

class SetPaymentStateResponse(_message.Message):
    __slots__ = ("customer",)
    CUSTOMER_FIELD_NUMBER: _ClassVar[int]
    customer: Customer
    def __init__(self, customer: _Optional[_Union[Customer, _Mapping]] = ...) -> None: ...

class SetSpendControlsRequest(_message.Message):
    __slots__ = ("id", "spend_controls", "reason")
    ID_FIELD_NUMBER: _ClassVar[int]
    SPEND_CONTROLS_FIELD_NUMBER: _ClassVar[int]
    REASON_FIELD_NUMBER: _ClassVar[int]
    id: str
    spend_controls: SpendControls
    reason: str
    def __init__(self, id: _Optional[str] = ..., spend_controls: _Optional[_Union[SpendControls, _Mapping]] = ..., reason: _Optional[str] = ...) -> None: ...

class SetSpendControlsResponse(_message.Message):
    __slots__ = ("customer",)
    CUSTOMER_FIELD_NUMBER: _ClassVar[int]
    customer: Customer
    def __init__(self, customer: _Optional[_Union[Customer, _Mapping]] = ...) -> None: ...
