"""Feature detection and the version check (design §44 §4 and §7.4; runtime
contract R9).

The unavailable-service path has two halves, and an SDK needs both:

1. **Without calling.** `GetInstance.services[]` says which packages this binary
   carries and which it does not (D600). One call, no auth, cheap, so an
   application asks first and hides a feature it cannot use. `loams.system`
   wraps it: `available`, `served`, `unavailable`, `guard`.
2. **When the caller calls anyway.** A call to a package the variant does not
   carry answers `unimplemented` with `ErrorInfo.reason =
   feature_not_in_variant` and the variant in `metadata.variant` (D600). The
   runtime turns that into a `FeatureNotInVariantError`, so the branch is on
   `except FeatureNotInVariantError` or on `error.reason ==
   "feature_not_in_variant"` — never on a message, and never on the package
   name, which is a proto detail.

Half 1 is the one to use. Half 2 is the safety net for a caller who skipped it,
or whose instance changed variant underneath a long-lived client.
"""

from __future__ import annotations

import threading
from dataclasses import dataclass
from typing import Awaitable, Callable

from connectrpc.code import Code

from loams._gen.facade import (
    FEATURE_NOT_IN_VARIANT,
    MODULES,
    PROTO_PACKAGES,
    PROTO_REV,
)
from loams.instance.v1.instance_pb2 import GetInstanceRequest, GetInstanceResponse, ServiceStatus
from loams.runtime.errors import FeatureNotInVariantError, LoamsError
from loams.runtime.options import CallOptions

__all__ = ["AsyncSystemApi", "Catalogue", "SystemApi", "VersionReport"]


@dataclass(frozen=True, slots=True)
class Catalogue:
    """`GetInstance.services[]`, sorted by what an SDK can use."""

    #: Every package the instance knows, served or not.
    services: tuple[ServiceStatus, ...]
    #: The packages this binary serves.
    served: tuple[str, ...]
    #: The packages this binary knows but does not serve, which is what makes
    #: `unavailable` a plan rather than a surprise.
    unavailable: tuple[str, ...]


@dataclass(frozen=True, slots=True)
class VersionReport:
    """What `loams.system.version()` reports (runtime contract R9)."""

    #: The proto revision this SDK was generated from.
    proto_rev: str
    #: The server's own semver.
    server_version: str
    #: The proto packages the server says it serves.
    api_versions: tuple[str, ...]
    #: The SDK's packages the server does not serve.
    missing: tuple[str, ...]
    #: True when every package the SDK speaks is served.
    compatible: bool


def to_catalogue(services: tuple[ServiceStatus, ...]) -> Catalogue:
    """Splits `GetInstance.services[]` into served and not."""
    ordered = tuple(sorted(services, key=lambda service: service.package))
    return Catalogue(
        services=ordered,
        served=tuple(s.package for s in ordered if s.available),
        unavailable=tuple(s.package for s in ordered if not s.available),
    )


def spoken_packages() -> tuple[str, ...]:
    """The proto packages this SDK speaks, which is what `GetInstance` lists."""
    return tuple(name for name in PROTO_PACKAGES if name.startswith("loams."))


def package_of(module_or_package: str) -> str:
    """The proto package behind a module name or a package name.

    Takes either, so a caller holding `loams.live` can pass `"live"` and a caller
    reading a `ServiceStatus` can pass `"loams.live.v1"`.
    """
    if module_or_package.startswith("loams."):
        return module_or_package
    for module in MODULES:
        if module.name == module_or_package:
            return module.package
    raise LoamsError(
        f"no generated module named {module_or_package}", code=Code.INTERNAL
    )


def _call_options(config: CallOptions | None) -> CallOptions | None:
    return config


class SystemApi:
    """The catalogue, the version check and the guard."""

    def __init__(
        self,
        get_instance: Callable[[GetInstanceRequest, CallOptions | None], GetInstanceResponse],
        config: CallOptions | None = None,
    ) -> None:
        """`get_instance` is `loams.instance.get_instance`, the one RPC used here.

        Injected rather than reached for, so the system API is testable without a
        client.
        """
        self._get_instance = get_instance
        self._config = config
        self._catalogue: Catalogue | None = None
        self._in_flight: GetInstanceResponse | None = None
        self._lock = threading.Lock()

    def catalogue(self) -> Catalogue:
        """The service catalogue, cached.

        Cached because it is asked on a cold start, when a UI is deciding which
        features to show, and again on every feature check: the instance's
        services do not change while a process runs. `invalidate()` drops it.
        """
        if self._catalogue is not None:
            return self._catalogue
        with self._lock:
            if self._in_flight is None:
                self._in_flight = self._get_instance(GetInstanceRequest(), self._config)
            response = self._in_flight
        try:
            catalogue = to_catalogue(tuple(response.services))
        finally:
            with self._lock:
                self._in_flight = None
        self._catalogue = catalogue
        return catalogue

    def invalidate(self) -> None:
        """Drops the cached catalogue, so the next check calls again."""
        with self._lock:
            self._catalogue = None

    def available(self, module_or_package: str) -> bool:
        """Whether this binary serves a module or a proto package."""
        name = package_of(module_or_package)
        return name in self.catalogue().served

    def served(self) -> tuple[str, ...]:
        """The packages this binary serves."""
        return self.catalogue().served

    def unavailable(self) -> tuple[str, ...]:
        """The packages it knows but does not serve."""
        return self.catalogue().unavailable

    def guard(self, module_or_package: str) -> None:
        """Raises `FeatureNotInVariantError` unless this binary serves the module.

        This is the guard to put in front of a feature: the error is the same
        class the server's own refusal maps to, so one `except` covers both "the
        instance does not have this" and "the guard said no", and it costs no RPC
        once the catalogue is cached.
        """
        name = package_of(module_or_package)
        if self.available(name):
            return
        raise FeatureNotInVariantError(
            f"{name} is not in this instance's build variant",
            code=Code.UNIMPLEMENTED,
            reason="feature_not_in_variant",
            metadata={"package": name},
            rpc="loams.instance.v1.InstanceService/GetInstance",
            variant=None,
        )

    def version(self) -> VersionReport:
        """The proto revision check (R9).

        A missing package is a **warning, not an exception**: the SDK still works
        for the modules that are there, and the caller decides what a missing one
        means.
        """
        response = self._get_instance(GetInstanceRequest(), self._config)
        api_versions = tuple(response.api_versions)
        spoken = spoken_packages()
        missing = tuple(name for name in spoken if name not in api_versions)
        return VersionReport(
            proto_rev=PROTO_REV,
            server_version=response.server_version,
            api_versions=api_versions,
            missing=missing,
            compatible=len(missing) == 0,
        )


class AsyncSystemApi:
    """`SystemApi` for `AsyncLoams`, over the same decisions."""

    def __init__(
        self,
        get_instance: Callable[
            [GetInstanceRequest, CallOptions | None], Awaitable[GetInstanceResponse]
        ],
        config: CallOptions | None = None,
    ) -> None:
        self._get_instance = get_instance
        self._config = config
        self._catalogue: Catalogue | None = None
        self._in_flight: Any = None

    async def catalogue(self) -> Catalogue:
        """The service catalogue, cached, with one in-flight fetch shared.

        Concurrent readers share the fetch, so a cold start with twenty
        availability checks makes one call, not twenty.
        """
        if self._catalogue is not None:
            return self._catalogue
        if self._in_flight is None:
            self._in_flight = self._get_instance(GetInstanceRequest(), self._config)
        response = await self._in_flight
        catalogue = to_catalogue(tuple(response.services))
        self._catalogue = catalogue
        self._in_flight = None
        return catalogue

    def invalidate(self) -> None:
        """Drops the cached catalogue."""
        self._catalogue = None

    async def available(self, module_or_package: str) -> bool:
        """Whether this binary serves a module or a proto package."""
        return package_of(module_or_package) in (await self.catalogue()).served

    async def served(self) -> tuple[str, ...]:
        """The packages this binary serves."""
        return (await self.catalogue()).served

    async def unavailable(self) -> tuple[str, ...]:
        """The packages it knows but does not serve."""
        return (await self.catalogue()).unavailable

    async def guard(self, module_or_package: str) -> None:
        """Raises `FeatureNotInVariantError` unless this binary serves the module."""
        name = package_of(module_or_package)
        if await self.available(name):
            return
        raise FeatureNotInVariantError(
            f"{name} is not in this instance's build variant",
            code=Code.UNIMPLEMENTED,
            reason="feature_not_in_variant",
            metadata={"package": name},
            rpc="loams.instance.v1.InstanceService/GetInstance",
            variant=None,
        )

    async def version(self) -> VersionReport:
        """The proto revision check (R9)."""
        response = await self._get_instance(GetInstanceRequest(), self._config)
        api_versions = tuple(response.api_versions)
        spoken = spoken_packages()
        missing = tuple(name for name in spoken if name not in api_versions)
        return VersionReport(
            proto_rev=PROTO_REV,
            server_version=response.server_version,
            api_versions=api_versions,
            missing=missing,
            compatible=len(missing) == 0,
        )
