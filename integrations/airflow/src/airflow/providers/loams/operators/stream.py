# SPDX-License-Identifier: Apache-2.0
"""Operators on Loams streams, through the Iggy Kafka gateway."""

from __future__ import annotations

from collections.abc import Callable, Iterable, Mapping, Sequence
from typing import TYPE_CHECKING, Any

from airflow.sdk import BaseOperator

from airflow.providers.loams.hooks.stream import LoamsStreamHook
from airflow.providers.loams.triggers.stream import OFFSET_MODES, LoamsStreamOffsetTrigger

if TYPE_CHECKING:
    from airflow.sdk import Context

__all__ = ["LoamsStreamConsumeOperator", "LoamsStreamProduceOperator", "LoamsStreamWaitOperator"]


class LoamsStreamProduceOperator(BaseOperator):
    """Produces records to a stream and waits until each is acknowledged.

    :param topic: the stream's topic.
    :param records: the records (templated): a list of dicts with `value`
        and optionally `key`, `headers` and `partition`, or bare values.
        Text is sent as UTF-8, bytes as they are, anything else as JSON.
    :param producer_function: a callable answering the records instead,
        called with `producer_function_args`/`kwargs` (and the context, if it
        takes `context`).
    :param flush_timeout: seconds to wait for every acknowledgement.
    :return: each record's topic, partition and offset.
    """

    template_fields: Sequence[str] = (
        "topic",
        "records",
        "producer_function_args",
        "producer_function_kwargs",
    )
    template_fields_renderers = {"records": "json"}
    ui_color = "#b5d6f4"

    def __init__(
        self,
        *,
        topic: str,
        records: Iterable[Any] | None = None,
        producer_function: Callable[..., Iterable[Any]] | None = None,
        producer_function_args: Sequence[Any] | None = None,
        producer_function_kwargs: Mapping[str, Any] | None = None,
        loams_conn_id: str = LoamsStreamHook.default_conn_name,
        flush_timeout: float = 30.0,
        **kwargs: Any,
    ) -> None:
        super().__init__(**kwargs)
        if (records is None) == (producer_function is None):
            raise ValueError("pass exactly one of records and producer_function")
        self.topic = topic
        self.records = records
        self.producer_function = producer_function
        self.producer_function_args = list(producer_function_args or [])
        self.producer_function_kwargs = dict(producer_function_kwargs or {})
        self.loams_conn_id = loams_conn_id
        self.flush_timeout = flush_timeout

    def _records(self, context: Context) -> Iterable[Any]:
        if self.producer_function is None:
            return list(self.records or [])
        kwargs = dict(self.producer_function_kwargs)
        code = getattr(self.producer_function, "__code__", None)
        if code is not None and "context" in code.co_varnames[: code.co_argcount + code.co_kwonlyargcount]:
            kwargs.setdefault("context", context)
        return self.producer_function(*self.producer_function_args, **kwargs)

    def execute(self, context: Context) -> list[dict[str, Any]]:
        hook = LoamsStreamHook(loams_conn_id=self.loams_conn_id)
        delivered = hook.produce(self.topic, self._records(context), flush_timeout=self.flush_timeout)
        self.log.info("Produced %d records to %s", len(delivered), self.topic)
        return delivered


class LoamsStreamConsumeOperator(BaseOperator):
    """Reads records from streams as a consumer group and commits its offsets.

    :param topics: a topic or topics.
    :param group_id: the consumer group (templated); its committed offsets
        are where the next run starts.
    :param max_messages: stop after this many records.
    :param poll_timeout: stop when one poll waits this long for nothing.
    :param decode: `text`, `json` or `bytes` (base64) for keys and values.
    :param apply_function: called with the list of records; its answer is
        the task's return value (else the records are). Use it to keep large
        reads out of XCom.
    :param commit: commit the group's offsets after a successful read.
    """

    template_fields: Sequence[str] = ("topics", "group_id")
    ui_color = "#b5d6f4"

    def __init__(
        self,
        *,
        topics: str | Sequence[str],
        group_id: str,
        loams_conn_id: str = LoamsStreamHook.default_conn_name,
        max_messages: int = 100,
        poll_timeout: float = 5.0,
        decode: str = "text",
        apply_function: Callable[[list[dict[str, Any]]], Any] | None = None,
        commit: bool = True,
        **kwargs: Any,
    ) -> None:
        super().__init__(**kwargs)
        if decode not in ("text", "json", "bytes"):
            raise ValueError("decode must be 'text', 'json' or 'bytes'")
        self.topics = topics
        self.group_id = group_id
        self.loams_conn_id = loams_conn_id
        self.max_messages = max_messages
        self.poll_timeout = poll_timeout
        self.decode = decode
        self.apply_function = apply_function
        self.commit = commit

    def execute(self, context: Context) -> Any:
        hook = LoamsStreamHook(loams_conn_id=self.loams_conn_id)
        if self.apply_function is None:
            records = hook.consume(
                self.topics,
                group_id=self.group_id,
                max_messages=self.max_messages,
                poll_timeout=self.poll_timeout,
                decode=self.decode,
                commit=self.commit,
            )
            self.log.info("Read %d records from %s", len(records), self.topics)
            return records
        # With a function, commit only once it has handled the records, so a
        # failure in it leaves them to the next run.
        consumer = hook.get_consumer(self.group_id)
        try:
            records = hook.consume(
                self.topics,
                group_id=self.group_id,
                max_messages=self.max_messages,
                poll_timeout=self.poll_timeout,
                decode=self.decode,
                commit=False,
                consumer=consumer,
            )
            self.log.info("Read %d records from %s", len(records), self.topics)
            answer = self.apply_function(records)
            if self.commit and records:
                consumer.commit(asynchronous=False)
            return answer
        finally:
            consumer.close()


class LoamsStreamWaitOperator(BaseOperator):
    """Waits, deferred, until a stream's offsets reach a point (see `check_offsets`).

    Prefer `LoamsStreamOffsetSensor` in most DAGs; this operator always
    defers and returns the offsets it saw.
    """

    template_fields: Sequence[str] = ("topic", "group_id", "target_offsets")

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
        **kwargs: Any,
    ) -> None:
        super().__init__(**kwargs)
        if mode not in OFFSET_MODES:
            raise ValueError(f"mode must be one of {OFFSET_MODES}")
        self.topic = topic
        self.mode = mode
        self.target_offsets = dict(target_offsets or {})
        self.group_id = group_id
        self.partitions = partitions
        self.min_records = min_records
        self.loams_conn_id = loams_conn_id
        self.poll_interval = poll_interval

    def execute(self, context: Context) -> None:
        self.defer(
            trigger=LoamsStreamOffsetTrigger(
                topic=self.topic,
                mode=self.mode,
                target_offsets=self.target_offsets,
                group_id=self.group_id,
                partitions=self.partitions,
                min_records=self.min_records,
                loams_conn_id=self.loams_conn_id,
                poll_interval=self.poll_interval,
            ),
            method_name="execute_complete",
        )

    def execute_complete(self, context: Context, event: dict[str, Any]) -> dict[str, Any]:
        if event.get("status") != "success":
            raise RuntimeError(f"waiting on stream {self.topic} failed: {event.get('message')}")
        return event
