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

class Edition(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    EDITION_UNSPECIFIED: _ClassVar[Edition]
    EDITION_OSS: _ClassVar[Edition]
    EDITION_CLOUD: _ClassVar[Edition]
    EDITION_BYOC: _ClassVar[Edition]

class SignInKind(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    SIGN_IN_KIND_UNSPECIFIED: _ClassVar[SignInKind]
    SIGN_IN_KIND_NONE: _ClassVar[SignInKind]
    SIGN_IN_KIND_AUTHENTIK: _ClassVar[SignInKind]
    SIGN_IN_KIND_AUTHENTIK_DEVICE_CODE: _ClassVar[SignInKind]
    SIGN_IN_KIND_PAIRING: _ClassVar[SignInKind]

class PrincipalKind(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    PRINCIPAL_KIND_UNSPECIFIED: _ClassVar[PrincipalKind]
    PRINCIPAL_KIND_USER: _ClassVar[PrincipalKind]
    PRINCIPAL_KIND_SERVICE_ACCOUNT: _ClassVar[PrincipalKind]
    PRINCIPAL_KIND_AGENT: _ClassVar[PrincipalKind]
EDITION_UNSPECIFIED: Edition
EDITION_OSS: Edition
EDITION_CLOUD: Edition
EDITION_BYOC: Edition
SIGN_IN_KIND_UNSPECIFIED: SignInKind
SIGN_IN_KIND_NONE: SignInKind
SIGN_IN_KIND_AUTHENTIK: SignInKind
SIGN_IN_KIND_AUTHENTIK_DEVICE_CODE: SignInKind
SIGN_IN_KIND_PAIRING: SignInKind
PRINCIPAL_KIND_UNSPECIFIED: PrincipalKind
PRINCIPAL_KIND_USER: PrincipalKind
PRINCIPAL_KIND_SERVICE_ACCOUNT: PrincipalKind
PRINCIPAL_KIND_AGENT: PrincipalKind

class GetInstanceRequest(_message.Message):
    __slots__ = ()
    def __init__(self) -> None: ...

class GetInstanceResponse(_message.Message):
    __slots__ = ("instance_id", "name", "edition", "server_version", "api_versions", "features", "issuer", "jwks_uri", "sign_in_methods", "tls_pins", "push", "min_app_versions", "setup_required", "key_rotation", "services")
    class FeaturesEntry(_message.Message):
        __slots__ = ("key", "value")
        KEY_FIELD_NUMBER: _ClassVar[int]
        VALUE_FIELD_NUMBER: _ClassVar[int]
        key: str
        value: bool
        def __init__(self, key: _Optional[str] = ..., value: _Optional[bool] = ...) -> None: ...
    class MinAppVersionsEntry(_message.Message):
        __slots__ = ("key", "value")
        KEY_FIELD_NUMBER: _ClassVar[int]
        VALUE_FIELD_NUMBER: _ClassVar[int]
        key: str
        value: str
        def __init__(self, key: _Optional[str] = ..., value: _Optional[str] = ...) -> None: ...
    INSTANCE_ID_FIELD_NUMBER: _ClassVar[int]
    NAME_FIELD_NUMBER: _ClassVar[int]
    EDITION_FIELD_NUMBER: _ClassVar[int]
    SERVER_VERSION_FIELD_NUMBER: _ClassVar[int]
    API_VERSIONS_FIELD_NUMBER: _ClassVar[int]
    FEATURES_FIELD_NUMBER: _ClassVar[int]
    ISSUER_FIELD_NUMBER: _ClassVar[int]
    JWKS_URI_FIELD_NUMBER: _ClassVar[int]
    SIGN_IN_METHODS_FIELD_NUMBER: _ClassVar[int]
    TLS_PINS_FIELD_NUMBER: _ClassVar[int]
    PUSH_FIELD_NUMBER: _ClassVar[int]
    MIN_APP_VERSIONS_FIELD_NUMBER: _ClassVar[int]
    SETUP_REQUIRED_FIELD_NUMBER: _ClassVar[int]
    KEY_ROTATION_FIELD_NUMBER: _ClassVar[int]
    SERVICES_FIELD_NUMBER: _ClassVar[int]
    instance_id: str
    name: str
    edition: Edition
    server_version: str
    api_versions: _containers.RepeatedScalarFieldContainer[str]
    features: _containers.ScalarMap[str, bool]
    issuer: str
    jwks_uri: str
    sign_in_methods: _containers.RepeatedCompositeFieldContainer[SignInMethod]
    tls_pins: str
    push: PushConfig
    min_app_versions: _containers.ScalarMap[str, str]
    setup_required: bool
    key_rotation: str
    services: _containers.RepeatedCompositeFieldContainer[ServiceStatus]
    def __init__(self, instance_id: _Optional[str] = ..., name: _Optional[str] = ..., edition: _Optional[_Union[Edition, str]] = ..., server_version: _Optional[str] = ..., api_versions: _Optional[_Iterable[str]] = ..., features: _Optional[_Mapping[str, bool]] = ..., issuer: _Optional[str] = ..., jwks_uri: _Optional[str] = ..., sign_in_methods: _Optional[_Iterable[_Union[SignInMethod, _Mapping]]] = ..., tls_pins: _Optional[str] = ..., push: _Optional[_Union[PushConfig, _Mapping]] = ..., min_app_versions: _Optional[_Mapping[str, str]] = ..., setup_required: _Optional[bool] = ..., key_rotation: _Optional[str] = ..., services: _Optional[_Iterable[_Union[ServiceStatus, _Mapping]]] = ...) -> None: ...

class ServiceStatus(_message.Message):
    __slots__ = ("package", "version", "available", "services", "unstable")
    PACKAGE_FIELD_NUMBER: _ClassVar[int]
    VERSION_FIELD_NUMBER: _ClassVar[int]
    AVAILABLE_FIELD_NUMBER: _ClassVar[int]
    SERVICES_FIELD_NUMBER: _ClassVar[int]
    UNSTABLE_FIELD_NUMBER: _ClassVar[int]
    package: str
    version: str
    available: bool
    services: _containers.RepeatedScalarFieldContainer[str]
    unstable: bool
    def __init__(self, package: _Optional[str] = ..., version: _Optional[str] = ..., available: _Optional[bool] = ..., services: _Optional[_Iterable[str]] = ..., unstable: _Optional[bool] = ...) -> None: ...

class SignInMethod(_message.Message):
    __slots__ = ("kind", "issuer", "client_id", "display_name")
    KIND_FIELD_NUMBER: _ClassVar[int]
    ISSUER_FIELD_NUMBER: _ClassVar[int]
    CLIENT_ID_FIELD_NUMBER: _ClassVar[int]
    DISPLAY_NAME_FIELD_NUMBER: _ClassVar[int]
    kind: SignInKind
    issuer: str
    client_id: str
    display_name: str
    def __init__(self, kind: _Optional[_Union[SignInKind, str]] = ..., issuer: _Optional[str] = ..., client_id: _Optional[str] = ..., display_name: _Optional[str] = ...) -> None: ...

class PushConfig(_message.Message):
    __slots__ = ("gateway_url", "app_ids", "unifiedpush")
    class AppIdsEntry(_message.Message):
        __slots__ = ("key", "value")
        KEY_FIELD_NUMBER: _ClassVar[int]
        VALUE_FIELD_NUMBER: _ClassVar[int]
        key: str
        value: str
        def __init__(self, key: _Optional[str] = ..., value: _Optional[str] = ...) -> None: ...
    GATEWAY_URL_FIELD_NUMBER: _ClassVar[int]
    APP_IDS_FIELD_NUMBER: _ClassVar[int]
    UNIFIEDPUSH_FIELD_NUMBER: _ClassVar[int]
    gateway_url: str
    app_ids: _containers.ScalarMap[str, str]
    unifiedpush: bool
    def __init__(self, gateway_url: _Optional[str] = ..., app_ids: _Optional[_Mapping[str, str]] = ..., unifiedpush: _Optional[bool] = ...) -> None: ...

class WhoAmIRequest(_message.Message):
    __slots__ = ()
    def __init__(self) -> None: ...

class WhoAmIResponse(_message.Message):
    __slots__ = ("principal", "actor_chain", "org", "environments", "device", "authenticated_at")
    PRINCIPAL_FIELD_NUMBER: _ClassVar[int]
    ACTOR_CHAIN_FIELD_NUMBER: _ClassVar[int]
    ORG_FIELD_NUMBER: _ClassVar[int]
    ENVIRONMENTS_FIELD_NUMBER: _ClassVar[int]
    DEVICE_FIELD_NUMBER: _ClassVar[int]
    AUTHENTICATED_AT_FIELD_NUMBER: _ClassVar[int]
    principal: Principal
    actor_chain: _containers.RepeatedCompositeFieldContainer[Principal]
    org: Org
    environments: _containers.RepeatedCompositeFieldContainer[Environment]
    device: DeviceRef
    authenticated_at: _timestamp_pb2.Timestamp
    def __init__(self, principal: _Optional[_Union[Principal, _Mapping]] = ..., actor_chain: _Optional[_Iterable[_Union[Principal, _Mapping]]] = ..., org: _Optional[_Union[Org, _Mapping]] = ..., environments: _Optional[_Iterable[_Union[Environment, _Mapping]]] = ..., device: _Optional[_Union[DeviceRef, _Mapping]] = ..., authenticated_at: _Optional[_Union[datetime.datetime, _timestamp_pb2.Timestamp, _Mapping]] = ...) -> None: ...

class Principal(_message.Message):
    __slots__ = ("id", "kind", "display_name", "email")
    ID_FIELD_NUMBER: _ClassVar[int]
    KIND_FIELD_NUMBER: _ClassVar[int]
    DISPLAY_NAME_FIELD_NUMBER: _ClassVar[int]
    EMAIL_FIELD_NUMBER: _ClassVar[int]
    id: str
    kind: PrincipalKind
    display_name: str
    email: str
    def __init__(self, id: _Optional[str] = ..., kind: _Optional[_Union[PrincipalKind, str]] = ..., display_name: _Optional[str] = ..., email: _Optional[str] = ...) -> None: ...

class Org(_message.Message):
    __slots__ = ("id", "name")
    ID_FIELD_NUMBER: _ClassVar[int]
    NAME_FIELD_NUMBER: _ClassVar[int]
    id: str
    name: str
    def __init__(self, id: _Optional[str] = ..., name: _Optional[str] = ...) -> None: ...

class Environment(_message.Message):
    __slots__ = ("id", "project", "name", "namespace", "protected")
    ID_FIELD_NUMBER: _ClassVar[int]
    PROJECT_FIELD_NUMBER: _ClassVar[int]
    NAME_FIELD_NUMBER: _ClassVar[int]
    NAMESPACE_FIELD_NUMBER: _ClassVar[int]
    PROTECTED_FIELD_NUMBER: _ClassVar[int]
    id: str
    project: str
    name: str
    namespace: str
    protected: bool
    def __init__(self, id: _Optional[str] = ..., project: _Optional[str] = ..., name: _Optional[str] = ..., namespace: _Optional[str] = ..., protected: _Optional[bool] = ...) -> None: ...

class DeviceRef(_message.Message):
    __slots__ = ("id", "name")
    ID_FIELD_NUMBER: _ClassVar[int]
    NAME_FIELD_NUMBER: _ClassVar[int]
    id: str
    name: str
    def __init__(self, id: _Optional[str] = ..., name: _Optional[str] = ...) -> None: ...
