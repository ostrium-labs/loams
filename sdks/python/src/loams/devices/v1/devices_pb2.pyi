import datetime

from google.protobuf import timestamp_pb2 as _timestamp_pb2
from google.protobuf.internal import containers as _containers
from google.protobuf.internal import enum_type_wrapper as _enum_type_wrapper
from google.protobuf import descriptor as _descriptor
from google.protobuf import message as _message
from collections.abc import Iterable as _Iterable, Mapping as _Mapping
from typing import ClassVar as _ClassVar, Optional as _Optional, Union as _Union

DESCRIPTOR: _descriptor.FileDescriptor

class Platform(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    PLATFORM_UNSPECIFIED: _ClassVar[Platform]
    PLATFORM_IOS: _ClassVar[Platform]
    PLATFORM_ANDROID: _ClassVar[Platform]
    PLATFORM_DESKTOP: _ClassVar[Platform]

class PushProvider(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    PUSH_PROVIDER_UNSPECIFIED: _ClassVar[PushProvider]
    PUSH_PROVIDER_APNS: _ClassVar[PushProvider]
    PUSH_PROVIDER_FCM: _ClassVar[PushProvider]
    PUSH_PROVIDER_UNIFIEDPUSH: _ClassVar[PushProvider]
    PUSH_PROVIDER_WEBPUSH: _ClassVar[PushProvider]

class ApnsEnvironment(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    APNS_ENVIRONMENT_UNSPECIFIED: _ClassVar[ApnsEnvironment]
    APNS_ENVIRONMENT_SANDBOX: _ClassVar[ApnsEnvironment]
    APNS_ENVIRONMENT_PRODUCTION: _ClassVar[ApnsEnvironment]

class NotificationCategory(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    NOTIFICATION_CATEGORY_UNSPECIFIED: _ClassVar[NotificationCategory]
    NOTIFICATION_CATEGORY_APPROVALS: _ClassVar[NotificationCategory]
    NOTIFICATION_CATEGORY_OPERATIONS: _ClassVar[NotificationCategory]
    NOTIFICATION_CATEGORY_JOBS: _ClassVar[NotificationCategory]
    NOTIFICATION_CATEGORY_RUNS: _ClassVar[NotificationCategory]
    NOTIFICATION_CATEGORY_SECURITY: _ClassVar[NotificationCategory]
PLATFORM_UNSPECIFIED: Platform
PLATFORM_IOS: Platform
PLATFORM_ANDROID: Platform
PLATFORM_DESKTOP: Platform
PUSH_PROVIDER_UNSPECIFIED: PushProvider
PUSH_PROVIDER_APNS: PushProvider
PUSH_PROVIDER_FCM: PushProvider
PUSH_PROVIDER_UNIFIEDPUSH: PushProvider
PUSH_PROVIDER_WEBPUSH: PushProvider
APNS_ENVIRONMENT_UNSPECIFIED: ApnsEnvironment
APNS_ENVIRONMENT_SANDBOX: ApnsEnvironment
APNS_ENVIRONMENT_PRODUCTION: ApnsEnvironment
NOTIFICATION_CATEGORY_UNSPECIFIED: NotificationCategory
NOTIFICATION_CATEGORY_APPROVALS: NotificationCategory
NOTIFICATION_CATEGORY_OPERATIONS: NotificationCategory
NOTIFICATION_CATEGORY_JOBS: NotificationCategory
NOTIFICATION_CATEGORY_RUNS: NotificationCategory
NOTIFICATION_CATEGORY_SECURITY: NotificationCategory

class CreatePairingRequest(_message.Message):
    __slots__ = ("device_name_hint", "environments", "idempotency_key")
    DEVICE_NAME_HINT_FIELD_NUMBER: _ClassVar[int]
    ENVIRONMENTS_FIELD_NUMBER: _ClassVar[int]
    IDEMPOTENCY_KEY_FIELD_NUMBER: _ClassVar[int]
    device_name_hint: str
    environments: _containers.RepeatedScalarFieldContainer[str]
    idempotency_key: str
    def __init__(self, device_name_hint: _Optional[str] = ..., environments: _Optional[_Iterable[str]] = ..., idempotency_key: _Optional[str] = ...) -> None: ...

class CreatePairingResponse(_message.Message):
    __slots__ = ("pairing_id", "qr_payload", "user_code", "expires_at")
    PAIRING_ID_FIELD_NUMBER: _ClassVar[int]
    QR_PAYLOAD_FIELD_NUMBER: _ClassVar[int]
    USER_CODE_FIELD_NUMBER: _ClassVar[int]
    EXPIRES_AT_FIELD_NUMBER: _ClassVar[int]
    pairing_id: str
    qr_payload: str
    user_code: str
    expires_at: _timestamp_pb2.Timestamp
    def __init__(self, pairing_id: _Optional[str] = ..., qr_payload: _Optional[str] = ..., user_code: _Optional[str] = ..., expires_at: _Optional[_Union[datetime.datetime, _timestamp_pb2.Timestamp, _Mapping]] = ...) -> None: ...

class Device(_message.Message):
    __slots__ = ("id", "name", "platform", "model", "app_version", "created_at", "last_seen_at", "decision_key_thumbprint", "push_targets", "revoked_at")
    ID_FIELD_NUMBER: _ClassVar[int]
    NAME_FIELD_NUMBER: _ClassVar[int]
    PLATFORM_FIELD_NUMBER: _ClassVar[int]
    MODEL_FIELD_NUMBER: _ClassVar[int]
    APP_VERSION_FIELD_NUMBER: _ClassVar[int]
    CREATED_AT_FIELD_NUMBER: _ClassVar[int]
    LAST_SEEN_AT_FIELD_NUMBER: _ClassVar[int]
    DECISION_KEY_THUMBPRINT_FIELD_NUMBER: _ClassVar[int]
    PUSH_TARGETS_FIELD_NUMBER: _ClassVar[int]
    REVOKED_AT_FIELD_NUMBER: _ClassVar[int]
    id: str
    name: str
    platform: Platform
    model: str
    app_version: str
    created_at: _timestamp_pb2.Timestamp
    last_seen_at: _timestamp_pb2.Timestamp
    decision_key_thumbprint: str
    push_targets: _containers.RepeatedCompositeFieldContainer[PushTargetRef]
    revoked_at: _timestamp_pb2.Timestamp
    def __init__(self, id: _Optional[str] = ..., name: _Optional[str] = ..., platform: _Optional[_Union[Platform, str]] = ..., model: _Optional[str] = ..., app_version: _Optional[str] = ..., created_at: _Optional[_Union[datetime.datetime, _timestamp_pb2.Timestamp, _Mapping]] = ..., last_seen_at: _Optional[_Union[datetime.datetime, _timestamp_pb2.Timestamp, _Mapping]] = ..., decision_key_thumbprint: _Optional[str] = ..., push_targets: _Optional[_Iterable[_Union[PushTargetRef, _Mapping]]] = ..., revoked_at: _Optional[_Union[datetime.datetime, _timestamp_pb2.Timestamp, _Mapping]] = ...) -> None: ...

class PushTargetRef(_message.Message):
    __slots__ = ("id", "provider", "registered_at")
    ID_FIELD_NUMBER: _ClassVar[int]
    PROVIDER_FIELD_NUMBER: _ClassVar[int]
    REGISTERED_AT_FIELD_NUMBER: _ClassVar[int]
    id: str
    provider: PushProvider
    registered_at: _timestamp_pb2.Timestamp
    def __init__(self, id: _Optional[str] = ..., provider: _Optional[_Union[PushProvider, str]] = ..., registered_at: _Optional[_Union[datetime.datetime, _timestamp_pb2.Timestamp, _Mapping]] = ...) -> None: ...

class ListDevicesRequest(_message.Message):
    __slots__ = ("user_id", "include_revoked")
    USER_ID_FIELD_NUMBER: _ClassVar[int]
    INCLUDE_REVOKED_FIELD_NUMBER: _ClassVar[int]
    user_id: str
    include_revoked: bool
    def __init__(self, user_id: _Optional[str] = ..., include_revoked: _Optional[bool] = ...) -> None: ...

class ListDevicesResponse(_message.Message):
    __slots__ = ("devices",)
    DEVICES_FIELD_NUMBER: _ClassVar[int]
    devices: _containers.RepeatedCompositeFieldContainer[Device]
    def __init__(self, devices: _Optional[_Iterable[_Union[Device, _Mapping]]] = ...) -> None: ...

class RenameDeviceRequest(_message.Message):
    __slots__ = ("device_id", "name", "idempotency_key")
    DEVICE_ID_FIELD_NUMBER: _ClassVar[int]
    NAME_FIELD_NUMBER: _ClassVar[int]
    IDEMPOTENCY_KEY_FIELD_NUMBER: _ClassVar[int]
    device_id: str
    name: str
    idempotency_key: str
    def __init__(self, device_id: _Optional[str] = ..., name: _Optional[str] = ..., idempotency_key: _Optional[str] = ...) -> None: ...

class RenameDeviceResponse(_message.Message):
    __slots__ = ("device",)
    DEVICE_FIELD_NUMBER: _ClassVar[int]
    device: Device
    def __init__(self, device: _Optional[_Union[Device, _Mapping]] = ...) -> None: ...

class RevokeDeviceRequest(_message.Message):
    __slots__ = ("device_id", "idempotency_key")
    DEVICE_ID_FIELD_NUMBER: _ClassVar[int]
    IDEMPOTENCY_KEY_FIELD_NUMBER: _ClassVar[int]
    device_id: str
    idempotency_key: str
    def __init__(self, device_id: _Optional[str] = ..., idempotency_key: _Optional[str] = ...) -> None: ...

class RevokeDeviceResponse(_message.Message):
    __slots__ = ("device",)
    DEVICE_FIELD_NUMBER: _ClassVar[int]
    device: Device
    def __init__(self, device: _Optional[_Union[Device, _Mapping]] = ...) -> None: ...

class RegisterPushTargetRequest(_message.Message):
    __slots__ = ("device_id", "provider", "token_or_endpoint", "app_id", "apns_environment", "hpke_public_key", "idempotency_key")
    DEVICE_ID_FIELD_NUMBER: _ClassVar[int]
    PROVIDER_FIELD_NUMBER: _ClassVar[int]
    TOKEN_OR_ENDPOINT_FIELD_NUMBER: _ClassVar[int]
    APP_ID_FIELD_NUMBER: _ClassVar[int]
    APNS_ENVIRONMENT_FIELD_NUMBER: _ClassVar[int]
    HPKE_PUBLIC_KEY_FIELD_NUMBER: _ClassVar[int]
    IDEMPOTENCY_KEY_FIELD_NUMBER: _ClassVar[int]
    device_id: str
    provider: PushProvider
    token_or_endpoint: str
    app_id: str
    apns_environment: ApnsEnvironment
    hpke_public_key: bytes
    idempotency_key: str
    def __init__(self, device_id: _Optional[str] = ..., provider: _Optional[_Union[PushProvider, str]] = ..., token_or_endpoint: _Optional[str] = ..., app_id: _Optional[str] = ..., apns_environment: _Optional[_Union[ApnsEnvironment, str]] = ..., hpke_public_key: _Optional[bytes] = ..., idempotency_key: _Optional[str] = ...) -> None: ...

class RegisterPushTargetResponse(_message.Message):
    __slots__ = ("target",)
    TARGET_FIELD_NUMBER: _ClassVar[int]
    target: PushTargetRef
    def __init__(self, target: _Optional[_Union[PushTargetRef, _Mapping]] = ...) -> None: ...

class UnregisterPushTargetRequest(_message.Message):
    __slots__ = ("target_id", "idempotency_key")
    TARGET_ID_FIELD_NUMBER: _ClassVar[int]
    IDEMPOTENCY_KEY_FIELD_NUMBER: _ClassVar[int]
    target_id: str
    idempotency_key: str
    def __init__(self, target_id: _Optional[str] = ..., idempotency_key: _Optional[str] = ...) -> None: ...

class UnregisterPushTargetResponse(_message.Message):
    __slots__ = ()
    def __init__(self) -> None: ...

class GetNotificationPreferencesRequest(_message.Message):
    __slots__ = ()
    def __init__(self) -> None: ...

class GetNotificationPreferencesResponse(_message.Message):
    __slots__ = ("preferences",)
    PREFERENCES_FIELD_NUMBER: _ClassVar[int]
    preferences: NotificationPreferences
    def __init__(self, preferences: _Optional[_Union[NotificationPreferences, _Mapping]] = ...) -> None: ...

class SetNotificationPreferencesRequest(_message.Message):
    __slots__ = ("preferences", "idempotency_key")
    PREFERENCES_FIELD_NUMBER: _ClassVar[int]
    IDEMPOTENCY_KEY_FIELD_NUMBER: _ClassVar[int]
    preferences: NotificationPreferences
    idempotency_key: str
    def __init__(self, preferences: _Optional[_Union[NotificationPreferences, _Mapping]] = ..., idempotency_key: _Optional[str] = ...) -> None: ...

class SetNotificationPreferencesResponse(_message.Message):
    __slots__ = ("preferences",)
    PREFERENCES_FIELD_NUMBER: _ClassVar[int]
    preferences: NotificationPreferences
    def __init__(self, preferences: _Optional[_Union[NotificationPreferences, _Mapping]] = ...) -> None: ...

class NotificationPreferences(_message.Message):
    __slots__ = ("categories", "environments", "quiet_hours", "approvals_bypass_quiet_hours")
    CATEGORIES_FIELD_NUMBER: _ClassVar[int]
    ENVIRONMENTS_FIELD_NUMBER: _ClassVar[int]
    QUIET_HOURS_FIELD_NUMBER: _ClassVar[int]
    APPROVALS_BYPASS_QUIET_HOURS_FIELD_NUMBER: _ClassVar[int]
    categories: _containers.RepeatedCompositeFieldContainer[CategoryPreference]
    environments: _containers.RepeatedScalarFieldContainer[str]
    quiet_hours: QuietHours
    approvals_bypass_quiet_hours: bool
    def __init__(self, categories: _Optional[_Iterable[_Union[CategoryPreference, _Mapping]]] = ..., environments: _Optional[_Iterable[str]] = ..., quiet_hours: _Optional[_Union[QuietHours, _Mapping]] = ..., approvals_bypass_quiet_hours: _Optional[bool] = ...) -> None: ...

class CategoryPreference(_message.Message):
    __slots__ = ("category", "enabled")
    CATEGORY_FIELD_NUMBER: _ClassVar[int]
    ENABLED_FIELD_NUMBER: _ClassVar[int]
    category: NotificationCategory
    enabled: bool
    def __init__(self, category: _Optional[_Union[NotificationCategory, str]] = ..., enabled: _Optional[bool] = ...) -> None: ...

class QuietHours(_message.Message):
    __slots__ = ("start", "end", "time_zone")
    START_FIELD_NUMBER: _ClassVar[int]
    END_FIELD_NUMBER: _ClassVar[int]
    TIME_ZONE_FIELD_NUMBER: _ClassVar[int]
    start: str
    end: str
    time_zone: str
    def __init__(self, start: _Optional[str] = ..., end: _Optional[str] = ..., time_zone: _Optional[str] = ...) -> None: ...

class SendTestNotificationRequest(_message.Message):
    __slots__ = ("device_id", "idempotency_key")
    DEVICE_ID_FIELD_NUMBER: _ClassVar[int]
    IDEMPOTENCY_KEY_FIELD_NUMBER: _ClassVar[int]
    device_id: str
    idempotency_key: str
    def __init__(self, device_id: _Optional[str] = ..., idempotency_key: _Optional[str] = ...) -> None: ...

class SendTestNotificationResponse(_message.Message):
    __slots__ = ("delivered",)
    DELIVERED_FIELD_NUMBER: _ClassVar[int]
    delivered: int
    def __init__(self, delivered: _Optional[int] = ...) -> None: ...
