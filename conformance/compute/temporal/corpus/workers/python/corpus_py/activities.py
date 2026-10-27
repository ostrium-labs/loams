"""The corpus's Python activities, named as the Go and TypeScript ones."""

from __future__ import annotations

import asyncio

from temporalio import activity
from temporalio.exceptions import ApplicationError


@activity.defn(name="greet")
async def greet(name: str) -> str:
    return f"hello {name}"


@activity.defn(name="heartbeat_thrice")
async def heartbeat_thrice() -> int:
    for i in range(1, 4):
        activity.heartbeat(i)
    return 3


@activity.defn(name="fail_twice")
async def fail_twice() -> int:
    attempt = activity.info().attempt
    if attempt < 3:
        raise ApplicationError(f"attempt {attempt} fails", type="Transient")
    return attempt


@activity.defn(name="sleep_for")
async def sleep_for(ms: int) -> None:
    # Cancellable: the worker cancels it at its start-to-close timeout.
    await asyncio.sleep(ms / 1000)


@activity.defn(name="large")
async def large(n: int) -> str:
    return "x" * n


ACTIVITIES = [greet, heartbeat_thrice, fail_twice, sleep_for, large]
