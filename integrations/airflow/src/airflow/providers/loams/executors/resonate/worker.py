# SPDX-License-Identifier: Apache-2.0
"""The interim worker: claims Airflow tasks from Resonate and runs them (D2094).

Design 74 §9.3 names a Rust ``loams-worker`` as the long-term runner. Until it
lands, this module is the worker: it subscribes to one or more target groups
(``GET /poll/<group>/<pid>``), claims each ``execute`` with ``task.acquire``,
and runs the workload in a child process through the Airflow Task SDK
(:mod:`airflow.providers.loams.executors.resonate.runner`, which calls
``BaseExecutor.run_workload`` and so ``supervise_task``). The task talks to the
Execution API itself, with the workload's short-lived token; this worker never
logs the token or the workload.

Per claimed task:

1. ``task.acquire`` (a ``409`` means another worker won: drop the message).
2. Decode and check the param. A malformed one is settled ``rejected`` /
   ``refused`` so it is never redelivered.
3. If the try's ``started`` marker exists, a previous worker started a process
   for it and then lost its lease (it died). The try is settled ``rejected`` /
   ``lease_lost`` and is not run again; the executor reports it failed and
   Airflow's retry policy makes a new try (D2093).
4. Create the marker, start the process, and heartbeat the lease every
   ``ttl / 3`` while it runs. Every few heartbeats the promise is re-read: if
   someone settled it (the executor revoked it, or it timed out), the process
   is stopped.
5. At the promise's deadline the process gets SIGTERM, then SIGKILL after
   ``kill_grace`` seconds, and the try is settled ``rejected`` / ``timed_out``.
6. Otherwise ``task.fulfill`` resolves the promise with the exit code.

Shutdown: the first SIGTERM/SIGINT stops claiming and lets running tasks
finish; a second one stops the processes (SIGTERM, then SIGKILL) and exits
without settling, so their leases expire and step 3 settles them.
"""

from __future__ import annotations

import argparse
import contextlib
import json
import logging
import os
import signal
import socket
import subprocess
import sys
import tempfile
import threading
import time
import uuid
from collections.abc import Callable
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass, field
from typing import Any, Protocol

from airflow.providers.loams.executors.resonate import ids
from airflow.providers.loams.executors.resonate.client import (
    REJECTED,
    RESOLVED,
    ExecuteMessage,
    HttpResonateClient,
    ResonateClient,
    ResonateError,
    Unavailable,
)
from airflow.providers.loams.executors.resonate.dispatch import (
    KIND_CALLBACK,
    KIND_TASK,
    OUTCOME_EXITED,
    OUTCOME_LEASE_LOST,
    OUTCOME_REFUSED,
    OUTCOME_SPAWN_FAILED,
    OUTCOME_TIMED_OUT,
    SCHEMA_VERSION,
)

logger = logging.getLogger(__name__)

#: Tag on the ``started`` marker promises.
TAG_MARKER = "loams:af.marker"


# --------------------------------------------------------------------------
# Processes
# --------------------------------------------------------------------------


class ProcessHandle(Protocol):
    pid: int

    def poll(self) -> int | None: ...
    def terminate(self) -> None: ...
    def kill(self) -> None: ...


class ProcessRunner(Protocol):
    def start(self, workload: dict[str, Any], server: str, title: str) -> ProcessHandle:
        """Start the task process; raise if it cannot be started."""


@dataclass
class SubprocessRunner:
    """Runs ``python -m airflow.providers.loams.executors.resonate.runner`` for each workload.

    The workload (which carries the task's token) goes through a ``0600``
    temporary file that the child deletes after reading, never through argv
    or the environment, so it does not show up in ``ps``.
    """

    python: str = sys.executable
    workdir: str | None = None
    extra_env: dict[str, str] = field(default_factory=dict)

    def start(self, workload: dict[str, Any], server: str, title: str) -> ProcessHandle:
        fd, path = tempfile.mkstemp(prefix="af-workload-", suffix=".json", dir=self.workdir)
        try:
            with os.fdopen(fd, "w", encoding="utf-8") as fh:
                json.dump(workload, fh)
            env = dict(os.environ)
            env.update(self.extra_env)
            cmd = [
                self.python,
                "-m",
                "airflow.providers.loams.executors.resonate.runner",
                "--json-path",
                path,
                "--delete",
                "--server",
                server,
                "--title",
                title,
            ]
            # A session of its own, so stopping the task reaches the task
            # supervisor's children too.
            return subprocess.Popen(cmd, env=env, start_new_session=True)
        except BaseException:
            with contextlib.suppress(OSError):
                os.unlink(path)
            raise


# --------------------------------------------------------------------------
# The worker
# --------------------------------------------------------------------------


@dataclass
class WorkerSettings:
    url: str = "http://127.0.0.1:8001"
    token: str | None = None
    groups: tuple[str, ...] = ()
    pid: str = ""
    concurrency: int = 4
    #: Lease length; the worker heartbeats every third of it.
    ttl_ms: int = 60_000
    #: Seconds between SIGTERM and SIGKILL when a task is stopped.
    kill_grace: float = 30.0
    #: Overrides the Execution API URL carried in each promise.
    execution_api_url: str = ""
    #: How often a running task's promise is re-read, in heartbeats.
    check_every: int = 2
    #: Main-loop tick while a task runs, seconds.
    tick: float = 0.5
    request_timeout: float = 10.0

    def __post_init__(self) -> None:
        if not self.pid:
            self.pid = f"af-{socket.gethostname()}-{os.getpid()}-{uuid.uuid4().hex[:6]}"
        if self.concurrency < 1:
            msg = "concurrency must be at least 1"
            raise ValueError(msg)
        if self.ttl_ms < 3000:
            msg = "ttl must be at least 3000 ms"
            raise ValueError(msg)
        for group in self.groups:
            ids.validate_group(group)


@dataclass
class Worker:
    client: ResonateClient
    settings: WorkerSettings
    runner: ProcessRunner = field(default_factory=SubprocessRunner)
    clock: Callable[[], float] = time.time
    sleep: Callable[[float], None] = time.sleep
    _draining: threading.Event = field(default_factory=threading.Event)
    _stopping: threading.Event = field(default_factory=threading.Event)
    _slots: threading.Semaphore | None = None
    _running: dict[str, ProcessHandle] = field(default_factory=dict)
    _lock: threading.Lock = field(default_factory=threading.Lock)

    def __post_init__(self) -> None:
        self._slots = threading.BoundedSemaphore(self.settings.concurrency)

    # -- one task -----------------------------------------------------------

    def handle(self, msg: ExecuteMessage) -> str:
        """Claim and run one task. Returns what happened, for logs and tests."""
        ttl = self.settings.ttl_ms
        try:
            acquired = self.client.task_acquire(msg.task_id, msg.version, self.settings.pid, ttl)
        except (Unavailable, ResonateError) as exc:
            # Nothing was claimed, or the claim will lapse: either way the
            # server redelivers.
            logger.warning("task.acquire %s failed: %s", msg.task_id, exc)
            return "acquire_failed"
        if acquired is None:
            return "lost"
        task_id = acquired.task.id
        version = acquired.task.version
        promise = acquired.promise

        try:
            param = promise.param_json()
        except ValueError:
            param = None
        problem = _check_param(param)
        if problem:
            self._settle(task_id, version, REJECTED, OUTCOME_REFUSED, reason=problem)
            return "refused"

        marker_id = ids.started_marker_id(promise.id)
        try:
            marker = self.client.promise_get(marker_id)
        except (Unavailable, ResonateError) as exc:
            logger.warning("cannot read the started marker of %s: %s", task_id, exc)
            self._release(task_id, version)
            return "released"
        if marker is not None:
            started_by = (marker.param_json() or {}).get("worker", "another worker")
            reason = (
                f"this try was started by {started_by} whose lease expired "
                "(the worker died); it is not run twice"
            )
            self._settle(task_id, version, REJECTED, OUTCOME_LEASE_LOST, reason=reason)
            return "lease_lost"

        now_ms = int(self.clock() * 1000)
        if promise.timeout_at and promise.timeout_at <= now_ms:
            self._settle(task_id, version, REJECTED, OUTCOME_TIMED_OUT, reason="expired unclaimed")
            return "timed_out"

        worker_name = f"{socket.gethostname()}/{self.settings.pid}"
        try:
            self.client.promise_create(
                marker_id,
                promise.timeout_at or now_ms + 24 * 3600 * 1000,
                {"v": SCHEMA_VERSION, "worker": worker_name, "version": version, "at": now_ms},
                {TAG_MARKER: "started"},
            )
        except (Unavailable, ResonateError) as exc:
            logger.warning("cannot write the started marker of %s: %s", task_id, exc)
            self._release(task_id, version)
            return "released"

        server = self.settings.execution_api_url or str(param.get("execution_api_url") or "")
        title = str(param.get("display_name") or task_id)
        try:
            proc = self.runner.start(param["workload"], server, title)
        except Exception as exc:
            logger.exception("cannot start the process for %s", task_id)
            self._settle(task_id, version, REJECTED, OUTCOME_SPAWN_FAILED, reason=str(exc))
            return "spawn_failed"

        with self._lock:
            self._running[task_id] = proc
        try:
            return self._supervise(task_id, version, promise.timeout_at, proc)
        finally:
            with self._lock:
                self._running.pop(task_id, None)

    def _supervise(self, task_id: str, version: int, deadline_ms: int, proc: ProcessHandle) -> str:
        period = self.settings.ttl_ms / 3000.0
        next_beat = self.clock() + period
        beats = 0
        while True:
            rc = proc.poll()
            if rc is not None:
                ok = self._settle(task_id, version, RESOLVED, OUTCOME_EXITED, returncode=rc)
                return "exited" if ok else "exited_unsettled"
            now = self.clock()
            if self._stopping.is_set():
                # Second signal: stop the process and leave the lease to expire.
                self._stop_process(proc)
                return "abandoned"
            if deadline_ms and now * 1000 >= deadline_ms:
                self._stop_process(proc)
                self._settle(task_id, version, REJECTED, OUTCOME_TIMED_OUT, reason="task_timeout reached")
                return "timed_out"
            if now >= next_beat:
                next_beat = now + period
                beats += 1
                try:
                    self.client.task_heartbeat(self.settings.pid, task_id, version)
                except (Unavailable, ResonateError) as exc:
                    logger.warning("heartbeat of %s failed: %s", task_id, exc)
                if beats % max(1, self.settings.check_every) == 0 and self._settled_elsewhere(task_id):
                    self._stop_process(proc)
                    return "cancelled"
            self.sleep(self.settings.tick)

    def _settled_elsewhere(self, task_id: str) -> bool:
        try:
            record = self.client.promise_get(task_id)
        except (Unavailable, ResonateError) as exc:
            logger.warning("cannot re-read %s: %s", task_id, exc)
            return False
        return record is None or record.settled

    def _stop_process(self, proc: ProcessHandle) -> None:
        if proc.poll() is not None:
            return
        with contextlib.suppress(ProcessLookupError, OSError):
            proc.terminate()
        deadline = self.clock() + self.settings.kill_grace
        while self.clock() < deadline:
            if proc.poll() is not None:
                return
            self.sleep(min(self.settings.tick, 0.2))
        with contextlib.suppress(ProcessLookupError, OSError):
            proc.kill()

    def _settle(
        self,
        task_id: str,
        version: int,
        state: str,
        outcome: str,
        *,
        reason: str = "",
        returncode: int | None = None,
    ) -> bool:
        value: dict[str, Any] = {
            "v": SCHEMA_VERSION,
            "outcome": outcome,
            "worker": self.settings.pid,
        }
        if reason:
            value["reason"] = reason
        if returncode is not None:
            value["returncode"] = returncode
        delay = 0.5
        for attempt in range(5):
            try:
                ok = self.client.task_fulfill(task_id, version, state, value)
            except Unavailable as exc:
                # The fulfill may have been applied; a retry then answers 409,
                # which is fine.
                logger.warning("task.fulfill %s failed (attempt %d): %s", task_id, attempt + 1, exc)
                self.sleep(delay)
                delay = min(delay * 2, 8.0)
                continue
            except ResonateError as exc:
                logger.error("task.fulfill %s refused: %s", task_id, exc)
                return False
            if not ok:
                logger.info("task.fulfill %s: lease lost or already settled", task_id)
            return ok
        return False

    def _release(self, task_id: str, version: int) -> None:
        try:
            self.client.task_release(task_id, version)
        except (Unavailable, ResonateError) as exc:
            logger.warning("task.release %s failed (the lease will lapse): %s", task_id, exc)

    # -- the loop -----------------------------------------------------------

    def offer(self, msg: ExecuteMessage, pool: ThreadPoolExecutor) -> bool:
        """Run ``msg`` if a slot is free; otherwise leave it for redelivery."""
        if self._draining.is_set():
            return False
        assert self._slots is not None
        if not self._slots.acquire(blocking=False):
            return False

        def run() -> None:
            try:
                result = self.handle(msg)
                logger.info("task %s: %s", msg.task_id, result)
            except Exception:
                logger.exception("task %s: worker error", msg.task_id)
            finally:
                assert self._slots is not None
                self._slots.release()

        pool.submit(run)
        return True

    def _listen(self, group: str, pool: ThreadPoolExecutor) -> None:
        backoff = 1.0
        while not self._draining.is_set():
            try:
                for msg in self.client.poll(group, self.settings.pid, self._draining.is_set):
                    backoff = 1.0
                    self.offer(msg, pool)
            except Unavailable as exc:
                logger.warning("poll %s: %s (retrying in %.0fs)", group, exc, backoff)
            if self._draining.is_set():
                return
            self.sleep(backoff)
            backoff = min(backoff * 2, 30.0)

    def request_stop(self) -> None:
        if self._draining.is_set():
            logger.warning("second stop signal: stopping running tasks")
            self._stopping.set()
        else:
            logger.info("stop signal: no new tasks; waiting for running ones")
            self._draining.set()

    def run(self) -> None:
        if not self.settings.groups:
            msg = "no target groups to poll"
            raise ValueError(msg)
        pool = ThreadPoolExecutor(max_workers=self.settings.concurrency, thread_name_prefix="af-task")
        listeners = [
            threading.Thread(target=self._listen, args=(g, pool), name=f"af-poll-{g}", daemon=True)
            for g in self.settings.groups
        ]
        for t in listeners:
            t.start()
        logger.info(
            "worker %s polling %s (concurrency %d)",
            self.settings.pid,
            ", ".join(self.settings.groups),
            self.settings.concurrency,
        )
        try:
            while not self._draining.is_set():
                self.sleep(0.5)
        finally:
            pool.shutdown(wait=True)
            self.client.close()


def _check_param(param: Any) -> str:
    """Empty when ``param`` is a workload this worker can run, else why not."""
    if not isinstance(param, dict):
        return "the promise param is not a JSON object"
    if param.get("v") != SCHEMA_VERSION:
        return f"unsupported param schema {param.get('v')!r}"
    if param.get("kind") not in (KIND_TASK, KIND_CALLBACK):
        return f"unsupported workload kind {param.get('kind')!r}"
    if not isinstance(param.get("workload"), dict):
        return "the param carries no workload"
    return ""


# --------------------------------------------------------------------------
# CLI
# --------------------------------------------------------------------------


def _parser() -> argparse.ArgumentParser:
    env = os.environ.get
    p = argparse.ArgumentParser(
        prog="loams-airflow-resonate-worker",
        description="Run Airflow tasks that the Resonate executor submitted (preview).",
    )
    p.add_argument("--url", default=env("LOAMS_RESONATE_URL", "http://127.0.0.1:8001"))
    p.add_argument(
        "--token-env",
        default="LOAMS_RESONATE_TOKEN",
        help="environment variable holding the Resonate bearer token, if any",
    )
    p.add_argument("--namespace", default=env("LOAMS_RESONATE_NAMESPACE", "default"))
    p.add_argument("--team", default=env("LOAMS_RESONATE_TEAM") or None)
    p.add_argument(
        "--queues",
        default=env("LOAMS_RESONATE_QUEUES", "default"),
        help="comma-separated Airflow queues; each becomes af.<ns>[.<team>].<queue>",
    )
    p.add_argument(
        "--groups",
        default=env("LOAMS_RESONATE_GROUPS", ""),
        help="comma-separated target groups, instead of --queues",
    )
    p.add_argument("--pid", default=env("LOAMS_RESONATE_PID", ""))
    p.add_argument("--concurrency", type=int, default=int(env("LOAMS_RESONATE_CONCURRENCY", "4")))
    p.add_argument("--ttl-ms", type=int, default=int(env("LOAMS_RESONATE_TTL_MS", "60000")))
    p.add_argument("--kill-grace", type=float, default=float(env("LOAMS_KILL_GRACE", "30")))
    p.add_argument(
        "--execution-api-url",
        default=env("LOAMS_RESONATE_EXECUTION_API_URL", ""),
        help="override the Execution API URL the executor put in each promise",
    )
    p.add_argument("--log-level", default=env("LOAMS_RESONATE_LOG_LEVEL", "INFO"))
    return p


def groups_from_args(args: argparse.Namespace) -> tuple[str, ...]:
    if args.groups.strip():
        return tuple(g.strip() for g in args.groups.split(",") if g.strip())
    queues = [q.strip() for q in args.queues.split(",") if q.strip()]
    return tuple(ids.target_group(args.namespace, q, args.team) for q in queues)


def main(argv: list[str] | None = None) -> int:
    args = _parser().parse_args(argv)
    logging.basicConfig(
        level=args.log_level.upper(),
        format="%(asctime)s %(levelname)s %(name)s %(message)s",
    )
    settings = WorkerSettings(
        url=args.url,
        token=os.environ.get(args.token_env) or None,
        groups=groups_from_args(args),
        pid=args.pid,
        concurrency=args.concurrency,
        ttl_ms=args.ttl_ms,
        kill_grace=args.kill_grace,
        execution_api_url=args.execution_api_url,
    )
    client = HttpResonateClient(settings.url, token=settings.token, timeout=settings.request_timeout)
    worker = Worker(client=client, settings=settings)
    for sig in (signal.SIGTERM, signal.SIGINT):
        signal.signal(sig, lambda *_: worker.request_stop())
    worker.run()
    return 0


if __name__ == "__main__":
    sys.exit(main())
