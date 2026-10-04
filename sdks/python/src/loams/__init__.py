"""The Loams SDK for Python.

``from loams import Loams`` is the SDK's front door, so this module re-exports
the clients and the token-source factories. Without it the package installs but
the documented imports fail: Python treats a directory with no ``__init__.py`` as
a namespace package, and a namespace package has none of these attributes.

The token sources are re-exported in matched sync and async pairs because every
one of them has an async twin, and picking the wrong one is a bug that only shows
up as a blocking call on the event loop -- ``aio.py`` had been importing the
sync ``api_key`` under the name ``async_api_key_source``.
"""

from loams.aio import AsyncLoams
from loams.loams import Loams
from loams.runtime.errors import (
    AbortedError,
    AlreadyExistsError,
    DeadlineExceededError,
    FailedPreconditionError,
    FeatureNotInVariantError,
    InternalError,
    InvalidArgumentError,
    LoamsError,
    NotFoundError,
    PermissionDeniedError,
    ResourceExhaustedError,
    TokenExpiredError,
    UnauthenticatedError,
    UnavailableError,
    UnimplementedError,
    is_loams_error,
)
from loams.runtime.token_source import (
    OidcExchangeOptions,
    api_key,
    async_api_key,
    async_env_token,
    async_oidc_exchange,
    async_refreshing,
    async_static_token,
    env_token,
    oidc_exchange,
    refreshing,
    static_token,
)

__all__ = [
    "AbortedError",
    "AlreadyExistsError",
    "DeadlineExceededError",
    "FailedPreconditionError",
    "FeatureNotInVariantError",
    "InternalError",
    "InvalidArgumentError",
    "LoamsError",
    "NotFoundError",
    "PermissionDeniedError",
    "ResourceExhaustedError",
    "TokenExpiredError",
    "UnauthenticatedError",
    "UnavailableError",
    "UnimplementedError",
    "is_loams_error",
    "AsyncLoams",
    "Loams",
    "OidcExchangeOptions",
    "api_key",
    "async_api_key",
    "async_env_token",
    "async_oidc_exchange",
    "async_refreshing",
    "async_static_token",
    "env_token",
    "oidc_exchange",
    "refreshing",
    "static_token",
]