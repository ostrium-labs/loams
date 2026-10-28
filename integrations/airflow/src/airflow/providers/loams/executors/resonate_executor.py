# SPDX-License-Identifier: Apache-2.0
"""``LoamsResonateExecutor``: an Airflow 3 executor on Resonate durable promises.

Configure it with::

    [core]
    executor = airflow.providers.loams.executors.resonate_executor.LoamsResonateExecutor

    [loams_resonate]
    url = http://127.0.0.1:8001
    namespace = default

and run ``loams-airflow-resonate-worker --queues default`` where tasks should
run. Each task instance try becomes one durable promise whose id is derived
from ``dag_id``, ``run_id``, ``task_id``, ``map_index`` and ``try_number``
(:func:`airflow.providers.loams.executors.resonate.ids.task_promise_id`), targeted at the
Resonate group of its queue. Workers claim it, run it through the Task SDK
against the Execution API, and settle the promise; ``sync`` reads the promises
back and reports infrastructure outcomes (:mod:`airflow.providers.loams.executors.resonate.dispatch`).

Design 74 §9.3 (D1525) and this lane's decisions D2090 to D2095. Preview: the
unit tests run against a fake Resonate; it has not run end to end yet.
"""

from __future__ import annotations

import time
from collections.abc import Callable, Sequence
from typing import TYPE_CHECKING, Any

from airflow.configuration import conf as airflow_conf
from airflow.executors import workloads
from airflow.executors.base_executor import BaseExecutor
from airflow.utils.state import TaskInstanceState

from airflow.providers.loams.executors.resonate import ids
from airflow.providers.loams.executors.resonate.client import HttpResonateClient, ResonateClient
from airflow.providers.loams.executors.resonate.config import ExecutorSettings
from airflow.providers.loams.executors.resonate.dispatch import (
    KIND_CALLBACK,
    KIND_TASK,
    SCHEMA_VERSION,
    TAG_DAG,
    TAG_KIND,
    TAG_NAMESPACE,
    TAG_QUEUE,
    TAG_RUN,
    TAG_TARGET,
    TAG_TASK,
    TAG_TEAM,
    TAG_TI,
    Dispatcher,
    Submission,
)

if TYPE_CHECKING:
    from airflow.models.taskinstance import TaskInstance

#: ``executor_config`` key for per-task overrides:
#: ``{"loams_resonate": {"group": "af.default.gpu", "timeout": 3600}}``.
EXECUTOR_CONFIG_KEY = "loams_resonate"


def _state_class(key: Any) -> Any:
    try:
        from airflow.executors.workloads.types import state_class_for_key

        return state_class_for_key(key)
    except ImportError:  # pragma: no cover - older 3.x layouts
        return TaskInstanceState


class LoamsResonateExecutor(BaseExecutor):
    """Run Airflow task instances as Resonate durable promises."""

    supports_ad_hoc_ti_run: bool = True
    supports_multi_team: bool = True
    supports_callbacks: bool = True
    supports_connection_test: bool = False
    is_local: bool = False
    is_production: bool = True
    serve_logs: bool = False

    #: Builds the Resonate client; tests replace it with a fake.
    client_factory: Callable[[ExecutorSettings], ResonateClient] | None = None
    #: The clock the dispatcher uses; tests replace it.
    clock: Callable[[], float] = staticmethod(time.time)

    def __init__(self, *args: Any, **kwargs: Any) -> None:
        super().__init__(*args, **kwargs)
        if not hasattr(self, "conf"):  # pragma: no cover - pre multi-team Airflow
            self.conf = airflow_conf
        self.settings = ExecutorSettings.load(self._conf_get)
        self._dispatch: Dispatcher | None = None

    # -- configuration ------------------------------------------------------

    def _conf_get(self, section: str, key: str, fallback: Any) -> Any:
        return self.conf.get(section, key, fallback=fallback)

    @property
    def team(self) -> str | None:
        return getattr(self, "team_name", None) or None

    def _execution_api_url(self) -> str:
        if self.settings.execution_api_url:
            return self.settings.execution_api_url
        try:
            from airflow.executors.base_executor import get_execution_api_server_url

            return get_execution_api_server_url(self.conf)
        except ImportError:  # pragma: no cover - older 3.x
            return str(
                self.conf.get("core", "execution_api_server_url", fallback="")
                or "http://localhost:8080/execution/"
            )

    # -- lifecycle ----------------------------------------------------------

    def start(self) -> None:
        self._ensure_dispatch()
        self.log.info(
            "LoamsResonateExecutor started: %s, namespace %r%s",
            self.settings.url,
            self.settings.namespace,
            f", team {self.team!r}" if self.team else "",
        )

    def _ensure_dispatch(self) -> Dispatcher:
        if self._dispatch is None:
            factory = type(self).client_factory
            if factory is not None:
                client = factory(self.settings)
            else:
                client = HttpResonateClient(
                    self.settings.url,
                    token=self.settings.token,
                    timeout=self.settings.request_timeout,
                )
            self._dispatch = Dispatcher(client=client, settings=self.settings, clock=self.clock)
        return self._dispatch

    @property
    def dispatcher(self) -> Dispatcher:
        return self._ensure_dispatch()

    def end(self) -> None:
        """Report what is already known, then let go; in-flight work stays adoptable."""
        if self._dispatch is None:
            return
        self._apply(self._dispatch.poll(force=True))
        self._dispatch.client.close()
        self._dispatch = None

    def terminate(self) -> None:
        if self._dispatch is None:
            return
        if self.settings.cancel_on_terminate:
            self._dispatch.cancel_all("the scheduler was terminated")
        self._dispatch.client.close()
        self._dispatch = None

    # -- submit -------------------------------------------------------------

    def _queue_of(self, workload: Any) -> str:
        ti = getattr(workload, "ti", None)
        queue = getattr(ti, "queue", None) if ti is not None else None
        return str(queue or "default")

    def _overrides(self, workload: Any) -> dict[str, Any]:
        ti = getattr(workload, "ti", None)
        cfg = getattr(ti, "executor_config", None) if ti is not None else None
        if isinstance(cfg, dict) and isinstance(cfg.get(EXECUTOR_CONFIG_KEY), dict):
            return cfg[EXECUTOR_CONFIG_KEY]
        return {}

    def build_submission(self, workload: Any) -> Submission:
        """The promise for one workload: id, target, param and tags."""
        ns = self.settings.namespace
        queue = self._queue_of(workload)
        overrides = self._overrides(workload)
        tags: dict[str, str] = {TAG_NAMESPACE: ns, TAG_QUEUE: queue}
        if self.team:
            tags[TAG_TEAM] = self.team
        if isinstance(workload, workloads.ExecuteTask):
            ti = workload.ti
            ref = ids.TaskRef(
                dag_id=ti.dag_id,
                run_id=ti.run_id,
                task_id=ti.task_id,
                try_number=ti.try_number,
                map_index=ti.map_index if ti.map_index is not None else -1,
            )
            promise_id = ids.task_promise_id(ns, ref)
            kind = KIND_TASK
            tags.update({TAG_DAG: ti.dag_id, TAG_RUN: ti.run_id, TAG_TASK: ti.task_id, TAG_TI: str(ti.id)})
        elif isinstance(workload, workloads.ExecuteCallback):
            promise_id = ids.callback_promise_id(ns, str(workload.callback.id))
            kind = KIND_CALLBACK
        else:
            msg = f"LoamsResonateExecutor cannot run {type(workload).__name__} workloads"
            raise ValueError(msg)
        tags[TAG_KIND] = kind

        group = str(
            overrides.get("group")
            or self.settings.queue_groups.get(queue)
            or ids.target_group(ns, queue, self.team)
        )
        ids.validate_group(group)
        tags[TAG_TARGET] = ids.anycast_address(group)

        timeout = float(overrides.get("timeout") or self.settings.task_timeout)
        now_ms = int(self.clock() * 1000)
        timeout_at_ms = now_ms + int((self.settings.claim_timeout + timeout) * 1000)
        param = {
            "v": SCHEMA_VERSION,
            "kind": kind,
            "workload": workload.model_dump(mode="json"),
            "execution_api_url": self._execution_api_url(),
            "display_name": workload.display_name,
            "queue": queue,
            "team": self.team,
        }
        return Submission(
            key=workload.key,
            promise_id=promise_id,
            queue=queue,
            group=group,
            param=param,
            tags=tags,
            timeout_at_ms=timeout_at_ms,
        )

    def _process_workloads(self, workload_items: Sequence[Any]) -> None:
        dispatch = self._ensure_dispatch()
        for workload in workload_items:
            key = workload.key
            if not dispatch.has_capacity(self._queue_of(workload)):
                continue  # stays queued until a slot of its queue frees up
            try:
                submission = self.build_submission(workload)
            except ValueError as exc:
                self._unqueue(key)
                self.log.error("cannot submit %s: %s", key, exc)
                self.fail(key, str(exc))
                continue
            result = dispatch.submit(submission)
            if result.status == "retry":
                continue
            self._unqueue(key)
            if result.status == "failed":
                self.log.error("giving up on %s: %s", key, result.reason)
                self.fail(key, result.reason)
                continue
            self.running.add(key)
            # QUEUED with the promise id: the scheduler records it as the
            # try's external_executor_id, which adoption reads back.
            self.change_state(key, _state_class(key).QUEUED, submission.promise_id, remove_running=False)

    def _unqueue(self, key: Any) -> None:
        self.queued_tasks.pop(key, None)
        self.queued_callbacks.pop(key, None)

    # -- sync ---------------------------------------------------------------

    def sync(self) -> None:
        if self._dispatch is None:
            return
        self._apply(self._dispatch.poll())

    def _apply(self, outcomes: Sequence[Any]) -> None:
        for outcome in outcomes:
            if outcome.state == "running":
                self.running_state(outcome.key, outcome.info)
            elif outcome.state == "success":
                self.success(outcome.key, outcome.info)
            else:
                self.log.warning("%s failed in Resonate: %s", outcome.key, outcome.info)
                self.fail(outcome.key, outcome.info)

    # -- adoption and revocation --------------------------------------------

    def promise_id_for(self, ti: TaskInstance) -> str:
        external = getattr(ti, "external_executor_id", None)
        if isinstance(external, str) and external.startswith(f"{ids.PREFIX}."):
            return external
        return ids.task_promise_id(
            self.settings.namespace,
            ids.TaskRef(
                dag_id=ti.dag_id,
                run_id=ti.run_id,
                task_id=ti.task_id,
                try_number=ti.try_number,
                map_index=ti.map_index if ti.map_index is not None else -1,
            ),
        )

    def try_adopt_task_instances(self, tis: Sequence[TaskInstance]) -> Sequence[TaskInstance]:
        """Adopt tries whose promise exists; the rest go back to the scheduler."""
        dispatch = self._ensure_dispatch()
        not_adopted: list[TaskInstance] = []
        for ti in tis:
            promise_id = self.promise_id_for(ti)
            if dispatch.adopt(ti.key, promise_id, str(ti.queue or "default")):
                self.running.add(ti.key)
                self.log.info("adopted %s (%s)", ti.key, promise_id)
            else:
                not_adopted.append(ti)
        return not_adopted

    def revoke_task(self, *, ti: TaskInstance) -> None:
        """Cancel a try's promise and forget it. Never raises, adds no events."""
        key = ti.key
        try:
            self._unqueue(key)
            self.running.discard(key)
            dispatch = self._ensure_dispatch()
            if not dispatch.revoke(key):
                # Not tracked here (another scheduler's try): cancel by id.
                dispatch.cancel(self.promise_id_for(ti), "revoked", "revoked by the scheduler")
        except Exception:
            self.log.exception("revoke of %s failed", key)

    def debug_dump(self) -> None:
        super().debug_dump()
        if self._dispatch is not None:
            self.log.info("resonate in-flight (%d)", self._dispatch.in_flight())


#: The short name, for configuration written against the AF-exec preview.
ResonateExecutor = LoamsResonateExecutor
