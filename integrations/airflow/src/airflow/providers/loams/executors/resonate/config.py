# SPDX-License-Identifier: Apache-2.0
"""Settings for the executor, read from Airflow's ``[loams_resonate]`` section.

Every key can be set the usual Airflow way, for example
``AIRFLOW__LOAMS_RESONATE__URL``. With Airflow's multi-team mode the executor
reads through its ``ExecutorConf``, so a team can override any key
(``AIRFLOW__<TEAM>___LOAMS_RESONATE__URL``, Airflow's team-variable form).

The loader takes a ``get(section, key, fallback)`` callable rather than
Airflow's ``conf`` object so it can be tested without Airflow.
"""

from __future__ import annotations

import json
from collections.abc import Callable
from dataclasses import dataclass, field
from typing import Any

SECTION = "loams_resonate"

Getter = Callable[[str, str, Any], Any]


@dataclass(frozen=True)
class ExecutorSettings:
    #: Base URL of the Resonate server. In the Loams compose stack the engine's
    #: durable listener is ``http://127.0.0.1:8001`` in the engine's network
    #: namespace; behind Loams auth it is ``https://<engine>/durable``.
    url: str = "http://127.0.0.1:8001"
    #: Bearer token for Resonate, if the server requires one.
    token: str | None = None
    #: The Loams namespace: the second label of every id and group.
    namespace: str = "default"
    #: Seconds a submitted task may wait for a worker to claim it before the
    #: executor cancels it and reports it failed. Keep it at or below
    #: ``[scheduler] task_queued_timeout``: the workload's token is minted for
    #: that long.
    claim_timeout: float = 300.0
    #: Seconds a claimed task may run before its promise times out (the
    #: worker kills the process at that deadline).
    task_timeout: float = 24 * 3600.0
    #: Seconds between two state polls of the in-flight promises.
    sync_interval: float = 5.0
    #: At most this many promises are read per poll (round robin).
    sync_batch_size: int = 256
    #: Per-request HTTP timeout, seconds.
    request_timeout: float = 10.0
    #: Times a workload is offered to Resonate before it is reported failed.
    submit_max_attempts: int = 5
    #: Queue → target group overrides, e.g. ``{"gpu": "af.default.gpu-pool"}``.
    queue_groups: dict[str, str] = field(default_factory=dict)
    #: Queue → maximum in-flight workloads in this executor (``"*"`` = all).
    queue_limits: dict[str, int] = field(default_factory=dict)
    #: Cancel every in-flight task when the scheduler terminates. Off by
    #: default so a restarted scheduler adopts them instead (D2095).
    cancel_on_terminate: bool = False
    #: The Execution API URL handed to workers in each promise. Empty: the
    #: executor's own ``[core] execution_api_server_url``.
    execution_api_url: str = ""

    @classmethod
    def load(cls, get: Getter) -> ExecutorSettings:
        def text(key: str, default: str) -> str:
            value = get(SECTION, key, default)
            return default if value is None else str(value)

        def number(key: str, default: float) -> float:
            raw = text(key, str(default))
            value = float(raw)
            if value <= 0:
                msg = f"[{SECTION}] {key} must be positive, got {raw!r}"
                raise ValueError(msg)
            return value

        def mapping(key: str) -> dict[str, Any]:
            raw = text(key, "").strip()
            if not raw:
                return {}
            value = json.loads(raw)
            if not isinstance(value, dict):
                msg = f"[{SECTION}] {key} must be a JSON object"
                raise ValueError(msg)
            return value

        limits = {str(k): int(v) for k, v in mapping("queue_limits").items()}
        for queue, limit in limits.items():
            if limit < 1:
                msg = f"[{SECTION}] queue_limits[{queue!r}] must be at least 1"
                raise ValueError(msg)
        token = text("token", "").strip() or None
        return cls(
            url=text("url", cls.url).rstrip("/"),
            token=token,
            namespace=text("namespace", cls.namespace) or cls.namespace,
            claim_timeout=number("claim_timeout", cls.claim_timeout),
            task_timeout=number("task_timeout", cls.task_timeout),
            sync_interval=number("sync_interval", cls.sync_interval),
            sync_batch_size=int(number("sync_batch_size", cls.sync_batch_size)),
            request_timeout=number("request_timeout", cls.request_timeout),
            submit_max_attempts=int(number("submit_max_attempts", cls.submit_max_attempts)),
            queue_groups={str(k): str(v) for k, v in mapping("queue_groups").items()},
            queue_limits=limits,
            cancel_on_terminate=text("cancel_on_terminate", "false").strip().lower()
            in ("1", "true", "yes", "on"),
            execution_api_url=text("execution_api_url", ""),
        )
