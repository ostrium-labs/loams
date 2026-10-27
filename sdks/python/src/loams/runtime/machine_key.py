"""An agent's token from its identity provider's private-key JWT key.

Design 76 §10, D1607; plan IA1, IA4 Task 2. Needs the `agent` extra
(`pip install 'loams[agent]'`), which adds `cryptography` for the RS256 signature.

An agent is a machine user at the identity provider (Zitadel). Its key file is the JSON the
provider hands out once (`{"type": "serviceaccount", "keyId", "key", "userId"}`), named by
`LOAMS_AGENT_KEY_FILE`. The source signs an RFC 7523 assertion (RS256, `kid` = `keyId`,
`iss` = `sub` = `userId`, `aud` = the issuer, valid for 5 minutes), trades it at
`<issuer>/oauth/v2/token` for an access token whose audience is the Loams project
(`urn:zitadel:iam:org:project:id:<project>:aud`), caches it, and trades again a minute before
it expires or when the server answers `token_expired`. Concurrent callers share one trade. The
private key never leaves the process. A sandboxed agent holds no key: agentd vends it a
15-minute token file instead (D1618).
"""

from __future__ import annotations

import asyncio
import base64
import json
import os
import threading
import time
import urllib.error
import urllib.parse
import urllib.request
from collections.abc import Callable, Mapping
from typing import Any, Final

__all__ = [
    "ASSERTION_TTL_S",
    "JWT_BEARER_GRANT",
    "REFRESH_EARLY_S",
    "AsyncMachineKey",
    "MachineKey",
    "async_machine_key",
    "async_machine_key_from_env",
    "machine_key",
    "machine_key_from_env",
]

ASSERTION_TTL_S: Final[int] = 300
REFRESH_EARLY_S: Final[int] = 60
JWT_BEARER_GRANT: Final[str] = "urn:ietf:params:oauth:grant-type:jwt-bearer"
ENV_KEY_FILE: Final[str] = "LOAMS_AGENT_KEY_FILE"
ENV_ISSUER: Final[str] = "LOAMS_AGENT_ISSUER"
ENV_PROJECT: Final[str] = "LOAMS_AGENT_PROJECT"

#: Posts a form body to a URL and answers the response body; raises on a refusal.
Poster = Callable[[str, str], str]


def _b64url(data: bytes) -> str:
    return base64.urlsafe_b64encode(data).rstrip(b"=").decode("ascii")


def _post_form(url: str, form: str) -> str:
    request = urllib.request.Request(
        url,
        data=form.encode("ascii"),
        headers={"content-type": "application/x-www-form-urlencoded"},
        method="POST",
    )
    try:
        with urllib.request.urlopen(request, timeout=30) as response:  # noqa: S310 (https issuer)
            return str(response.read().decode("utf-8"))
    except urllib.error.HTTPError as error:
        body = error.read().decode("utf-8", "replace")[:512]
        raise RuntimeError(
            f"the token endpoint refused the machine key ({error.code}): {body}"
        ) from None


class _Signer:
    """The parsed key file and the assertion it signs."""

    def __init__(self, key_file: bytes | str | Mapping[str, Any], issuer: str, project: str) -> None:
        try:
            from cryptography.hazmat.primitives import serialization
            from cryptography.hazmat.primitives.asymmetric import rsa
        except ImportError:  # pragma: no cover - the extra is installed in CI
            raise ImportError(
                "machine_key needs the `agent` extra: pip install 'loams[agent]'"
            ) from None
        if isinstance(key_file, Mapping):
            file = dict(key_file)
        else:
            try:
                file = json.loads(key_file)
            except ValueError:
                raise ValueError("machine_key: not a machine key file") from None
        if not isinstance(file, dict):
            raise ValueError("machine_key: not a machine key file")
        kind = file.get("type") or ""
        if kind not in ("", "serviceaccount"):
            raise ValueError(f"machine_key: a {kind!r} key is not a machine user's key")
        key_id, pem, user_id = file.get("keyId"), file.get("key"), file.get("userId")
        if not (isinstance(key_id, str) and key_id and isinstance(user_id, str) and user_id):
            raise ValueError("machine_key: the key file has no keyId or userId")
        if not isinstance(pem, str):
            raise ValueError("machine_key: the key file has no key")
        try:
            private = serialization.load_pem_private_key(pem.encode("ascii"), password=None)
        except (ValueError, TypeError):
            raise ValueError("machine_key: the machine key does not load") from None
        if not isinstance(private, rsa.RSAPrivateKey):
            raise ValueError("machine_key: the machine key is not an RSA key")
        issuer = issuer.rstrip("/")
        parsed = urllib.parse.urlparse(issuer)
        if parsed.scheme not in ("https", "http") or not parsed.netloc:
            raise ValueError(f"machine_key: the issuer {issuer!r} is not a URL")
        if project == "":
            raise ValueError("machine_key: the Loams project id (the audience) is empty")
        self.key_id: str = key_id
        self.user_id: str = user_id
        self.issuer = issuer
        self.scope = f"openid urn:zitadel:iam:org:project:id:{project}:aud"
        self._private = private

    def __repr__(self) -> str:
        return f"MachineKey(key_id={self.key_id!r}, user_id={self.user_id!r})"

    def assertion(self, now: float) -> str:
        from cryptography.hazmat.primitives import hashes
        from cryptography.hazmat.primitives.asymmetric import padding

        iat = int(now)
        header = _b64url(
            json.dumps({"alg": "RS256", "typ": "JWT", "kid": self.key_id}).encode()
        )
        claims = _b64url(
            json.dumps(
                {
                    "iss": self.user_id,
                    "sub": self.user_id,
                    "aud": self.issuer,
                    "iat": iat,
                    "exp": iat + ASSERTION_TTL_S,
                }
            ).encode()
        )
        signing = f"{header}.{claims}"
        signature = self._private.sign(signing.encode(), padding.PKCS1v15(), hashes.SHA256())
        return f"{signing}.{_b64url(signature)}"

    def form(self, assertion: str) -> str:
        return urllib.parse.urlencode(
            {"grant_type": JWT_BEARER_GRANT, "scope": self.scope, "assertion": assertion}
        )

    def token_url(self) -> str:
        return f"{self.issuer}/oauth/v2/token"


def _read_answer(body: str, started: float) -> tuple[str, float]:
    try:
        answer = json.loads(body)
    except ValueError:
        raise RuntimeError("the token endpoint answered no access_token") from None
    token = answer.get("access_token") if isinstance(answer, dict) else None
    if not isinstance(token, str) or token == "":
        raise RuntimeError("the token endpoint answered no access_token")
    life = answer.get("expires_in")
    life = float(life) if isinstance(life, (int, float)) else 900.0
    early = min(float(REFRESH_EARLY_S), life / 2)
    return token, started + life - early


class MachineKey:
    """The sync source (module docstring)."""

    def __init__(
        self,
        key_file: bytes | str | Mapping[str, Any],
        issuer: str,
        project: str,
        *,
        post: Poster | None = None,
        now: Callable[[], float] = time.time,
    ) -> None:
        self._signer = _Signer(key_file, issuer, project)
        self._post = post or _post_form
        self._now = now
        self._lock = threading.Lock()
        self._token: str | None = None
        self._refresh_at = 0.0

    def __repr__(self) -> str:
        return repr(self._signer)

    @property
    def user_id(self) -> str:
        return self._signer.user_id

    def _trade(self) -> str:
        started = self._now()
        body = self._post(self._signer.token_url(), self._signer.form(self._signer.assertion(started)))
        self._token, self._refresh_at = _read_answer(body, started)
        return self._token

    def token(self) -> str | None:
        with self._lock:
            if self._token is not None and self._now() < self._refresh_at:
                return self._token
            try:
                return self._trade()
            except Exception:  # noqa: BLE001 - a failed trade sends no credential
                self._token = None
                return None

    def refresh(self) -> None:
        with self._lock:
            try:
                self._trade()
            except Exception:
                self._token = None
                raise


class AsyncMachineKey:
    """The async source: the same trade, with the sync poster run in a thread."""

    def __init__(
        self,
        key_file: bytes | str | Mapping[str, Any],
        issuer: str,
        project: str,
        *,
        post: Poster | None = None,
        now: Callable[[], float] = time.time,
    ) -> None:
        self._signer = _Signer(key_file, issuer, project)
        self._post = post or _post_form
        self._now = now
        self._lock = asyncio.Lock()
        self._token: str | None = None
        self._refresh_at = 0.0

    def __repr__(self) -> str:
        return repr(self._signer)

    @property
    def user_id(self) -> str:
        return self._signer.user_id

    async def _trade(self) -> str:
        started = self._now()
        form = self._signer.form(self._signer.assertion(started))
        body = await asyncio.to_thread(self._post, self._signer.token_url(), form)
        self._token, self._refresh_at = _read_answer(body, started)
        return self._token

    async def token(self) -> str | None:
        async with self._lock:
            if self._token is not None and self._now() < self._refresh_at:
                return self._token
            try:
                return await self._trade()
            except Exception:  # noqa: BLE001 - a failed trade sends no credential
                self._token = None
                return None

    async def refresh(self) -> None:
        async with self._lock:
            try:
                await self._trade()
            except Exception:
                self._token = None
                raise


def machine_key(
    key_file: bytes | str | Mapping[str, Any],
    issuer: str,
    project: str,
    *,
    post: Poster | None = None,
) -> MachineKey:
    """A sync source from a key file's bytes, JSON text or parsed mapping."""
    return MachineKey(key_file, issuer, project, post=post)


def async_machine_key(
    key_file: bytes | str | Mapping[str, Any],
    issuer: str,
    project: str,
    *,
    post: Poster | None = None,
) -> AsyncMachineKey:
    """`machine_key` for `AsyncLoams`."""
    return AsyncMachineKey(key_file, issuer, project, post=post)


def _from_env(environment: Mapping[str, str] | None) -> tuple[bytes, str, str]:
    env = environment if environment is not None else os.environ

    def get(name: str) -> str:
        value = (env.get(name) or "").strip()
        if value == "":
            raise ValueError(f"{name} is not set")
        return value

    path, issuer, project = get(ENV_KEY_FILE), get(ENV_ISSUER), get(ENV_PROJECT)
    with open(path, "rb") as handle:
        return handle.read(), issuer, project


def machine_key_from_env(environment: Mapping[str, str] | None = None) -> MachineKey:
    """From `LOAMS_AGENT_KEY_FILE`, `LOAMS_AGENT_ISSUER` and `LOAMS_AGENT_PROJECT`."""
    return MachineKey(*_from_env(environment))


def async_machine_key_from_env(environment: Mapping[str, str] | None = None) -> AsyncMachineKey:
    """`machine_key_from_env` for `AsyncLoams`."""
    return AsyncMachineKey(*_from_env(environment))
