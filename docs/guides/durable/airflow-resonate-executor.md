# Airflow on Resonate: `LoamsResonateExecutor` (preview)

Status: **preview**. The code has unit tests against a fake Resonate and real Airflow 3.3.2 workload models. It has **not yet run end to end** against a Loams engine. The Edge executor stack (DX0, compose profile `airflow`) stays the supported baseline and fallback (D1524).

Design: [§74 §9.3](../../design/74-durable-execution-compat.md) (D1525). This lane's decisions are D2090–D2096 in the [decision log](../../design/13-decision-log.md). The executor is part of the provider `airflow-provider-loams` ([integrations/airflow](../../../integrations/airflow/README.md)): `airflow.providers.loams.executors.resonate_executor`, with its Airflow-free core, protocol client and worker under `airflow.providers.loams.executors.resonate`. The provider registers it under `executors:`.

## What it does

Each Airflow task instance try becomes one Resonate durable promise:

```
scheduler ── ExecuteTask ──▶ LoamsResonateExecutor  promise.create ──▶ Resonate
                                   ▲                                  │ execute (poll://any@<group>)
                                   │ promise.get (sync)               ▼
                                   └────────────────────────── loams-airflow-resonate-worker
                                                                      │ task.acquire / heartbeat / fulfill
                                                                      ▼
                                                     Task SDK supervisor ── Execution API (the task's own state)
```

- **Deterministic ids (D2091).** `af.<ns>.<dag_id>.<run_id>:<task_id>.<map_index>.<try_number>`. Every component is percent-escaped outside `[A-Za-z0-9_-]`. The origin, the part before `:`, is one Dag run. Ids over 255 bytes use a hashed form. A restarted scheduler finds a try by id alone.
- **Routing (D2092).** An Airflow queue maps to the Resonate target group `af.<ns>.<queue>`, or `af.<ns>.<team>.<queue>` for a team's executor. The promise's `resonate:target` is `poll://any@<group>`. `[loams_resonate] queue_groups` and a task's `executor_config={"loams_resonate": {"group": ..., "timeout": ...}}` override the routing. `queue_limits` caps in-flight work per queue. Pools stay the scheduler's job.
- **Who reports what (D2093).** The task reports its own state through the Execution API. The executor reports only infrastructure outcomes:

  | Outcome | Executor event |
  |---|---|
  | A worker claimed the try | `RUNNING` (info: the worker) |
  | The process exited 0 / non-zero | `SUCCESS` / `FAILED` |
  | No claim within `claim_timeout` | the promise is cancelled first, then `FAILED` |
  | The worker died (lease expired after the process started) | `FAILED` (`lease_lost`); the try is never run twice |
  | Spawn failure, or a malformed workload | `FAILED` |
  | The promise timed out (`claim_timeout + task_timeout`) | `FAILED` |

  Airflow's retry policy then makes a new try, which gets a new promise.
- **The `started` marker.** Before it starts a process, a worker creates `<id>.started`. If Resonate redelivers a try whose marker exists, the previous worker died after starting it. The new worker settles the try `lease_lost` instead of running it again. A claim that lapsed *before* the start is redelivered and runs normally.
- **Adoption (D2095).** `try_adopt_task_instances` reads each orphaned try's promise by id (or by the `external_executor_id` the executor recorded, which is the promise id). An existing promise is adopted. A missing one goes back to the scheduler. When the scheduler terminates, in-flight promises are left alone so that the next scheduler can adopt them. `cancel_on_terminate = true` changes that.
- **Revocation.** `revoke_task` halts the task and settles the promise `rejected_canceled`. The worker notices within two heartbeats and stops the process with SIGTERM, then SIGKILL after `--kill-grace`.
- **Multi-team.** `supports_multi_team = True`. Settings are read through the executor's team configuration, and the team goes into the group and the `loams:af.team` tag.

## Configuration

```ini
[core]
executor = airflow.providers.loams.executors.resonate_executor.LoamsResonateExecutor

[loams_resonate]
url = http://127.0.0.1:8001        # the engine's durable listener, or any Resonate server
namespace = default
claim_timeout = 300                # keep <= [scheduler] task_queued_timeout (the token's lifetime)
task_timeout = 86400
sync_interval = 5
sync_batch_size = 256
submit_max_attempts = 5
queue_groups = {"gpu": "af.default.gpu-pool"}
queue_limits = {"gpu": 4}
cancel_on_terminate = false
# token = ...                      # if the Resonate server requires a bearer token
# execution_api_url = ...          # defaults to [core] execution_api_server_url
```

`ResonateExecutor` is an alias of `LoamsResonateExecutor`.

## Workers

```
loams-airflow-resonate-worker --url http://127.0.0.1:8001 --queues default,gpu --concurrency 8
```

A worker needs the same Airflow version as the scheduler, with the Dag bundle configuration but no database. It needs `airflow-provider-loams` installed, which ships the `loams-airflow-resonate-worker` script. It runs each workload in a child process (`python -m airflow.providers.loams.executors.resonate.runner`), which calls `BaseExecutor.run_workload` and so `supervise_task`. The workload's token reaches the child through a `0600` file that the child deletes after reading. The worker never logs it. Flags can also be set from the environment: `LOAMS_RESONATE_URL`, `LOAMS_RESONATE_QUEUES`, `LOAMS_RESONATE_GROUPS`, `LOAMS_RESONATE_TEAM`, `LOAMS_RESONATE_CONCURRENCY`, `LOAMS_RESONATE_TTL_MS`, `LOAMS_KILL_GRACE` and `LOAMS_RESONATE_TOKEN`.

Shutdown works in two steps. The first SIGTERM stops claiming new tries and lets the running ones finish. A second SIGTERM stops the running processes and exits without settling them, so they surface as `lease_lost`.

This Python worker is interim (D2094). Design 74 names a Rust `crates/loams-worker` that runs tasks in `loams-sandbox` and reports cgroup CPU time. Until that lands, tasks run as plain child processes with no sandbox and no metering.

## Compose

`deploy/release/compose.airflow-resonate.yaml` adds the profile `airflow-resonate`. It needs the compute pins:

```
cd deploy/release
podman compose --env-file ../../conformance/compute/pins.env \
  -f compose.yaml -f compose.compute.yaml -f compose.airflow-resonate.yaml \
  --profile airflow-resonate up -d
```

The engine's durable listener is loopback-only and unauthenticated (D111, D138), so the snippet uses a socat relay in the engine's network namespace. That relay exposes the durable API to the stack network, but not to the host or the tailnet (D2096). Use it only on a stack you trust.

## Not done yet

- An end-to-end run on a Loams engine, and the chaos cases from §74 §15: worker kill, lease expiry and scheduler failover.
- Remote task logs: point `[logging]` at the bucket as DX0 does. Workers serve no logs.
- `TestConnection` workloads (`supports_connection_test = False`).
- Moving the provider to `ostrium-labs/loams-airflow` (Q1031), if that repository is created.
