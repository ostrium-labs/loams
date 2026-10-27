import datetime

from google.protobuf import timestamp_pb2 as _timestamp_pb2
from loams.admin.v1 import common_pb2 as _common_pb2
from loams.admin.v1 import org_pb2 as _org_pb2
from loams.options.v1 import options_pb2 as _options_pb2
from google.protobuf.internal import containers as _containers
from google.protobuf.internal import enum_type_wrapper as _enum_type_wrapper
from google.protobuf import descriptor as _descriptor
from google.protobuf import message as _message
from collections.abc import Iterable as _Iterable, Mapping as _Mapping
from typing import ClassVar as _ClassVar, Optional as _Optional, Union as _Union

DESCRIPTOR: _descriptor.FileDescriptor

class OidcProviderKind(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    OIDC_PROVIDER_KIND_UNSPECIFIED: _ClassVar[OidcProviderKind]
    OIDC_PROVIDER_KIND_KEYCLOAK: _ClassVar[OidcProviderKind]
    OIDC_PROVIDER_KIND_OKTA: _ClassVar[OidcProviderKind]
    OIDC_PROVIDER_KIND_ENTRA: _ClassVar[OidcProviderKind]
    OIDC_PROVIDER_KIND_GOOGLE: _ClassVar[OidcProviderKind]
    OIDC_PROVIDER_KIND_AUTHENTIK: _ClassVar[OidcProviderKind]
    OIDC_PROVIDER_KIND_DEX: _ClassVar[OidcProviderKind]
    OIDC_PROVIDER_KIND_GENERIC: _ClassVar[OidcProviderKind]
    OIDC_PROVIDER_KIND_ZITADEL: _ClassVar[OidcProviderKind]

class ConsentDecision(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    CONSENT_DECISION_UNSPECIFIED: _ClassVar[ConsentDecision]
    CONSENT_DECISION_ALLOW: _ClassVar[ConsentDecision]
    CONSENT_DECISION_DENY: _ClassVar[ConsentDecision]
OIDC_PROVIDER_KIND_UNSPECIFIED: OidcProviderKind
OIDC_PROVIDER_KIND_KEYCLOAK: OidcProviderKind
OIDC_PROVIDER_KIND_OKTA: OidcProviderKind
OIDC_PROVIDER_KIND_ENTRA: OidcProviderKind
OIDC_PROVIDER_KIND_GOOGLE: OidcProviderKind
OIDC_PROVIDER_KIND_AUTHENTIK: OidcProviderKind
OIDC_PROVIDER_KIND_DEX: OidcProviderKind
OIDC_PROVIDER_KIND_GENERIC: OidcProviderKind
OIDC_PROVIDER_KIND_ZITADEL: OidcProviderKind
CONSENT_DECISION_UNSPECIFIED: ConsentDecision
CONSENT_DECISION_ALLOW: ConsentDecision
CONSENT_DECISION_DENY: ConsentDecision

class OidcProvider(_message.Message):
    __slots__ = ("id", "name", "kind")
    ID_FIELD_NUMBER: _ClassVar[int]
    NAME_FIELD_NUMBER: _ClassVar[int]
    KIND_FIELD_NUMBER: _ClassVar[int]
    id: str
    name: str
    kind: OidcProviderKind
    def __init__(self, id: _Optional[str] = ..., name: _Optional[str] = ..., kind: _Optional[_Union[OidcProviderKind, str]] = ...) -> None: ...

class Session(_message.Message):
    __slots__ = ("user", "org", "role", "csrf_token", "expires_at")
    USER_FIELD_NUMBER: _ClassVar[int]
    ORG_FIELD_NUMBER: _ClassVar[int]
    ROLE_FIELD_NUMBER: _ClassVar[int]
    CSRF_TOKEN_FIELD_NUMBER: _ClassVar[int]
    EXPIRES_AT_FIELD_NUMBER: _ClassVar[int]
    user: _common_pb2.User
    org: _org_pb2.Org
    role: _common_pb2.OrgRole
    csrf_token: str
    expires_at: _timestamp_pb2.Timestamp
    def __init__(self, user: _Optional[_Union[_common_pb2.User, _Mapping]] = ..., org: _Optional[_Union[_org_pb2.Org, _Mapping]] = ..., role: _Optional[_Union[_common_pb2.OrgRole, str]] = ..., csrf_token: _Optional[str] = ..., expires_at: _Optional[_Union[datetime.datetime, _timestamp_pb2.Timestamp, _Mapping]] = ...) -> None: ...

class ListProvidersRequest(_message.Message):
    __slots__ = ()
    def __init__(self) -> None: ...

class ListProvidersResponse(_message.Message):
    __slots__ = ("password", "totp", "passkeys", "oidc")
    PASSWORD_FIELD_NUMBER: _ClassVar[int]
    TOTP_FIELD_NUMBER: _ClassVar[int]
    PASSKEYS_FIELD_NUMBER: _ClassVar[int]
    OIDC_FIELD_NUMBER: _ClassVar[int]
    password: bool
    totp: bool
    passkeys: bool
    oidc: _containers.RepeatedCompositeFieldContainer[OidcProvider]
    def __init__(self, password: _Optional[bool] = ..., totp: _Optional[bool] = ..., passkeys: _Optional[bool] = ..., oidc: _Optional[_Iterable[_Union[OidcProvider, _Mapping]]] = ...) -> None: ...

class CompleteSetupRequest(_message.Message):
    __slots__ = ("setup_token", "org_name", "name", "email", "password")
    SETUP_TOKEN_FIELD_NUMBER: _ClassVar[int]
    ORG_NAME_FIELD_NUMBER: _ClassVar[int]
    NAME_FIELD_NUMBER: _ClassVar[int]
    EMAIL_FIELD_NUMBER: _ClassVar[int]
    PASSWORD_FIELD_NUMBER: _ClassVar[int]
    setup_token: str
    org_name: str
    name: str
    email: str
    password: str
    def __init__(self, setup_token: _Optional[str] = ..., org_name: _Optional[str] = ..., name: _Optional[str] = ..., email: _Optional[str] = ..., password: _Optional[str] = ...) -> None: ...

class CompleteSetupResponse(_message.Message):
    __slots__ = ("session",)
    SESSION_FIELD_NUMBER: _ClassVar[int]
    session: Session
    def __init__(self, session: _Optional[_Union[Session, _Mapping]] = ...) -> None: ...

class GetSessionRequest(_message.Message):
    __slots__ = ()
    def __init__(self) -> None: ...

class GetSessionResponse(_message.Message):
    __slots__ = ("session",)
    SESSION_FIELD_NUMBER: _ClassVar[int]
    session: Session
    def __init__(self, session: _Optional[_Union[Session, _Mapping]] = ...) -> None: ...

class CreateSessionRequest(_message.Message):
    __slots__ = ("email", "password", "totp_code")
    EMAIL_FIELD_NUMBER: _ClassVar[int]
    PASSWORD_FIELD_NUMBER: _ClassVar[int]
    TOTP_CODE_FIELD_NUMBER: _ClassVar[int]
    email: str
    password: str
    totp_code: str
    def __init__(self, email: _Optional[str] = ..., password: _Optional[str] = ..., totp_code: _Optional[str] = ...) -> None: ...

class CreateSessionResponse(_message.Message):
    __slots__ = ("session",)
    SESSION_FIELD_NUMBER: _ClassVar[int]
    session: Session
    def __init__(self, session: _Optional[_Union[Session, _Mapping]] = ...) -> None: ...

class DeleteSessionRequest(_message.Message):
    __slots__ = ()
    def __init__(self) -> None: ...

class DeleteSessionResponse(_message.Message):
    __slots__ = ()
    def __init__(self) -> None: ...

class DecideConsentRequest(_message.Message):
    __slots__ = ("decision", "client_id", "redirect_uri", "scope", "audience", "state", "code_challenge", "code_challenge_method")
    DECISION_FIELD_NUMBER: _ClassVar[int]
    CLIENT_ID_FIELD_NUMBER: _ClassVar[int]
    REDIRECT_URI_FIELD_NUMBER: _ClassVar[int]
    SCOPE_FIELD_NUMBER: _ClassVar[int]
    AUDIENCE_FIELD_NUMBER: _ClassVar[int]
    STATE_FIELD_NUMBER: _ClassVar[int]
    CODE_CHALLENGE_FIELD_NUMBER: _ClassVar[int]
    CODE_CHALLENGE_METHOD_FIELD_NUMBER: _ClassVar[int]
    decision: ConsentDecision
    client_id: str
    redirect_uri: str
    scope: str
    audience: str
    state: str
    code_challenge: str
    code_challenge_method: str
    def __init__(self, decision: _Optional[_Union[ConsentDecision, str]] = ..., client_id: _Optional[str] = ..., redirect_uri: _Optional[str] = ..., scope: _Optional[str] = ..., audience: _Optional[str] = ..., state: _Optional[str] = ..., code_challenge: _Optional[str] = ..., code_challenge_method: _Optional[str] = ...) -> None: ...

class DecideConsentResponse(_message.Message):
    __slots__ = ("redirect_to",)
    REDIRECT_TO_FIELD_NUMBER: _ClassVar[int]
    redirect_to: str
    def __init__(self, redirect_to: _Optional[str] = ...) -> None: ...
