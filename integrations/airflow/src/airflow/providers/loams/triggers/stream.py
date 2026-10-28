# SPDX-License-Identifier: Apache-2.0
"""Triggers on Loams streams, through the Iggy Kafka gateway.

- `LoamsStreamOffsetTrigger` (deferrable tasks and `LoamsStreamOffsetSensor`):
  waits until a topic's offsets reach a point: a target offset per
  partition, new records for a consumer group, or a group that has caught up.
- `LoamsStreamMessageTrigger` (a `BaseEventTrigger`, for `AssetWatcher`):
  fires once per matching record, as a consumer group, so a record on a
  stream (a CloudEvent such as a table commit) becomes an asset event.

confluent-kafka is a blocking client, so each Kafka call runs in a worker
thread (`asyncio.to_thread`) and the triggerer's loop is never blocked.
"""

from __future__ import annotations

import asyncio
from collections.abc import AsyncIterator, Mapping, Sequence
from typing import Any

from airflow.triggers.base import BaseEventTrigger, BaseTrigger, TriggerEvent

from airflow.providers.loams.hooks.stream import LoamsStreamHook, decode_message

__all__ = ["OFFSET_MODES", "LoamsStreamMessageTrigger", "LoamsStreamOffsetTrigger", "check_offsets"]

OFFSET_MODES = ("offset", "new_records", "caught_up")


def check_offsets(
    hook: LoamsStreamHook,
    *,
    topic: str,
    mode: str,
    target_offsets: Mapping[Any, int] | None = None,
    group_id: str | None = None,
    partitions: Sequence[int] | None = None,
    min_records: int = 1,
) -> tuple[bool, dict[str, Any]]:
    """Whether a topic's offsets have reached the point `mode` names.

    - `offset`: every partition in `target_offsets` has a high watermark (the
      next offset to be written) at or past its target;
    - `new_records`: the group `group_id` has at least `min_records` records
      it has not consumed;
    - `caught_up`: the group has consumed everything (lag 0 everywhere).

    Answers the verdict and the offsets it saw (JSON-safe, partitions as text).
    """
    if mode not in OFFSET_MODES:
        raise ValueError(f"mode must be one of {OFFSET_MODES}, not {mode!r}")
    if mode == "offset":
        if not target_offsets:
            raise ValueError("mode 'offset' needs target_offsets: {partition: offset}")
        targets = {int(p): int(o) for p, o in target_offsets.items()}
        marks = hook.watermarks(topic, sorted(targets))
        state = {"watermarks": {str(p): [lo, hi] for p, (lo, hi) in marks.items()}}
        done = all(marks.get(p, (0, -1))[1] >= target for p, target in targets.items())
        return done, state
    if not group_id:
        raise ValueError(f"mode {mode!r} needs group_id")
    lag = hook.lag(group_id, topic, partitions)
    state = {"lag": {str(p): n for p, n in lag.items()}}
    total = sum(lag.values())
    if mode == "new_records":
        return total >= max(1, int(min_records)), state
    return total == 0, state


class LoamsStreamOffsetTrigger(BaseTrigger):
    """Polls a topic's offsets until `check_offsets` says done.

    :param topic: the stream's topic on the Kafka gateway.
    :param mode: `offset`, `new_records` or `caught_up` (see `check_offsets`).
    :param target_offsets: for `offset`, {partition: offset}.
    :param group_id: for `new_records` and `caught_up`, the consumer group.
    :param partitions: the partitions to watch (all of the topic's by default).
    :param min_records: for `new_records`, how many unconsumed records count.
    :param poll_interval: seconds between checks.
    """

    def __init__(
        self,
        *,
        topic: str,
        mode: str = "new_records",
        target_offsets: Mapping[Any, int] | None = None,
        group_id: str | None = None,
        partitions: Sequence[int] | None = None,
        min_records: int = 1,
        loams_conn_id: str = LoamsStreamHook.default_conn_name,
        poll_interval: float = 10.0,
    ) -> None:
        super().__init__()
        if mode not in OFFSET_MODES:
            raise ValueError(f"mode must be one of {OFFSET_MODES}, not {mode!r}")
        self.topic = topic
        self.mode = mode
        self.target_offsets = {str(p): int(o) for p, o in (target_offsets or {}).items()}
        self.group_id = group_id
        self.partitions = list(partitions) if partitions is not None else None
        self.min_records = min_records
        self.loams_conn_id = loams_conn_id
        self.poll_interval = poll_interval

    def serialize(self) -> tuple[str, dict[str, Any]]:
        return (
            f"{type(self).__module__}.{type(self).__qualname__}",
            {
                "topic": self.topic,
                "mode": self.mode,
                "target_offsets": self.target_offsets,
                "group_id": self.group_id,
                "partitions": self.partitions,
                "min_records": self.min_records,
                "loams_conn_id": self.loams_conn_id,
                "poll_interval": self.poll_interval,
            },
        )

    def _check(self) -> tuple[bool, dict[str, Any]]:
        return check_offsets(
            LoamsStreamHook(loams_conn_id=self.loams_conn_id),
            topic=self.topic,
            mode=self.mode,
            target_offsets=self.target_offsets,
            group_id=self.group_id,
            partitions=self.partitions,
            min_records=self.min_records,
        )

    async def run(self) -> AsyncIterator[TriggerEvent]:
        while True:
            try:
                done, state = await asyncio.to_thread(self._check)
            except Exception as exc:
                yield TriggerEvent(
                    {"status": "error", "topic": self.topic, "mode": self.mode, "message": str(exc)}
                )
                return
            if done:
                yield TriggerEvent({"status": "success", "topic": self.topic, "mode": self.mode, **state})
                return
            self.log.debug("Stream %s not there yet (%s): %s", self.topic, self.mode, state)
            await asyncio.sleep(self.poll_interval)


def matches(value: Any, match: Mapping[str, Any] | None) -> bool:
    """True when every `match` key is in the (JSON) value with the same value.

    Dotted keys look into nested objects: `{"data.table": "orders"}`.
    """
    if not match:
        return True
    if not isinstance(value, Mapping):
        return False
    for key, expected in match.items():
        current: Any = value
        for part in str(key).split("."):
            if not isinstance(current, Mapping) or part not in current:
                return False
            current = current[part]
        if current != expected:
            return False
    return True


class LoamsStreamMessageTrigger(BaseEventTrigger):
    """Fires on the next matching record of a topic, as a consumer group.

    For `AssetWatcher`: the triggerer runs it, it yields one event per
    matching record and ends, and the triggerer starts it again; the group's
    committed offset is where it resumes. The offset is committed before the
    event is yielded, so a record fires at most once (a crash between the
    two loses that event rather than repeating it).

    :param topics: a topic or topics on the Kafka gateway.
    :param group_id: the consumer group; give each watcher its own.
    :param match: fire only for records whose JSON value has these keys and
        values (dotted keys look into nested objects); others are skipped.
    :param decode: `json`, `text` or `bytes` (base64) for the event's value.
    :param poll_timeout: seconds one poll waits.
    """

    def __init__(
        self,
        *,
        topics: str | Sequence[str],
        group_id: str,
        match: Mapping[str, Any] | None = None,
        decode: str = "json",
        loams_conn_id: str = LoamsStreamHook.default_conn_name,
        poll_timeout: float = 1.0,
    ) -> None:
        super().__init__()
        self.topics = [topics] if isinstance(topics, str) else list(topics)
        self.group_id = group_id
        self.match = dict(match or {})
        self.decode = decode
        self.loams_conn_id = loams_conn_id
        self.poll_timeout = poll_timeout

    def serialize(self) -> tuple[str, dict[str, Any]]:
        return (
            f"{type(self).__module__}.{type(self).__qualname__}",
            {
                "topics": self.topics,
                "group_id": self.group_id,
                "match": self.match,
                "decode": self.decode,
                "loams_conn_id": self.loams_conn_id,
                "poll_timeout": self.poll_timeout,
            },
        )

    async def run(self) -> AsyncIterator[TriggerEvent]:
        hook = LoamsStreamHook(loams_conn_id=self.loams_conn_id)
        consumer = await asyncio.to_thread(hook.get_consumer, self.group_id)
        try:
            await asyncio.to_thread(consumer.subscribe, self.topics)
            while True:
                message = await asyncio.to_thread(consumer.poll, self.poll_timeout)
                if message is None:
                    continue
                error = message.error()
                if error is not None:
                    from confluent_kafka import KafkaError

                    if error.code() == KafkaError._PARTITION_EOF:
                        continue
                    raise RuntimeError(f"consuming {self.topics} failed: {error}")
                record = decode_message(message, decode=self.decode)
                await asyncio.to_thread(consumer.commit, message=message, asynchronous=False)
                if not matches(record["value"], self.match):
                    continue
                yield TriggerEvent(record)
                return
        finally:
            await asyncio.to_thread(consumer.close)
