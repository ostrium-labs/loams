# SPDX-License-Identifier: Apache-2.0
"""`LoamsStreamHook`: produce to and consume from streams through the Kafka gateway.

Loams's Kafka wire front end is the Iggy Kafka gateway (the `kafka` profile of
the release compose bundle; `docs/guides/kafka.md`). Any Kafka client talks to
it, so the hook uses `confluent-kafka` (librdkafka) with the bootstrap
address, SASL/PLAIN and any client settings from the `loams` connection.

Status (preview, as the guide says): records land in Iggy, and the Event
Fabric bridge that carries Iggy topics into Loams streams is planned, not
built. Transactions and exactly-once are not supported by the gateway, so
the hook never asks for them.
"""

from __future__ import annotations

import base64
import json
from collections.abc import Callable, Iterable, Mapping, Sequence
from typing import TYPE_CHECKING, Any

from airflow.providers.loams.hooks.base import LoamsHook

if TYPE_CHECKING:
    from confluent_kafka import Consumer, Message, Producer
    from confluent_kafka.admin import AdminClient

__all__ = ["LoamsStreamHook", "decode_message", "encode_value"]

# librdkafka's "no committed offset" sentinel (OFFSET_INVALID).
OFFSET_INVALID = -1001


def _kafka() -> Any:
    try:
        import confluent_kafka
    except ImportError as exc:  # pragma: no cover - the message is the point
        raise ImportError(
            "Loams streams need confluent-kafka: pip install 'airflow-provider-loams[kafka]'"
        ) from exc
    return confluent_kafka


def encode_value(value: Any) -> bytes | None:
    """A record key or value as bytes: bytes stay, text is UTF-8, anything else JSON."""
    if value is None:
        return None
    if isinstance(value, (bytes, bytearray, memoryview)):
        return bytes(value)
    if isinstance(value, str):
        return value.encode()
    return json.dumps(value, separators=(",", ":"), default=str).encode()


def _decode(data: bytes | None, how: str) -> Any:
    if data is None:
        return None
    if how == "bytes":
        return base64.b64encode(data).decode()
    text = data.decode("utf-8", errors="replace")
    if how == "json":
        try:
            return json.loads(text)
        except json.JSONDecodeError:
            return text
    return text


def decode_message(message: Message, *, decode: str = "text") -> dict[str, Any]:
    """A consumed record as a JSON-safe dict (for XCom and trigger events).

    :param decode: `text` (UTF-8), `json` (parsed when it parses, else text)
        or `bytes` (base64).
    """
    _ts_type, timestamp = message.timestamp()
    headers = message.headers() or []
    return {
        "topic": message.topic(),
        "partition": message.partition(),
        "offset": message.offset(),
        "timestamp": timestamp,
        "key": _decode(message.key(), "text" if decode == "json" else decode),
        "value": _decode(message.value(), decode),
        "headers": {k: _decode(v, "text") for k, v in headers},
    }


class LoamsStreamHook(LoamsHook):
    """Kafka clients for Loams streams, through the Iggy Kafka gateway.

    :param loams_conn_id: the `loams` connection; its extras
        `kafka_bootstrap_servers` (required), `kafka_security_protocol`,
        `kafka_sasl_mechanism` (`PLAIN`), `kafka_sasl_username`,
        `kafka_sasl_password` and `kafka_config` (any librdkafka setting)
        configure the clients.
    """

    hook_name = "Loams Streams (Kafka gateway)"

    def kafka_config(self, **overrides: Any) -> dict[str, Any]:
        """librdkafka settings for a client of this connection."""
        s = self.settings
        if not s.kafka_bootstrap_servers and "bootstrap.servers" not in s.kafka_config:
            raise ValueError(
                "the Loams connection has no Kafka gateway: set its extra `kafka_bootstrap_servers`"
            )
        config: dict[str, Any] = {"client.id": "airflow-loams"}
        if s.kafka_bootstrap_servers:
            config["bootstrap.servers"] = s.kafka_bootstrap_servers
        if s.kafka_sasl_username:
            config["security.protocol"] = s.kafka_security_protocol or "SASL_PLAINTEXT"
            config["sasl.mechanisms"] = s.kafka_sasl_mechanism or "PLAIN"
            config["sasl.username"] = s.kafka_sasl_username
            config["sasl.password"] = s.kafka_sasl_password or ""
        elif s.kafka_security_protocol:
            config["security.protocol"] = s.kafka_security_protocol
        config.update(s.kafka_config)
        config.update(overrides)
        return config

    def get_producer(self, **overrides: Any) -> Producer:
        # The gateway has no transactions; idempotence needs them on some
        # brokers, so it stays off unless the connection turns it on.
        config = {"enable.idempotence": False, "acks": "all", **self.kafka_config(**overrides)}
        return _kafka().Producer(config)

    def get_consumer(self, group_id: str, **overrides: Any) -> Consumer:
        config = {
            "group.id": group_id,
            "auto.offset.reset": "earliest",
            "enable.auto.commit": False,
            "isolation.level": "read_uncommitted",
            **self.kafka_config(**overrides),
        }
        return _kafka().Consumer(config)

    def get_admin_client(self, **overrides: Any) -> AdminClient:
        from confluent_kafka.admin import AdminClient

        return AdminClient(self.kafka_config(**overrides))

    # -- produce -------------------------------------------------------------

    def produce(
        self,
        topic: str,
        records: Iterable[Any],
        *,
        flush_timeout: float = 30.0,
        producer: Producer | None = None,
    ) -> list[dict[str, Any]]:
        """Produces records and waits until each is acknowledged.

        A record is a dict with `value` and optionally `key`, `headers`
        (a dict) and `partition`, or a bare value. Answers each record's
        topic, partition and offset, in order; raises if any delivery failed.
        """
        producer = producer or self.get_producer()
        delivered: list[dict[str, Any] | None] = []
        errors: list[str] = []

        def on_delivery(index: int) -> Callable[[Any, Any], None]:
            def callback(err: Any, msg: Any) -> None:
                if err is not None:
                    errors.append(f"record {index}: {err}")
                    return
                delivered[index] = {
                    "topic": msg.topic(),
                    "partition": msg.partition(),
                    "offset": msg.offset(),
                }

            return callback

        for index, record in enumerate(records):
            delivered.append(None)
            if isinstance(record, Mapping) and "value" in record:
                key = record.get("key")
                value = record.get("value")
                headers = record.get("headers") or {}
                partition = record.get("partition")
            else:
                key, value, headers, partition = None, record, {}, None
            kwargs: dict[str, Any] = {
                "key": encode_value(key),
                "value": encode_value(value),
                "headers": [(str(k), encode_value(v)) for k, v in dict(headers).items()],
                "on_delivery": on_delivery(index),
            }
            if partition is not None:
                kwargs["partition"] = int(partition)
            while True:
                try:
                    producer.produce(topic, **kwargs)
                    break
                except BufferError:
                    # The local queue is full: serve callbacks and retry.
                    producer.poll(0.5)
            producer.poll(0)
        remaining = producer.flush(flush_timeout)
        if remaining:
            raise TimeoutError(f"{remaining} records to {topic} were not acknowledged in {flush_timeout}s")
        if errors:
            raise RuntimeError(f"producing to {topic} failed: " + "; ".join(errors[:10]))
        return [d for d in delivered if d is not None]

    # -- consume -------------------------------------------------------------

    def consume(
        self,
        topics: str | Sequence[str],
        *,
        group_id: str,
        max_messages: int = 100,
        poll_timeout: float = 5.0,
        commit: bool = True,
        decode: str = "text",
        consumer: Consumer | None = None,
    ) -> list[dict[str, Any]]:
        """Reads up to `max_messages` records as the consumer group, then commits.

        Stops at `max_messages` or when one poll waits `poll_timeout` seconds
        for nothing. With `commit`, the group's offsets move past what was
        read (synchronously, before this returns).
        """
        topic_list = [topics] if isinstance(topics, str) else list(topics)
        own = consumer is None
        consumer = consumer or self.get_consumer(group_id)
        out: list[dict[str, Any]] = []
        try:
            consumer.subscribe(topic_list)
            while len(out) < max_messages:
                batch = consumer.consume(num_messages=max_messages - len(out), timeout=poll_timeout)
                if not batch:
                    break
                for message in batch:
                    error = message.error()
                    if error is not None:
                        if error.code() == _kafka().KafkaError._PARTITION_EOF:
                            continue
                        raise RuntimeError(f"consuming {topic_list} failed: {error}")
                    out.append(decode_message(message, decode=decode))
            if commit and out:
                consumer.commit(asynchronous=False)
        finally:
            if own:
                consumer.close()
        return out

    # -- offsets -------------------------------------------------------------

    def partitions(self, topic: str, *, timeout: float = 10.0) -> list[int]:
        metadata = self.get_admin_client().list_topics(topic=topic, timeout=timeout)
        info = metadata.topics.get(topic)
        if info is None or info.error is not None:
            raise ValueError(f"topic {topic} not found ({getattr(info, 'error', None)})")
        return sorted(info.partitions)

    def watermarks(
        self,
        topic: str,
        partitions: Sequence[int] | None = None,
        *,
        timeout: float = 10.0,
        consumer: Consumer | None = None,
    ) -> dict[int, tuple[int, int]]:
        """Each partition's (low, high) watermark: high is the next offset to be written."""
        own = consumer is None
        consumer = consumer or self.get_consumer("airflow-loams-watermarks")
        try:
            parts = list(partitions) if partitions is not None else self.partitions(topic, timeout=timeout)
            k = _kafka()
            out: dict[int, tuple[int, int]] = {}
            for p in parts:
                low, high = consumer.get_watermark_offsets(k.TopicPartition(topic, p), timeout=timeout)
                out[int(p)] = (int(low), int(high))
            return out
        finally:
            if own:
                consumer.close()

    def committed(
        self,
        group_id: str,
        topic: str,
        partitions: Sequence[int] | None = None,
        *,
        timeout: float = 10.0,
    ) -> dict[int, int | None]:
        """The group's committed offset per partition, None where it has none."""
        consumer = self.get_consumer(group_id)
        try:
            parts = list(partitions) if partitions is not None else self.partitions(topic, timeout=timeout)
            k = _kafka()
            answers = consumer.committed([k.TopicPartition(topic, p) for p in parts], timeout=timeout)
            return {
                int(tp.partition): (None if tp.offset < 0 or tp.offset == OFFSET_INVALID else int(tp.offset))
                for tp in answers
            }
        finally:
            consumer.close()

    def lag(
        self, group_id: str, topic: str, partitions: Sequence[int] | None = None, *, timeout: float = 10.0
    ) -> dict[int, int]:
        """Records the group has not consumed, per partition."""
        marks = self.watermarks(topic, partitions, timeout=timeout)
        done = self.committed(group_id, topic, list(marks), timeout=timeout)
        out: dict[int, int] = {}
        for p, (low, high) in marks.items():
            position = done.get(p)
            out[p] = max(0, high - (low if position is None else position))
        return out

    def test_connection(self) -> tuple[bool, str]:
        try:
            metadata = self.get_admin_client().list_topics(timeout=10)
            return True, f"Kafka gateway answered with {len(metadata.topics)} topics"
        except Exception as exc:
            return False, str(exc)
