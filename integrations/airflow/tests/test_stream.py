# SPDX-License-Identifier: Apache-2.0
from __future__ import annotations

from types import SimpleNamespace
from unittest import mock

import pytest
from airflow.sdk.exceptions import TaskDeferred

from airflow.providers.loams.hooks.stream import LoamsStreamHook, decode_message, encode_value
from airflow.providers.loams.operators.stream import (
    LoamsStreamConsumeOperator,
    LoamsStreamProduceOperator,
    LoamsStreamWaitOperator,
)
from airflow.providers.loams.sensors.stream import LoamsStreamOffsetSensor
from airflow.providers.loams.triggers.stream import check_offsets, matches


class FakeMessage:
    def __init__(self, value: bytes | None, *, key: bytes | None = None, offset: int = 0, error=None) -> None:
        self._value, self._key, self._offset, self._error = value, key, offset, error

    def error(self):
        return self._error

    def topic(self) -> str:
        return "orders"

    def partition(self) -> int:
        return 0

    def offset(self) -> int:
        return self._offset

    def timestamp(self):
        return (1, 1793059200000)

    def key(self):
        return self._key

    def value(self):
        return self._value

    def headers(self):
        return [("h", b"v")]


class FakeProducer:
    def __init__(self, fail: bool = False) -> None:
        self.sent: list[tuple[str, dict]] = []
        self.pending: list = []
        self.fail = fail

    def produce(self, topic: str, **kwargs) -> None:
        self.sent.append((topic, kwargs))
        offset = len(self.sent) - 1
        msg = SimpleNamespace(topic=lambda: topic, partition=lambda: 0, offset=lambda: offset)
        self.pending.append((kwargs["on_delivery"], msg))

    def poll(self, timeout: float) -> int:
        return 0

    def flush(self, timeout: float) -> int:
        for callback, msg in self.pending:
            callback("broker down" if self.fail else None, msg)
        self.pending = []
        return 0


class FakeConsumer:
    def __init__(self, batches: list[list[FakeMessage]]) -> None:
        self.batches = batches
        self.committed_calls = 0
        self.closed = False
        self.subscribed: list[str] = []

    def subscribe(self, topics: list[str]) -> None:
        self.subscribed = topics

    def consume(self, num_messages: int, timeout: float):
        return self.batches.pop(0) if self.batches else []

    def poll(self, timeout: float):
        batch = self.consume(1, timeout)
        return batch[0] if batch else None

    def commit(self, *args, **kwargs) -> None:
        self.committed_calls += 1

    def close(self) -> None:
        self.closed = True


def test_encode_value() -> None:
    assert encode_value(None) is None
    assert encode_value(b"x") == b"x"
    assert encode_value("é") == "é".encode()
    assert encode_value({"a": 1}) == b'{"a":1}'


def test_decode_message() -> None:
    record = decode_message(FakeMessage(b'{"a": 1}', key=b"k", offset=5), decode="json")
    assert record == {
        "topic": "orders",
        "partition": 0,
        "offset": 5,
        "timestamp": 1793059200000,
        "key": "k",
        "value": {"a": 1},
        "headers": {"h": "v"},
    }
    assert decode_message(FakeMessage(b"\x00"), decode="bytes")["value"] == "AA=="
    assert decode_message(FakeMessage(b"not json"), decode="json")["value"] == "not json"


def test_kafka_config(loams_connection) -> None:
    loams_connection(
        {
            "kafka_bootstrap_servers": "gw:9093",
            "kafka_sasl_username": "iggy",
            "kafka_sasl_password": "pw",
            "kafka_config": {"linger.ms": 5},
        }
    )
    config = LoamsStreamHook().kafka_config(**{"client.id": "x"})
    assert config == {
        "client.id": "x",
        "bootstrap.servers": "gw:9093",
        "security.protocol": "SASL_PLAINTEXT",
        "sasl.mechanisms": "PLAIN",
        "sasl.username": "iggy",
        "sasl.password": "pw",
        "linger.ms": 5,
    }


def test_kafka_config_needs_bootstrap(loams_connection) -> None:
    loams_connection({})
    with pytest.raises(ValueError, match="kafka_bootstrap_servers"):
        LoamsStreamHook().kafka_config()


def test_produce(loams_connection) -> None:
    producer = FakeProducer()
    delivered = LoamsStreamHook().produce(
        "orders",
        [{"key": "k", "value": {"a": 1}, "headers": {"h": "v"}, "partition": 2}, "bare"],
        producer=producer,
    )
    assert delivered == [
        {"topic": "orders", "partition": 0, "offset": 0},
        {"topic": "orders", "partition": 0, "offset": 1},
    ]
    first = producer.sent[0][1]
    assert first["key"] == b"k" and first["value"] == b'{"a":1}'
    assert first["headers"] == [("h", b"v")] and first["partition"] == 2
    assert producer.sent[1][1]["value"] == b"bare"


def test_produce_raises_on_delivery_error(loams_connection) -> None:
    with pytest.raises(RuntimeError, match="broker down"):
        LoamsStreamHook().produce("orders", ["x"], producer=FakeProducer(fail=True))


def test_consume_commits(loams_connection) -> None:
    consumer = FakeConsumer([[FakeMessage(b"a"), FakeMessage(b"b", offset=1)], []])
    records = LoamsStreamHook().consume("orders", group_id="g", consumer=consumer, max_messages=10)
    assert [r["value"] for r in records] == ["a", "b"]
    assert consumer.subscribed == ["orders"]
    assert consumer.committed_calls == 1
    assert not consumer.closed  # the caller's consumer stays open


def test_consume_stops_at_max(loams_connection) -> None:
    consumer = FakeConsumer([[FakeMessage(b"a")], [FakeMessage(b"b")], [FakeMessage(b"c")]])
    records = LoamsStreamHook().consume("orders", group_id="g", consumer=consumer, max_messages=2)
    assert len(records) == 2


def test_lag(loams_connection) -> None:
    hook = LoamsStreamHook()
    with (
        mock.patch.object(hook, "watermarks", return_value={0: (0, 10), 1: (5, 5), 2: (3, 8)}),
        mock.patch.object(hook, "committed", return_value={0: 7, 1: 5, 2: None}),
    ):
        assert hook.lag("g", "orders") == {0: 3, 1: 0, 2: 5}


def test_check_offsets_modes() -> None:
    hook = mock.Mock(spec=LoamsStreamHook)
    hook.watermarks.return_value = {0: (0, 10), 1: (0, 4)}
    assert check_offsets(hook, topic="t", mode="offset", target_offsets={"0": 10, "1": 4})[0]
    done, state = check_offsets(hook, topic="t", mode="offset", target_offsets={0: 11})
    assert not done
    assert state == {"watermarks": {"0": [0, 10], "1": [0, 4]}}

    hook.lag.return_value = {0: 0, 1: 2}
    assert check_offsets(hook, topic="t", mode="new_records", group_id="g")[0]
    assert not check_offsets(hook, topic="t", mode="new_records", group_id="g", min_records=3)[0]
    assert not check_offsets(hook, topic="t", mode="caught_up", group_id="g")[0]
    hook.lag.return_value = {0: 0}
    assert check_offsets(hook, topic="t", mode="caught_up", group_id="g")[0]

    with pytest.raises(ValueError, match="group_id"):
        check_offsets(hook, topic="t", mode="caught_up")
    with pytest.raises(ValueError, match="target_offsets"):
        check_offsets(hook, topic="t", mode="offset")
    with pytest.raises(ValueError, match="mode"):
        check_offsets(hook, topic="t", mode="nope")


def test_matches() -> None:
    event = {"type": "commit", "data": {"table": "orders"}}
    assert matches(event, None)
    assert matches(event, {"type": "commit", "data.table": "orders"})
    assert not matches(event, {"data.table": "other"})
    assert not matches(event, {"data.missing": 1})
    assert not matches("text", {"type": "commit"})


def test_produce_operator(loams_connection, context) -> None:
    op = LoamsStreamProduceOperator(
        task_id="p", topic="orders", producer_function=lambda n, context: range(n), producer_function_args=[2]
    )
    with mock.patch.object(LoamsStreamHook, "produce", return_value=[{"offset": 0}]) as produce:
        assert op.execute(context) == [{"offset": 0}]
    assert list(produce.call_args.args[1]) == [0, 1]


def test_produce_operator_needs_one_source() -> None:
    with pytest.raises(ValueError, match="exactly one"):
        LoamsStreamProduceOperator(task_id="p", topic="t")


def test_consume_operator_with_function_commits_after(loams_connection, context) -> None:
    consumer = FakeConsumer([[FakeMessage(b"a")], []])
    op = LoamsStreamConsumeOperator(task_id="c", topics="orders", group_id="g", apply_function=len)
    with mock.patch.object(LoamsStreamHook, "get_consumer", return_value=consumer):
        assert op.execute(context) == 1
    assert consumer.committed_calls == 1
    assert consumer.closed


def test_consume_operator_function_failure_does_not_commit(loams_connection, context) -> None:
    consumer = FakeConsumer([[FakeMessage(b"a")], []])

    def boom(records):
        raise RuntimeError("bad")

    op = LoamsStreamConsumeOperator(task_id="c", topics="orders", group_id="g", apply_function=boom)
    with (
        mock.patch.object(LoamsStreamHook, "get_consumer", return_value=consumer),
        pytest.raises(RuntimeError),
    ):
        op.execute(context)
    assert consumer.committed_calls == 0
    assert consumer.closed


def test_offset_sensor_poke(loams_connection, context) -> None:
    sensor = LoamsStreamOffsetSensor(task_id="s", topic="orders", group_id="g")
    with mock.patch(
        "airflow.providers.loams.sensors.stream.check_offsets", return_value=(True, {"lag": {"0": 1}})
    ):
        result = sensor.poke(context)
    assert result.is_done and result.xcom_value == {"lag": {"0": 1}}


def test_offset_sensor_validates() -> None:
    with pytest.raises(ValueError, match="group_id"):
        LoamsStreamOffsetSensor(task_id="s", topic="t", mode_="caught_up")
    with pytest.raises(ValueError, match="target_offsets"):
        LoamsStreamOffsetSensor(task_id="s", topic="t", mode_="offset")


def test_offset_sensor_defers(loams_connection, context) -> None:
    sensor = LoamsStreamOffsetSensor(
        task_id="s", topic="orders", group_id="g", deferrable=True, poke_interval=3
    )
    with (
        mock.patch("airflow.providers.loams.sensors.stream.check_offsets", return_value=(False, {})),
        pytest.raises(TaskDeferred) as deferred,
    ):
        sensor.execute(context)
    assert deferred.value.method_name == "execute_complete"
    trigger = deferred.value.trigger
    assert trigger.poll_interval == 3.0
    assert trigger.group_id == "g"
    resume = getattr(sensor, deferred.value.method_name)
    assert resume(context, {"status": "success", "topic": "orders", "lag": {"0": 2}}) == {
        "topic": "orders",
        "lag": {"0": 2},
    }
    with pytest.raises(RuntimeError, match="no topic"):
        resume(context, {"status": "error", "message": "no topic"})


def test_wait_operator_defers_and_completes(context) -> None:
    op = LoamsStreamWaitOperator(task_id="w", topic="orders", mode="offset", target_offsets={0: 5})
    with pytest.raises(TaskDeferred) as deferred:
        op.execute(context)
    assert deferred.value.method_name == "execute_complete"
    assert deferred.value.trigger.target_offsets == {"0": 5}
    resume = getattr(op, deferred.value.method_name)
    assert resume(context, {"status": "success", "watermarks": {}})["status"] == "success"
    with pytest.raises(RuntimeError):
        resume(context, {"status": "error", "message": "x"})
