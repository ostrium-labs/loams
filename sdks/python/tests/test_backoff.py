"""The retry policy and the key it mints.

`loams.runtime.retry` decides whether to try again, how long to wait, and when to
give up. The loop that *uses* those answers is covered -- `tests/
test_retry_idempotency.py` drives real retries -- but the answers themselves were
not, which is the wrong way round: a loop faithfully applying a wrong policy still
passes. Coverage measured with stdlib `trace --count` had 45% of this module
unrun, all of it the decision code.

Two of the uncovered branches are limits rather than behaviour, and an untested
limit is a limit nobody has checked:

- `backoff_seconds` honours a server-sent `RetryInfo.retry_delay` in place of the
  computed jitter, up to `MAX_SERVER_DELAY_MS`. A server that asks for an hour is
  capped at 30 s. If the cap were wrong, a client would obey it.
- `uuidv7_time` reads the timestamp out of a minted key. Its own docstring warns
  that it is the first *twelve* hex digits rather than eight, and that reading
  eight "would return a number around 2**25, which is January 1970" -- a
  plausible bug that only an assertion catches.
"""

from __future__ import annotations

import time
import uuid

from connectrpc.code import Code

from loams.runtime.retry import (
    BASE_DELAY_MS,
    DEFAULT_MAX_RETRIES,
    MAX_DELAY_MS,
    MAX_SERVER_DELAY_MS,
    RETRYABLE_CODES,
    backoff_seconds,
    is_retryable_code,
    should_retry,
    sleep,
)
from loams.runtime.uuidv7 import uuidv7, uuidv7_time

# -- which codes may be retried ------------------------------------------------


def test_only_the_three_transient_codes_are_retryable() -> None:
    """D610. Everything else is a decision the client must not make on its own."""
    assert RETRYABLE_CODES == frozenset(
        {Code.UNAVAILABLE, Code.DEADLINE_EXCEEDED, Code.RESOURCE_EXHAUSTED}
    ), f"the retryable set is {sorted(c.name for c in RETRYABLE_CODES)}"
    assert is_retryable_code(Code.UNAVAILABLE)
    assert not is_retryable_code(Code.NOT_FOUND)
    assert not is_retryable_code(Code.PERMISSION_DENIED)
    assert not is_retryable_code(Code.INVALID_ARGUMENT)
    assert not is_retryable_code(Code.UNIMPLEMENTED)


# -- how long to wait ---------------------------------------------------------


def test_full_jitter_stays_inside_the_ceiling() -> None:
    """M1.6 Ruling 5's numbers, shared by every SDK: 100 ms base, doubling, 2 s cap.

    Full jitter rather than plain exponential, because every client backing off at
    the same instant is how a recovering node gets knocked over again. So the
    assertion is the range, not the exact value.
    """
    for attempt in range(8):
        ceiling_ms = min(MAX_DELAY_MS, BASE_DELAY_MS * 2**attempt)
        for _ in range(50):
            delay = backoff_seconds(attempt)
            assert 0.0 <= delay <= ceiling_ms / 1000.0, (
                f"attempt {attempt} waited {delay:.4f}s, outside "
                f"[0, {ceiling_ms / 1000.0:.4f}]"
            )


def test_the_backoff_doubles_until_it_is_capped() -> None:
    """Beyond attempt 5 the ceiling is the 2 s cap, not a growing number."""
    assert min(MAX_DELAY_MS, BASE_DELAY_MS * 2**5) == MAX_DELAY_MS
    assert min(MAX_DELAY_MS, BASE_DELAY_MS * 2**20) == MAX_DELAY_MS, (
        "the ceiling grows past the cap, so a long-out retry would wait minutes"
    )
    assert backoff_seconds(20) <= MAX_DELAY_MS / 1000.0


def test_a_server_sent_delay_replaces_the_jitter() -> None:
    """`RetryInfo.retry_delay` wins over the computed backoff, in milliseconds."""
    assert backoff_seconds(0, server_delay_ms=250) == 0.250
    assert backoff_seconds(9, server_delay_ms=1_500) == 1.500, (
        "a server delay was overridden by the local backoff"
    )


def test_a_server_sent_delay_is_capped() -> None:
    """The reason the cap exists: a server asking for an hour does not get an hour."""
    assert MAX_SERVER_DELAY_MS == 30_000
    assert backoff_seconds(0, server_delay_ms=3_600_000) == 30.0, (
        "an absurd server delay was obeyed rather than capped"
    )
    assert backoff_seconds(0, server_delay_ms=10**12) == 30.0


def test_a_server_delay_of_zero_or_less_falls_back_to_the_jitter() -> None:
    """`> 0` is the guard, so a server saying "retry now" does not mean "wait forever"."""
    for attempt in (0, 4):
        assert backoff_seconds(attempt, server_delay_ms=0) <= backoff_seconds(attempt) + 2.0
    assert backoff_seconds(0, server_delay_ms=-5) <= MAX_DELAY_MS / 1000.0


def test_sleep_does_not_wait_for_a_non_positive_delay() -> None:
    """`sleep(0)` is on the retry path and in tests; time.sleep(0) is harmless but
    `time.sleep(-1)` raises, so the guard is load-bearing."""
    started = time.monotonic()
    sleep(0)
    sleep(-1.0)
    assert time.monotonic() - started < 0.5, "a non-positive delay was slept on"


# -- when to give up ----------------------------------------------------------


def test_a_call_gives_up_at_max_retries() -> None:
    assert not should_retry(Code.UNAVAILABLE, True, attempt=3, max_retries=3)
    assert should_retry(Code.UNAVAILABLE, True, attempt=2, max_retries=3)


def test_a_unsafe_call_does_not_retry_on_a_transient_code() -> None:
    """`retry_safe` comes from the generated binding, never a guess.

    A mutation without an idempotency key retries into a duplicate write, so a
    transient code is not on its own a reason.
    """
    assert not should_retry(Code.UNAVAILABLE, False, attempt=0, max_retries=3)


def test_a_retryable_call_does_not_retry_an_unretryable_code() -> None:
    """The two conditions are independent: a safe call still stops on NOT_FOUND."""
    assert not should_retry(Code.NOT_FOUND, True, attempt=0, max_retries=3)
    assert not should_retry(Code.PERMISSION_DENIED, True, attempt=0, max_retries=3)


def test_the_default_is_three_retries_after_the_first_attempt() -> None:
    assert DEFAULT_MAX_RETRIES == 3
    assert should_retry(Code.UNAVAILABLE, True, 0, DEFAULT_MAX_RETRIES)
    assert not should_retry(Code.UNAVAILABLE, True, 3, DEFAULT_MAX_RETRIES)


def test_zero_retries_means_one_attempt() -> None:
    assert not should_retry(Code.UNAVAILABLE, True, attempt=0, max_retries=0)


# -- the key the policy mints --------------------------------------------------


def test_a_minted_key_is_a_well_formed_v7() -> None:
    parsed = uuid.UUID(uuidv7())
    assert parsed.version == 7
    assert uuidv7_time(str(parsed)) is not None, "our own key did not parse back"


def test_minted_keys_are_unique() -> None:
    """The counter inside a millisecond is what stops a burst colliding."""
    keys = {uuidv7() for _ in range(5_000)}
    assert len(keys) == 5_000, f"a burst of 5000 minted only {len(keys)} distinct keys"


def test_minted_keys_sort_in_issue_order() -> None:
    """Time-ordered is the whole reason for v7 over v4, and the reason an operator
    can find a stalled set of mutations by range scan."""
    keys = [uuidv7() for _ in range(200)]
    assert keys == sorted(keys), "keys minted in order did not sort in that order"


def test_a_burst_inside_one_millisecond_still_sorts_and_stays_unique(monkeypatch) -> None:
    """The counter wraps at 4096, and the wrap is where ordering is easiest to lose.

    With the clock frozen, every key lands in one millisecond, so the counter has
    to carry 5000 of them -- which is past 4096, so this exercises the borrow into
    the next millisecond. Wrapping instead would hand out a key that sorts before
    one already issued, and reusing a counter value would collide.
    """
    import loams.runtime.uuidv7 as module

    monkeypatch.setattr(module.time, "time", lambda: 1_700_000_000.0)
    # The module keeps process-wide tick state, and earlier tests in this run
    # already issued keys at the real current millisecond. Freezing below that
    # would -- correctly -- hit the backwards-clock branch and keep the later
    # timestamp, so the burst has to start from a reset to be the intended test.
    monkeypatch.setattr(module, "_tick_ms", 0)
    monkeypatch.setattr(module, "_tick_counter", 0)
    keys = [module.uuidv7() for _ in range(5_000)]

    assert len(set(keys)) == 5_000, f"a frozen-clock burst produced {len(set(keys))} distinct keys"
    assert keys == sorted(keys), (
        "keys minted inside one frozen millisecond did not sort in issue order"
    )
    # The borrow should have advanced the timestamp by exactly one millisecond
    # past the wrap, not further.
    first, last = uuidv7_time(keys[0]), uuidv7_time(keys[-1])
    assert first == 1_700_000_000_000, f"the first key read back as {first}"
    assert last == first + 1, (
        f"the burst spanned {last - first} ms, want 1 for 5000 keys over 4096 slots"
    )


def test_uuidv7_time_reads_the_current_millisecond() -> None:
    """Twelve hex digits, not eight.

    This is the assertion the docstring's warning is really about: eight digits
    is 32 bits, which for milliseconds since the epoch is January 1970. Reading
    the right number of digits is the whole behaviour of this function.
    """
    before = int(time.time() * 1000)
    key = uuidv7()
    after = int(time.time() * 1000)
    stamp = uuidv7_time(key)
    assert stamp is not None
    assert before <= stamp <= after, (
        f"the key {key} read back as {stamp}, outside [{before}, {after}]; "
        "the timestamp is not being read from the first 48 bits"
    )


def test_uuidv7_time_reads_forty_eight_bits() -> None:
    """48 bits of milliseconds is good until 10889, so the width has to be right."""
    assert len(str(uuidv7().replace("-", ""))[:12]) == 12
    # 2**48 ms is year 10889, so a correct 48-bit read is far past any 32-bit value.
    assert uuidv7_time(uuidv7()) > 2**32, (
        "the timestamp fits in 32 bits, so fewer than 48 bits were read"
    )


def test_uuidv7_time_refuses_anything_that_is_not_a_v7() -> None:
    assert uuidv7_time("") is None
    assert uuidv7_time("not-a-uuid") is None
    assert uuidv7_time(str(uuid.uuid4())) is None, "a v4 was accepted as a v7"
    assert uuidv7_time(str(uuid.uuid1())) is None
    assert uuidv7_time(uuidv7().upper()) is None, (
        "the pattern is lower-case only, so an upper-case key reads as invalid"
    )
    assert uuidv7_time(uuidv7() + "0") is None, "a trailing character was accepted"