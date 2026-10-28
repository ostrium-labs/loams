# Loams Durable (Temporal API): the Temporal gateway

Loams speaks the Temporal API through its own gateway, `loams-temporal`. It works with the
Temporal SDKs for the methods and SDKs listed in the [compatibility matrix](#compatibility-matrix)
below, and only those (design 74 D1512). The gateway is Loams' own code: it keeps each run's
history in Postgres and hands its tasks, timers and timeouts to Resonate (design 74 §5). It is
not the Temporal server and contains none of its code. "Temporal" is a trademark of Temporal
Technologies Inc., which does not endorse Loams.

**Status (2026-10-27): DX1, the MVP.** The official Go SDK v1.49.0 and TypeScript SDK 1.24.0 run
the DX1 scenarios against it (hello, activities, a signal, queries, cancel, terminate, a failed
workflow, heartbeats, a fan-out of ten activities), and each SDK's replayer accepts the histories
it writes. Timers beyond a cancelled one, retries, child workflows, continue-as-new, visibility
(`ListWorkflowExecutions`), schedules and updates are later phases (DX2-DX4). For production
Temporal workloads today, run the upstream Temporal server on Loams Postgres instead (compat mode,
design 74 §5.10).

## Connect an SDK

Point the SDK at the gateway's address and give it an API key. Nothing else changes.

```go
c, err := client.Dial(client.Options{
    HostPort:    "loams-temporal.example:7233",
    Namespace:   "default",
    Credentials: client.NewAPIKeyStaticCredentials(os.Getenv("LOAMS_API_KEY")),
})
```

```ts
const connection = await NativeConnection.connect({
  address: 'loams-temporal.example:7233',
  apiKey: process.env.LOAMS_API_KEY,
});
```

```python
# Python SDK (temporalio): its leg is new and not yet run; see the matrix below.
client = await Client.connect(
    "loams-temporal.example:7233",
    namespace="default",
    api_key=os.environ["LOAMS_API_KEY"],
    tls=True,  # False only against a plaintext test listener
)
```

The gateway itself serves plaintext gRPC (h2c), so off loopback put TLS in front of it. Some SDKs
switch TLS on by default when an API key is set: against a plaintext listener (a test setup), turn
TLS off in the SDK's connection options. The SDKs send the key as `authorization: Bearer <key>`. The gateway hashes it with SHA-256 and
looks the hash up in its `[[keys]]` table; each row binds the key to one Loams namespace and a
role (plan ruling X1.R5). Role `admin` may call `RegisterNamespace`; any other role may not. A
Temporal namespace belongs to the Loams namespace of the key that created it, so two Loams
namespaces can each have their own `default`. Zitadel tokens and `loams-authz` checks come with
the cluster deployment (DX3).

## Run it

### Development mode

```sh
cargo build -p loams-temporal-frontend --bin loams-temporal
target/debug/loams-temporal --dev
```

`--dev` serves `127.0.0.1:7233` with:

| Setting | Development mode | What it means |
|---|---|---|
| Store | `kind = "memory"` | History and state live in the process. **Nothing survives a restart.** |
| Durability | `kind = "in_process"` | An in-process stand-in for Resonate (`FakeDurability`) on the wall clock, not a Resonate server |
| Auth | no `[[keys]]` | Every caller is the admin of Loams namespace `local` (`dev_loams_namespace`) |
| Task-token keys | none | A random key per start: task tokens from a previous process are refused |
| Namespace | `default` | Bootstrapped, 72 h retention |

Any of these settings needs `insecure_dev = true`, and `insecure_dev` refuses to start unless
`listen` is a loopback address (`config_rejects_dev_settings_on_a_public_listener`).

### With Postgres and Resonate

```sh
loams-temporal --config temporal-gateway.toml
loams-gateway --module temporal --config temporal-gateway.toml   # built with --features temporal
```

Both serve the same frontend. `loams-temporal` depends on nothing from Loams outside the
`loams-temporal-*` crates (`standalone_binary_has_no_loams_deps`), so it can serve any Resonate
server. The configuration starts from
[`deploy/release/temporal-gateway.toml.example`](../../../deploy/release/temporal-gateway.toml.example):

| Key | Default | Meaning |
|---|---|---|
| `listen` | `127.0.0.1:7233` | The gRPC listener (h2c). Put TLS in front of it off loopback; the gateway does not terminate TLS |
| `insecure_dev` | `false` | Allows the development settings above, on a loopback listener only |
| `shards` | `64` | Shards of the store, fixed when the database is created; another count on an existing database refuses to start (X1.R6) |
| `pid` | `loams-temporal-1` | This process's Resonate process ID |
| `[store] kind` | required | `postgres` with `url_env` (the name of the environment variable holding the URL), or `memory` (development) |
| `[durability] kind` | required | `resonate` with `url` and an optional `token_env`, or `in_process` (development) |
| `[[task_token_keys]]` | none | `id` and `secret_env`; the first key signs, every key verifies, so keys rotate by adding one in front (X1.R4) |
| `[[keys]]` | none | `sha256` (hex SHA-256 of the API key), `loams_namespace_id`, `role` |
| `dev_loams_namespace` | none | The Loams namespace of every caller when there are no keys (development) |
| `[[bootstrap_namespaces]]` | none | `loams_namespace_id`, `name`, `retention_hours` (72): Temporal namespaces created at start when missing |

Secrets never go in the file: the database URL, the task-token secrets and the Resonate token are
read from the environment variables the file names.

The Postgres database must be reached through a session or direct endpoint, never PgDog
transaction pooling: the store uses `LISTEN`/`NOTIFY` for its outbox (design 74 §5.5 rule 5). The
gateway creates and migrates its schema at start.

The gateway reaches Resonate through the fork's `resonate-client` crate (`HttpDurability`): its
typed protocol requests, and its poll consumer for the task and settlement messages (design 74
D1516; D2040, D2043). The interim adapter of plan D1591 is gone, and the configuration did not
change.

### Compose

The compose profile `temporal-gateway` in `deploy/release/compose.compute.yaml` runs the release
image `ghcr.io/ostrium-labs/loams-temporal` (from the first release after 0.1.2; D2030, D2031):

- in the engine container's network namespace, so it reaches the engine's Resonate on
  `127.0.0.1:8001` and SDKs on the compose network dial `loams:7233`; no host port;
- on its own database `loams_temporal` (role `loams_temporal`), which `compute-init` creates on
  `compute1`'s direct endpoint;
- with your `temporal-gateway.toml` (copy `deploy/release/temporal-gateway.toml.example`; API key
  hashes in `[[keys]]`) and a task-token key of 32 bytes or more in `LOAMS_TEMPORAL_TOKEN_KEY_1`.

```sh
cd deploy/release
cp temporal-gateway.toml.example temporal-gateway.toml   # then fill in [[keys]]
docker compose --env-file .env --env-file ../../conformance/compute/pins.env \
  -f compose.yaml -f compose.compute.yaml --profile temporal-gateway up -d
```

The Go and TypeScript SDK legs run through this profile on pull requests that touch the gateway
(`conformance/compute/temporal/gateway/run.sh --profile`, workflow `temporal-gateway.yml`), with
an image built from the pull request. That leg is new and not yet green; the results above are
from the loopback runs. The Python SDK (temporalio 1.34.0, lane TSDK-python) has a leg of its own
in the same workflow (job `sdk-leg`, `SDK_LEGS=python`); it has not been run yet, so its matrix
column reads "not run". See `deploy/release/README.md`, "The Temporal gateway".

## The Temporal Web UI and the CLI

The stock Temporal Web UI and the `temporal` CLI can read from the gateway (preview): see
[The Temporal Web UI and the `temporal` CLI](temporal-ui-cli.md).

## Limits

| Limit | Value | Enforced |
|---|---|---|
| One gRPC request | 4 MiB (Temporal's) | Yes: larger requests are refused by the transport |
| Signals per run | 2,000 | Yes: `RESOURCE_EXHAUSTED` |
| One payload | 2 MiB | From #1291 (DX1F.R2), as Temporal does; unit tests only |
| History per run | 50 MiB and 51,200 events | From #1291: the run is terminated, as Temporal does; unit tests only |
| Pending activities per run | 2,000 | From #1291: the workflow task fails, as Temporal does; unit tests only |
| Long polls | answered empty after 60 s, or 5 s before the request's own deadline (half of a deadline of 5 s or less) | Yes (X1.R3). A history long poll answered empty keeps a page token from #1291 on (DX1F.R1); before it the Go SDK failed under load (matrix below, known issue 1) |

The values are Temporal's defaults at v1.32.1 (plan ruling R0.9, X1.R7). The payload, history
and pending-activity limits were defined but not checked in DX1 (the
[DX1 exit report](../../plans/dx1-exit-report.md), open issues); #1291 enforces them, and no SDK
run has exercised them yet.
Lowering them per namespace is DX2's.

## Deployment limits in DX1

- **One replica** (X1.R8). Run one gateway process per database: it owns every task queue, and
  long polls and queries are not forwarded between processes. Partition leases and several
  replicas come in DX3.
- **Loopback to Resonate.** The engine's durable listener has no authentication yet (D111), so
  the gateway reaches Resonate only on the same host or network namespace.
- **No sticky queues.** Workers replay a run's history on each workflow task (Q1156; DX3).
- **No `/metrics` endpoint.** The relay counts `loams_temporal_outbox_{dispatched,conflict,unavailable,rejected}_total`
  (no tenant or namespace labels), but neither binary serves them yet.
- **No visibility.** `ListWorkflowExecutions` and the other list methods answer `UNIMPLEMENTED`
  (DX3). `DescribeWorkflowExecution` works.

Every method outside the DX1 set answers `UNIMPLEMENTED` with a link to this page (X1.R18). The
SDKs tolerate it where they call such a method in the DX1 runs: both call `ShutdownWorker` when a
worker stops, and the Go corpus generator falls back to the current run when
`ListWorkflowExecutions` is unimplemented.
`GetSystemInfo` advertises only built behaviour: `sdk_metadata`, `signal_and_query_header`,
`internal_error_differentiation`, `activity_failure_include_heartbeat` and
`encoded_failure_attributes` (X1.R2). The test `capabilities_match_impl` keeps the advertised set,
the implemented set and this page's DX1 list the same.

## Compatibility matrix

Generated by `scripts/compute/matrix.py temporal-gateway` from the SDK legs' JUnit
(`conformance/compute/temporal/gateway/run.sh`), committed as
`conformance/compute/temporal/gateway/last-run.junit.xml`; which cases exercise which method is
`conformance/compute/temporal/gateway/methods.toml`. Do not edit the table by hand: re-run the legs
and `matrix.py temporal-gateway --junit … --splice docs/guides/durable/temporal-gateway.md`.

<!-- matrix:temporal-gateway:begin -->
### Temporal API (gateway)

Run: 2026-10-27, dev 0e9c0e8fb, loams-temporal in development mode (memory store, in-process Resonate) on loopback, the corpus's Go and TypeScript generators.

Not covered by this run: the SDK legs ran the gateway in development mode (memory store, in-process Resonate) on loopback with the corpus scenarios only, so they say nothing about the Postgres store, the Resonate server, the compose profile or load. The only run under load is the 2026-10-25 bench (Go SDK only; Postgres store, the fork's Resonate server): fan-out finished 500 of 500 at 1.3 workflows/s (p99 28.2 s), and burst and chain failed (known issue 1). The TypeScript SDK has not been run under load. Payload, pending-activity and history limits (X1.R7) are enforced from #1291 on and covered by unit tests only.

SDK legs: 26 of 26 cases passed. 52 of the 123 `WorkflowService` methods are implemented (design 74 §5.8, DX1).

| Method | Go SDK v1.49.0 | TypeScript SDK 1.24.0 | Python SDK 1.34.0 | .NET SDK 1.20.0 | Java SDK 1.40.0 | Ruby SDK 1.9.0 | Exercised by |
|---|---|---|---|---|---|---|---|
| `GetSystemInfo` | pass | pass | not yet run | not yet run | not yet run | not yet run | `gateway_dx1_scenarios_<sdk>`; client connect |
| `DescribeNamespace` | pass | pass | not yet run | not yet run | not yet run | not yet run | `gateway_dx1_scenarios_<sdk>`; worker start |
| `RegisterNamespace` | pass | not exercised | not yet run | not yet run | not yet run | not yet run | `gateway_dx1_scenarios_<sdk>`; the Go and Java corpus generators (admin role, X1.R9); `api_keys_authenticate_and_bind_the_loams_namespace` |
| `ListNamespaces` | not exercised | not exercised | not yet run | not yet run | not yet run | not yet run | no SDK call in these runs and no test yet |
| `StartWorkflowExecution` | pass | pass | not yet run | not yet run | not yet run | not yet run | `gateway_dx1_scenarios_<sdk>`, `gateway_hello_<sdk>` |
| `DescribeWorkflowExecution` | pass | pass | not yet run | not yet run | not yet run | not yet run | `gateway_dx1_differential_<sdk>_signal_then_complete`, `gateway_dx1_differential_<sdk>_query_while_running`, `gateway_dx1_differential_<sdk>_cancel_requested`, `gateway_dx1_differential_<sdk>_terminated`; the generators wait for a running run |
| `GetWorkflowExecutionHistory` | pass, known issue 1 | pass | not yet run | not yet run | not yet run | not yet run | `gateway_dx1_scenarios_<sdk>`, `gateway_histories_replay_<sdk>`; waiting for a result (close-event long poll) and the export |
| `TerminateWorkflowExecution` | pass | pass | not yet run | not yet run | not yet run | not yet run | `gateway_dx1_differential_<sdk>_terminated` |
| `RequestCancelWorkflowExecution` | pass | pass | not yet run | not yet run | not yet run | not yet run | `gateway_dx1_differential_<sdk>_cancel_requested` |
| `PollWorkflowTaskQueue` | pass | pass | not yet run | not yet run | not yet run | not yet run | `gateway_dx1_scenarios_<sdk>`, `gateway_hello_<sdk>` |
| `RespondWorkflowTaskCompleted` | pass | pass | not yet run | not yet run | not yet run | not yet run | `gateway_dx1_scenarios_<sdk>`, `gateway_hello_<sdk>` |
| `RespondWorkflowTaskFailed` | not exercised | not exercised | not yet run | not yet run | not yet run | not yet run | no SDK call in these runs; the state machine's table tests |
| `PollActivityTaskQueue` | pass | pass | not yet run | not yet run | not yet run | not yet run | `gateway_hello_<sdk>`, `gateway_dx1_differential_<sdk>_two_activities_sequential`, `gateway_dx1_differential_<sdk>_activity_parallel_fanout_10` |
| `RespondActivityTaskCompleted` | pass | pass | not yet run | not yet run | not yet run | not yet run | `gateway_hello_<sdk>`, `gateway_dx1_differential_<sdk>_two_activities_sequential`, `gateway_dx1_differential_<sdk>_activity_parallel_fanout_10` |
| `RespondActivityTaskFailed` | not exercised | not exercised | not yet run | not yet run | not yet run | not yet run | no SDK call in these runs (no DX1 scenario fails an activity); the scripted worker of `tests/scenarios.rs` |
| `RespondActivityTaskCanceled` | not exercised | not exercised | not yet run | not yet run | not yet run | not yet run | no SDK call in these runs and no test yet |
| `RecordActivityTaskHeartbeat` | pass | pass | not yet run | not yet run | not yet run | not yet run | `gateway_dx1_differential_<sdk>_activity_heartbeat_details` |
| `SignalWorkflowExecution` | pass | pass | not yet run | not yet run | not yet run | not yet run | `gateway_signal_<sdk>`, `gateway_dx1_differential_<sdk>_signal_then_complete` |
| `QueryWorkflow` | pass | pass | not yet run | not yet run | not yet run | not yet run | `gateway_dx1_differential_<sdk>_query_while_running` |
| `RespondQueryTaskCompleted` | pass | pass | not yet run | not yet run | not yet run | not yet run | `gateway_dx1_differential_<sdk>_query_while_running` |
| `ResetStickyTaskQueue` | not exercised | not exercised | not yet run | not yet run | not yet run | not yet run | a no-op; no SDK call in these runs and no test yet |
| `UpdateWorkflowExecution` | not exercised | not exercised | not yet run | not yet run | not yet run | not yet run | TMP-api (D2080): statemachine unit tests and the golden corpus scenario only; no SDK leg yet |
| `PollWorkflowExecutionUpdate` | not exercised | not exercised | not yet run | not yet run | not yet run | not yet run | TMP-api (D2080): statemachine unit tests and the golden corpus scenario only; no SDK leg yet |
| `SignalWithStartWorkflowExecution` | not exercised | not exercised | not yet run | not yet run | not yet run | not yet run | TMP-api (D2085, D2083): statemachine and policy unit tests and the golden corpus scenario only; no SDK leg yet |
| `ResetWorkflowExecution` | not exercised | not exercised | not yet run | not yet run | not yet run | not yet run | TMP-api (D2085, D2083): statemachine and policy unit tests and the golden corpus scenario only; no SDK leg yet |
| `RespondActivityTaskCompletedById` | not exercised | not exercised | not yet run | not yet run | not yet run | not yet run | TMP-api (D2086): unit tests only; no SDK leg yet |
| `RespondActivityTaskFailedById` | not exercised | not exercised | not yet run | not yet run | not yet run | not yet run | TMP-api (D2086): unit tests only; no SDK leg yet |
| `RespondActivityTaskCanceledById` | not exercised | not exercised | not yet run | not yet run | not yet run | not yet run | TMP-api (D2086): unit tests only; no SDK leg yet |
| `RecordActivityTaskHeartbeatById` | not exercised | not exercised | not yet run | not yet run | not yet run | not yet run | TMP-api (D2086): unit tests only; no SDK leg yet |
| `GetWorkflowExecutionHistoryReverse` | not exercised | not exercised | not yet run | not yet run | not yet run | not yet run | TMP-api (D2086); the Temporal Web UI's workflow page; `reverse_history_pages_are_the_history_newest_first`; no SDK leg yet |
| `DescribeTaskQueue` | not exercised | not exercised | not yet run | not yet run | not yet run | not yet run | TMP-api (D2086): unit tests only; no SDK leg yet |
| `GetClusterInfo` | not exercised | not exercised | not yet run | not yet run | not yet run | not yet run | TMP-api (D2086); the Temporal Web UI's first call and `temporal operator cluster describe` (D2051); `cluster_info_names_the_gateway_not_a_temporal_server`; no SDK leg yet |
| `ListTaskQueuePartitions` | not exercised | not exercised | not yet run | not yet run | not yet run | not yet run | TMP-api (D2086): unit tests only; no SDK leg yet |
| `ShutdownWorker` | not exercised | not exercised | not yet run | not yet run | not yet run | not yet run | TMP-api (D2086): unit tests only; no SDK leg yet |
| `UpdateNamespace` | not exercised | not exercised | not yet run | not yet run | not yet run | not yet run | TMP-api (D2088): unit tests only; versioning reads answer no sets, no rules, all reachable |
| `DeprecateNamespace` | not exercised | not exercised | not yet run | not yet run | not yet run | not yet run | TMP-api (D2088): unit tests only; versioning reads answer no sets, no rules, all reachable |
| `GetWorkerBuildIdCompatibility` | not exercised | not exercised | not yet run | not yet run | not yet run | not yet run | TMP-api (D2088): unit tests only; versioning reads answer no sets, no rules, all reachable |
| `GetWorkerVersioningRules` | not exercised | not exercised | not yet run | not yet run | not yet run | not yet run | TMP-api (D2088): unit tests only; versioning reads answer no sets, no rules, all reachable |
| `GetWorkerTaskReachability` | not exercised | not exercised | not yet run | not yet run | not yet run | not yet run | TMP-api (D2088): unit tests only; versioning reads answer no sets, no rules, all reachable |
| `CreateSchedule` | not exercised | not exercised | not yet run | not yet run | not yet run | not yet run | no SDK call in these runs; loams-temporal-frontend tests/schedules.rs |
| `DescribeSchedule` | not exercised | not exercised | not yet run | not yet run | not yet run | not yet run | no SDK call in these runs; loams-temporal-frontend tests/schedules.rs |
| `UpdateSchedule` | not exercised | not exercised | not yet run | not yet run | not yet run | not yet run | no SDK call in these runs; loams-temporal-frontend tests/schedules.rs |
| `PatchSchedule` | not exercised | not exercised | not yet run | not yet run | not yet run | not yet run | no SDK call in these runs; loams-temporal-frontend tests/schedules.rs |
| `DeleteSchedule` | not exercised | not exercised | not yet run | not yet run | not yet run | not yet run | no SDK call in these runs; loams-temporal-frontend tests/schedules.rs |
| `ListSchedules` | not exercised | not exercised | not yet run | not yet run | not yet run | not yet run | no SDK call in these runs; loams-temporal-frontend tests/schedules.rs |
| `ListScheduleMatchingTimes` | not exercised | not exercised | not yet run | not yet run | not yet run | not yet run | no SDK call in these runs; loams-temporal-frontend tests/schedules.rs |
| `ListWorkflowExecutions` | not exercised | not exercised | not yet run | not yet run | not yet run | not yet run | no SDK call in these runs; loams-temporal-frontend tests/visibility.rs |
| `ScanWorkflowExecutions` | not exercised | not exercised | not yet run | not yet run | not yet run | not yet run | no SDK call in these runs; loams-temporal-frontend tests/visibility.rs |
| `CountWorkflowExecutions` | not exercised | not exercised | not yet run | not yet run | not yet run | not yet run | no SDK call in these runs; loams-temporal-frontend tests/visibility.rs |
| `GetSearchAttributes` | not exercised | not exercised | not yet run | not yet run | not yet run | not yet run | no SDK call in these runs; loams-temporal-frontend tests/visibility.rs |
| `ListOpenWorkflowExecutions` | not exercised | not exercised | not yet run | not yet run | not yet run | not yet run | TMP-api: unit tests (listing.rs); served from Store::list_runs |
| `ListClosedWorkflowExecutions` | not exercised | not exercised | not yet run | not yet run | not yet run | not yet run | TMP-api: unit tests (listing.rs); served from Store::list_runs |

Not yet run: Python SDK 1.34.0, .NET SDK 1.20.0, Java SDK 1.40.0, Ruby SDK 1.9.0. The leg exists, but this run did not include it, so its cells say nothing yet.

Known issues (not shown by the cases above):

1. `GetWorkflowExecutionHistory`, Go SDK v1.49.0: under load the Go SDK failed in `run.Get` with "could not get last history event for workflow" (bench 2026-10-25: 500 workflows started at once, and a chain of 5 activities and 5 one-second sleeps). A close-event long poll that reached its deadline before the run closed answered an empty page without a page token. Fixed in #1291 (DX1F.R1, DX1F.R3, with regression tests); the bench has not been re-run since.

Not implemented (71; each answers `UNIMPLEMENTED` with a link to this page): `ExecuteMultiOperation`, `DeleteWorkflowExecution`, `ListArchivedWorkflowExecutions`, `CountSchedules`, `UpdateWorkerBuildIdCompatibility`, `UpdateWorkerVersioningRules`, `DescribeDeployment`, `DescribeWorkerDeploymentVersion`, `ListDeployments`, `GetDeploymentReachability`, `GetCurrentDeployment`, `SetCurrentDeployment`, `SetWorkerDeploymentCurrentVersion`, `DescribeWorkerDeployment`, `DeleteWorkerDeployment`, `DeleteWorkerDeploymentVersion`, `SetWorkerDeploymentRampingVersion`, `ListWorkerDeployments`, `CreateWorkerDeployment`, `CreateWorkerDeploymentVersion`, `UpdateWorkerDeploymentVersionComputeConfig`, `ValidateWorkerDeploymentVersionComputeConfig`, `UpdateWorkerDeploymentVersionMetadata`, `SetWorkerDeploymentManager`, `StartBatchOperation`, `StopBatchOperation`, `DescribeBatchOperation`, `ListBatchOperations`, `PollNexusTaskQueue`, `RespondNexusTaskCompleted`, `RespondNexusTaskFailed`, `UpdateActivityOptions`, `UpdateWorkflowExecutionOptions`, `PauseActivity`, `UnpauseActivity`, `ResetActivity`, `CreateWorkflowRule`, `DescribeWorkflowRule`, `DeleteWorkflowRule`, `ListWorkflowRules`, `TriggerWorkflowRule`, `RecordWorkerHeartbeat`, `ListWorkers`, `CountWorkers`, `UpdateTaskQueueConfig`, `FetchWorkerConfig`, `UpdateWorkerConfig`, `DescribeWorker`, `PauseWorkflowExecution`, `UnpauseWorkflowExecution`, `StartActivityExecution`, `StartNexusOperationExecution`, `DescribeActivityExecution`, `DescribeNexusOperationExecution`, `PollActivityExecution`, `PollNexusOperationExecution`, `ListActivityExecutions`, `ListNexusOperationExecutions`, `CountActivityExecutions`, `CountNexusOperationExecutions`, `RequestCancelActivityExecution`, `RequestCancelNexusOperationExecution`, `TerminateActivityExecution`, `DeleteActivityExecution`, `PauseActivityExecution`, `ResetActivityExecution`, `UnpauseActivityExecution`, `UpdateActivityExecutionOptions`, `TerminateNexusOperationExecution`, `DeleteNexusOperationExecution`, `PollWorkflowExecutionTimeSkipping`.
<!-- matrix:temporal-gateway:end -->
