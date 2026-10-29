"""IA4 Task 2 (plan IA1, design 76 §10): an agent's token from its machine key."""

from __future__ import annotations

import asyncio
import base64
import json
import threading
import urllib.parse

import pytest
from cryptography.exceptions import InvalidSignature
from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import padding, rsa

from loams.runtime.machine_key import (
    ASSERTION_TTL_S,
    JWT_BEARER_GRANT,
    REFRESH_EARLY_S,
    AsyncMachineKey,
    MachineKey,
)


def _key(fmt: serialization.PrivateFormat) -> tuple[str, rsa.RSAPublicKey]:
    key = rsa.generate_private_key(public_exponent=65537, key_size=2048)
    pem = key.private_bytes(serialization.Encoding.PEM, fmt, serialization.NoEncryption())
    return pem.decode(), key.public_key()


def _file(pem: str, kind: str = "serviceaccount") -> str:
    return json.dumps({"type": kind, "keyId": "k-1", "key": pem, "userId": "agent-7"})


class _Endpoint:
    def __init__(self, expires_in: int = 900, fail: bool = False) -> None:
        self.calls: list[tuple[str, dict[str, str]]] = []
        self.expires_in = expires_in
        self.fail = fail
        self._lock = threading.Lock()

    def __call__(self, url: str, form: str) -> str:
        with self._lock:
            self.calls.append((url, dict(urllib.parse.parse_qsl(form))))
            n = len(self.calls)
        if self.fail:
            raise RuntimeError("the token endpoint refused the machine key (400): invalid_grant")
        return json.dumps({"access_token": f"at-{n}", "expires_in": self.expires_in})


def _part(part: str) -> dict[str, object]:
    padded = part + "=" * (-len(part) % 4)
    value = json.loads(base64.urlsafe_b64decode(padded))
    assert isinstance(value, dict)
    return value


@pytest.mark.parametrize(
    "fmt", [serialization.PrivateFormat.TraditionalOpenSSL, serialization.PrivateFormat.PKCS8]
)
def test_the_assertion_is_rfc_7523_signed_with_the_key(fmt: serialization.PrivateFormat) -> None:
    pem, public = _key(fmt)
    endpoint = _Endpoint()
    source = MachineKey(_file(pem), "https://auth.example.com/", "3141", post=endpoint)
    assert source.user_id == "agent-7"
    assert "PRIVATE" not in repr(source)
    assert source.token() == "at-1"
    url, form = endpoint.calls[0]
    assert url == "https://auth.example.com/oauth/v2/token"
    assert form["grant_type"] == JWT_BEARER_GRANT
    assert form["scope"] == "openid urn:zitadel:iam:org:project:id:3141:aud"
    header, claims, signature = form["assertion"].split(".")
    sig = base64.urlsafe_b64decode(signature + "=" * (-len(signature) % 4))
    public.verify(sig, f"{header}.{claims}".encode(), padding.PKCS1v15(), hashes.SHA256())
    with pytest.raises(InvalidSignature):
        public.verify(sig, b"tampered", padding.PKCS1v15(), hashes.SHA256())
    assert _part(header) == {"alg": "RS256", "typ": "JWT", "kid": "k-1"}
    c = _part(claims)
    assert (c["iss"], c["sub"], c["aud"]) == ("agent-7", "agent-7", "https://auth.example.com")
    assert isinstance(c["exp"], int) and isinstance(c["iat"], int)
    assert c["exp"] - c["iat"] == ASSERTION_TTL_S


def test_caches_until_a_minute_before_expiry_and_refreshes() -> None:
    pem, _ = _key(serialization.PrivateFormat.TraditionalOpenSSL)
    endpoint = _Endpoint(expires_in=900)
    clock = [1_000_000.0]
    source = MachineKey(_file(pem), "https://i.example", "p", post=endpoint, now=lambda: clock[0])
    assert source.token() == "at-1"
    assert source.token() == "at-1"
    clock[0] += 900 - REFRESH_EARLY_S + 1
    assert source.token() == "at-2"
    source.refresh()
    assert source.token() == "at-3"
    clock[0] += 3600
    threads = [threading.Thread(target=source.token) for _ in range(8)]
    for t in threads:
        t.start()
    for t in threads:
        t.join()
    assert len(endpoint.calls) == 4, "concurrent callers share one trade"


def test_a_refused_trade_sends_no_credential_and_refresh_reports() -> None:
    pem, _ = _key(serialization.PrivateFormat.PKCS8)
    source = MachineKey(_file(pem), "https://i.example", "p", post=_Endpoint(fail=True))
    assert source.token() is None
    with pytest.raises(RuntimeError, match="invalid_grant"):
        source.refresh()


def test_refuses_what_is_not_a_machine_key() -> None:
    pem, _ = _key(serialization.PrivateFormat.PKCS8)
    for bad in [
        lambda: MachineKey(_file(pem, "application"), "https://i", "p"),
        lambda: MachineKey("{", "https://i", "p"),
        lambda: MachineKey(_file("nope"), "https://i", "p"),
        lambda: MachineKey(_file(pem), "https://i", ""),
        lambda: MachineKey(_file(pem), "auth.example", "p"),
    ]:
        with pytest.raises(ValueError):
            bad()


def test_the_async_source_trades_once_for_concurrent_callers() -> None:
    pem, _ = _key(serialization.PrivateFormat.TraditionalOpenSSL)
    endpoint = _Endpoint()

    async def run() -> list[str | None]:
        source = AsyncMachineKey(_file(pem), "https://i.example", "p", post=endpoint)
        tokens = await asyncio.gather(*(source.token() for _ in range(8)))
        await source.refresh()
        return [*tokens, await source.token()]

    tokens = asyncio.run(run())
    assert tokens[:8] == ["at-1"] * 8
    assert tokens[8] == "at-2"
    assert len(endpoint.calls) == 2
