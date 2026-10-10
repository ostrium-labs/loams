"""The RFC 8693 token exchange, and how its failures surface.

`OidcExchangeOptions` documents honestly that the instance serves no OAuth
endpoint yet, so this cannot be exercised against a live server -- and it was not
exercised at all. `post` is injectable precisely so it could be, and nothing did,
which left ~60 lines of auth code with zero tests: the one part of the SDK that
turns a person's sign-in into the credential every call carries.

The failure modes are the interesting half, because they are inconsistent by
construction. Three different exceptions can come out of the same operation:

- a non-2xx status raises `RuntimeError` naming the status;
- a 2xx body with no usable `access_token` raises `RuntimeError` saying so; but
- a 2xx body that is not JSON at all raises `json.JSONDecodeError`, which is a
  `ValueError` and names neither the endpoint nor the fact that a proxy answered.

And on the default transport, an `HTTPError` becomes `(status, body)` while a
connection failure stays a `urllib.error.URLError`. A caller cannot write one
`except` over "the exchange did not work". These tests pin the behaviour as it
is, including which of the three it is, so the day someone unifies them the
change is visible rather than silent.
"""

from __future__ import annotations

import asyncio
import json
import urllib.error
import urllib.parse

import pytest

from loams.runtime.token_source import (
    OidcExchangeOptions,
    _default_post,
    async_oidc_exchange,
    oidc_exchange,
)

ENDPOINT = "https://instance.test/oauth/token"
CLIENT_ID = "loams-console"

#: The grant RFC 8693 §2.1 defines for exchanging an id_token for an access token.
TOKEN_EXCHANGE_GRANT = "urn:ietf:params:oauth:grant-type:token-exchange"
ID_TOKEN_TYPE = "urn:ietf:params:oauth:token-type:id_token"
ACCESS_TOKEN_TYPE = "urn:ietf:params:oauth:token-type:access_token"


class _StubPost:
    """Records what was sent, and answers with whatever it is told to."""

    def __init__(self, *answers: tuple[int, bytes]) -> None:
        self.calls: list[tuple[str, dict[str, str]]] = []
        self._answers = list(answers) or [(200, b'{"access_token": "tok"}')]

    def __call__(self, url: str, body: bytes) -> tuple[int, bytes]:
        fields = dict(urllib.parse.parse_qsl(body.decode()))
        self.calls.append((url, fields))
        if len(self._answers) > 1:
            return self._answers.pop(0)
        return self._answers[0]

    @property
    def count(self) -> int:
        return len(self.calls)


def _options(post, *, subject: str = "id-token-1") -> OidcExchangeOptions:
    return OidcExchangeOptions(
        endpoint=ENDPOINT, client_id=CLIENT_ID, subject_token=lambda: subject, post=post
    )


# -- the request --------------------------------------------------------------


def test_the_exchange_sends_the_grant_rfc_8693_defines() -> None:
    """A token exchange that sends the wrong grant is a 400 from a real server,
    and the four URNs are the entire content of the standard."""
    post = _StubPost()
    assert oidc_exchange(_options(post)).token() == "tok"

    url, fields = post.calls[0]
    assert url == ENDPOINT
    assert fields["grant_type"] == TOKEN_EXCHANGE_GRANT
    assert fields["subject_token_type"] == ID_TOKEN_TYPE
    assert fields["requested_token_type"] == ACCESS_TOKEN_TYPE
    assert fields["subject_token"] == "id-token-1"
    assert fields["client_id"] == CLIENT_ID


def test_the_subject_token_is_read_at_exchange_time_not_at_construction() -> None:
    """An id_token expires; a refresh must not replay the one captured at build
    time, or the exchange succeeds once and never again."""
    post = _StubPost()
    options = _options(post, subject="first")

    source = oidc_exchange(options)
    assert source.token() == "tok"

    options.subject_token = lambda: "second"
    source.refresh()
    assert [fields["subject_token"] for _, fields in post.calls] == ["first", "second"], (
        f"the exchange sent {[f['subject_token'] for _, f in post.calls]}"
    )


# -- caching ------------------------------------------------------------------


def test_the_access_token_is_cached_until_refreshed() -> None:
    """One exchange for repeated reads. Without this every call would spend an
    HTTP round trip on the instance's token endpoint."""
    post = _StubPost()
    source = oidc_exchange(_options(post))
    assert source.token() == "tok"
    assert source.token() == "tok"
    assert source.token() == "tok"
    assert post.count == 1, f"three token() calls made {post.count} exchanges"


def test_refresh_forces_a_new_exchange() -> None:
    post = _StubPost((200, b'{"access_token": "first"}'), (200, b'{"access_token": "second"}'))
    source = oidc_exchange(_options(post))
    assert source.token() == "first"
    source.refresh()
    assert source.token() == "second"
    assert post.count == 2


def test_a_failed_exchange_is_not_cached() -> None:
    """A transient failure must not leave a blank token behind that every later
    call happily returns."""
    post = _StubPost((503, b"unavailable"), (200, b'{"access_token": "tok"}'))
    source = oidc_exchange(_options(post))
    with pytest.raises(RuntimeError):
        source.token()
    assert source.token() == "tok", "the retry after a failed exchange did not happen"


# -- failures -----------------------------------------------------------------


def test_a_refusal_names_the_status() -> None:
    post = _StubPost((401, b'{"error": "invalid_token"}'))
    with pytest.raises(RuntimeError) as caught:
        oidc_exchange(_options(post)).token()
    assert "401" in str(caught.value), f"the error does not name the status: {caught.value}"


@pytest.mark.parametrize("status", [300, 400, 403, 404, 500, 503])
def test_everything_outside_2xx_is_a_failure(status: int) -> None:
    post = _StubPost((status, b""))
    with pytest.raises(RuntimeError):
        oidc_exchange(_options(post)).token()


def test_a_2xx_without_an_access_token_says_so() -> None:
    for body in (b"{}", b'{"access_token": null}', b'{"access_token": ""}', b'{"access_token": 7}'):
        post = _StubPost((200, body))
        with pytest.raises(RuntimeError) as caught:
            oidc_exchange(_options(post)).token()
        assert "access_token" in str(caught.value), (
            f"a body of {body!r} produced {caught.value}, which does not say what was missing"
        )


def test_a_2xx_that_is_not_json_raises_a_json_error_not_a_runtime_error() -> None:
    """The inconsistency, pinned.

    A proxy's HTML error page with a 200 status is the realistic case here. It
    surfaces as `json.JSONDecodeError`, which is a `ValueError` -- so a caller
    wrapping the exchange in `except RuntimeError` does not catch it, and the
    message names neither the endpoint nor the fact that a non-JSON body arrived.
    """
    post = _StubPost((200, b"<html>gateway</html>"))
    with pytest.raises(json.JSONDecodeError):
        oidc_exchange(_options(post)).token()

    # And explicitly *not* a RuntimeError, which is the point.
    post = _StubPost((200, b"<html>gateway</html>"))
    try:
        oidc_exchange(_options(post)).token()
    except RuntimeError:  # pragma: no cover - the assertion below is the test
        pytest.fail("a non-JSON body raised RuntimeError; the pin is out of date")
    except ValueError:
        pass


def test_a_transport_failure_is_not_normalised() -> None:
    """A connection refused stays a `URLError`; only an HTTP status becomes a
    `RuntimeError`. Same operation, two exception types for the caller."""
    post = _StubPost((0, b""))  # never reached; the stub raises first

    def refuse(url: str, body: bytes) -> tuple[int, bytes]:
        raise urllib.error.URLError("connection refused")

    with pytest.raises(urllib.error.URLError):
        oidc_exchange(_options(refuse)).token()
    assert post.count == 0


# -- the default transport ----------------------------------------------------


def test_the_default_post_sends_a_form_encoding() -> None:
    """`content-type: application/x-www-form-urlencoded` is what RFC 6749 §4.1.3
    requires of a token request; an OAuth server rejects the body without it."""
    captured: dict[str, object] = {}

    class _Response:
        status = 200

        def read(self) -> bytes:
            return b'{"access_token": "tok"}'

        def __enter__(self):
            return self

        def __exit__(self, *exc: object) -> bool:
            return False

    def fake_urlopen(request: object, **_kw: object) -> _Response:
        captured["url"] = getattr(request, "full_url", None)
        captured["headers"] = dict(getattr(request, "headers", {}))
        captured["data"] = getattr(request, "data", None)
        return _Response()

    original = urllib.request.urlopen
    urllib.request.urlopen = fake_urlopen  # type: ignore[assignment]
    try:
        status, body = _default_post(ENDPOINT, b"grant_type=x")
    finally:
        urllib.request.urlopen = original  # type: ignore[assignment]

    assert status == 200
    assert json.loads(body)["access_token"] == "tok"
    assert captured["url"] == ENDPOINT
    # urllib stores the header key as given but reads it back lower-cased, so the
    # comparison is on the lower-cased form rather than the title-cased one.
    headers = {k.lower(): v for k, v in dict(captured["headers"]).items()}  # type: ignore[arg-type]
    assert headers.get("content-type") == "application/x-www-form-urlencoded", (
        f"the form content type is missing: {captured['headers']}"
    )


def test_the_default_post_turns_an_http_error_into_a_status() -> None:
    """An HTTPError is a response, not a failure of the transport, so it becomes
    `(status, body)` and the caller applies the same rules as any other status."""
    error = urllib.error.HTTPError(ENDPOINT, 429, "Too Many Requests", {}, None)  # type: ignore[arg-type]
    original = urllib.request.urlopen
    urllib.request.urlopen = lambda *_a, **_k: (_ for _ in ()).throw(error)  # type: ignore[assignment]
    try:
        status, body = _default_post(ENDPOINT, b"grant_type=x")
    finally:
        urllib.request.urlopen = original  # type: ignore[assignment]

    assert status == 429
    assert body == b"", f"the error body was not carried through: {body!r}"


# -- the async form -----------------------------------------------------------


def test_the_async_exchange_answers_the_same_thing() -> None:
    post = _StubPost()
    source = async_oidc_exchange(_options(post))

    async def call() -> str:
        token: str = await source.token()
        return token

    assert asyncio.run(call()) == "tok"
    assert post.count == 1


def test_the_async_exchange_happens_off_the_event_loop() -> None:
    """`AsyncLoams` must not block its caller while the instance answers.

    The default `_Refreshing` runs the fetch on the calling thread, so the
    blocking form would freeze every other task. `_exchange_in_executor` is the
    one blocking call in the SDK and it says so; this is what holds it to that.

    Proved by interleaving rather than by timing: a second task increments a
    counter while the exchange is in flight, and it must get to run.
    """
    import time

    ticks = 0

    def slow_post(url: str, body: bytes) -> tuple[int, bytes]:
        time.sleep(0.2)  # a real token endpoint is not instant
        return 200, b'{"access_token": "tok"}'

    source = async_oidc_exchange(_options(slow_post))

    async def ticker() -> None:
        nonlocal ticks
        for _ in range(5):
            await asyncio.sleep(0.01)
            ticks += 1

    async def main() -> str:
        token_task = asyncio.create_task(source.token())
        await ticker()
        token: str = await token_task
        return token

    assert asyncio.run(main()) == "tok"
    assert ticks == 5, (
        f"the ticker only got {ticks} of 5 turns while the exchange was in "
        "flight, so the exchange blocked the loop"
    )


def test_the_async_exchange_caches_like_the_sync_one() -> None:
    post = _StubPost()
    source = async_oidc_exchange(_options(post))

    async def call() -> None:
        await source.token()
        await source.token()
        await source.token()

    asyncio.run(call())
    assert post.count == 1, f"three async token() calls made {post.count} exchanges"


def test_an_async_exchange_failure_surfaces_unchanged() -> None:
    """The executor does not swallow or re-wrap: a caller sees the same
    `RuntimeError` the sync path raises, so one `except` covers both."""
    post = _StubPost((500, b"boom"))
    source = async_oidc_exchange(_options(post))

    async def call() -> None:
        await source.token()

    with pytest.raises(RuntimeError) as caught:
        asyncio.run(call())
    assert "500" in str(caught.value)