# SPDX-License-Identifier: Apache-2.0
"""`LoamsStreamOffsetSensor`: waits until a stream's offsets reach a point."""

from __future__ import annotations

from collections.abc import Mapping, Sequence
from datetime import timedelta
from typing import TYPE_CHECKING, Any

from airflow.sdk import BaseSensorOperator, PokeReturnValue

from airflow.providers.loams.hooks.stream import LoamsStreamHook
from airflow.providers.loams.triggers.stream import OFFSET_MODES, LoamsStreamOffsetTrigger, check_offsets

if TYPE_CHECKING:
    from airflow.sdk import Context

__all__ = ["LoamsStreamOffsetSensor"]


class LoamsStreamOffsetSensor(BaseSensorOperator):
    """Pokes a topic's offsets (see `check_offsets` for the modes).

    - `mode_="new_records"` with `group_id`: data the group has not read yet;
    - `mode_="caught_up"` with `group_id`: the group has read everything;
    - `mode_="offset"` with `target_offsets`: the stream got this far.

    (`mode_`, because `mode` is the sensor's `poke`/`reschedule` setting.)
    The offsets seen go to XCom.
    """

    template_fields: Sequence[str] = ("topic", "group_id", "target_offsets")

    def __init__(
        self,
        *,
        topic: str,
        mode_: str = "new_records",
        target_offsets: Mapping[Any, int] | None = None,
        group_id: str | None = None,
        partitions: Sequence[int] | None = None,
        min_records: int = 1,
        loams_conn_id: str = LoamsStreamHook.default_conn_name,
        deferrable: bool = False,
        **kwargs: Any,
    ) -> None:
        super().__init__(**kwargs)
        if mode_ not in OFFSET_MODES:
            raise ValueError(f"mode_ must be one of {OFFSET_MODES}")
        if mode_ == "offset" and not target_offsets:
            raise ValueError("mode_='offset' needs target_offsets")
        if mode_ != "offset" and not group_id:
            raise ValueError(f"mode_={mode_!r} needs group_id")
        self.topic = topic
        self.mode_ = mode_
        self.target_offsets = dict(target_offsets or {})
        self.group_id = group_id
        self.partitions = list(partitions) if partitions is not None else None
        self.min_records = min_records
        self.loams_conn_id = loams_conn_id
        self.deferrable = deferrable

    def poke(self, context: Context) -> PokeReturnValue:
        done, state = check_offsets(
            LoamsStreamHook(loams_conn_id=self.loams_conn_id),
            topic=self.topic,
            mode=self.mode_,
            target_offsets=self.target_offsets,
            group_id=self.group_id,
            partitions=self.partitions,
            min_records=self.min_records,
        )
        self.log.info("Stream %s (%s): %s", self.topic, self.mode_, state)
        return PokeReturnValue(is_done=done, xcom_value=state)

    def execute(self, context: Context) -> Any:
        if not self.deferrable:
            return super().execute(context)
        first = self.poke(context)
        if first.is_done:
            return first.xcom_value
        self.defer(
            trigger=LoamsStreamOffsetTrigger(
                topic=self.topic,
                mode=self.mode_,
                target_offsets=self.target_offsets,
                group_id=self.group_id,
                partitions=self.partitions,
                min_records=self.min_records,
                loams_conn_id=self.loams_conn_id,
                poll_interval=float(self.poke_interval),
            ),
            method_name="execute_complete",
            timeout=timedelta(seconds=float(self.timeout)) if self.timeout else None,
        )
        return None

    def execute_complete(self, context: Context, event: dict[str, Any]) -> dict[str, Any]:
        if event.get("status") != "success":
            raise RuntimeError(f"waiting on stream {self.topic} failed: {event.get('message')}")
        return {k: v for k, v in event.items() if k not in ("status",)}
