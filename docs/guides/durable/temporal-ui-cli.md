# The Temporal Web UI and the `temporal` CLI on the Loams gateway

Status: **preview, not yet run end to end.** The gateway code and its tests are in the repository;
nobody has yet pointed a real `temporalio/ui` container or a real `temporal` CLI at a running
gateway and recorded the result. This page says what should work, what will not, and why.

The Loams Temporal gateway (`loams-temporal`, see [the gateway guide](temporal-gateway.md)) serves
the Temporal API, so the stock tools can read from it:

- the Temporal Web UI: the `temporalio/ui` image, which runs `ui-server` (MIT);
- the `temporal` CLI.

Neither tool is changed or forked.

## What the gateway serves for them

The UI and the CLI read a few methods that SDK workers never call. The gateway serves them
(decisions D2050–D2053; `GetClusterInfo` and the reverse history are lane TMP-api's):

| Method | Used by | What the gateway answers |
|---|---|---|
| `GetSystemInfo` | both, on connect | The gateway's version (`loams-temporal x.y.z`) and the capabilities it really has (X1.R2) |
| `GetClusterInfo` | UI (first call), `temporal operator cluster describe` | Served by lane TMP-api's `describe.rs`: cluster name `loams`, the store (`postgres` or `memory`) and its shard count when the standalone runner knows them, and the gateway's version (`loams-temporal x.y.z`), never a Temporal server version, so the UI's version gates keep pages off whose methods the gateway does not serve |
| `ListNamespaces`, `DescribeNamespace` | UI namespace picker, `temporal operator namespace list/describe` | The Temporal namespaces of the caller's Loams namespace |
| `OperatorService.ListSearchAttributes` | UI filter builder, `temporal operator search-attribute list` | The system attributes the gateway stores for every run (`WorkflowId`, `RunId`, `WorkflowType`, `ExecutionStatus`, `TaskQueue`, `StartTime`, `CloseTime`, `ExecutionTime`). It has no custom attributes yet |
| `DescribeWorkflowExecution`, `GetWorkflowExecutionHistory` | UI workflow page, `temporal workflow describe/show` | As for SDKs |
| `GetWorkflowExecutionHistoryReverse` | UI workflow page | The history newest first, in pages (lane TMP-api) |
| `QueryWorkflow` | UI "Queries" tab, `temporal workflow query` | As for SDKs; it needs a running worker |
| Nexus endpoints (`OperatorService.ListNexusEndpoints`, `Get…`, `Create…`, `Update…`, `Delete…`) and every other Nexus method | UI Nexus page, SDK workers that register Nexus services | `UNIMPLEMENTED` with "Nexus is not supported by the Loams Temporal gateway…". An SDK or UI page can tell "no Nexus here" apart from an outage |

Every other `OperatorService` method answers `UNIMPLEMENTED` with a link to the compatibility
matrix, as the `WorkflowService` methods outside the matrix do.

## What does not work yet

- **The UI's workflow list** (the start page of a namespace) calls `ListWorkflowExecutions` and
  `CountWorkflowExecutions`. They answer `UNIMPLEMENTED` until the gateway's visibility listing
  lands (the TMP-sched work), so the list shows an error. Open a workflow directly instead:
  `http://127.0.0.1:8233/namespaces/default/workflows/<workflow ID>/<run ID>/history`.
- **Schedules, batch operations, workers, deployments, task-queue details and the Nexus page**
  show errors. Their methods are not implemented.
- **Writes from the UI** (terminate, cancel, signal, reset, start) are off. The UI's listener is
  read-only, and the compose overlay hides the buttons (`TEMPORAL_DISABLE_WRITE_ACTIONS=true`).
  Use the CLI with an API key for writes.
- **`temporal operator cluster health`** uses the gRPC health service, which the gateway does not
  serve yet.
- **`temporal workflow list`/`count`** need the visibility listing, the same as the UI list.

## The UI listener (D2050)

`ui-server` can send an OIDC token or headers forwarded from the browser, but not a static API
key, while the gateway authenticates every call with a Loams API key. The gateway therefore has a
second listener just for the UI:

```toml
# gateway.toml
[ui]
listen = "127.0.0.1:7234"          # must be loopback; the gateway refuses anything else
loams_namespace_id = "conformance" # whose Temporal namespaces the UI sees
read_only = true                   # the default: only reads pass
```

- **Who calls on it:** every call on this listener is that Loams namespace's, never admin. The
  principal is attached inside the gateway, so no header a client sends can change it.
- **Read-only:** a write answers `PERMISSION_DENIED` with a message that names `[ui] read_only`.
  The methods that pass are `READ_METHODS` in `crates/loams-temporal-frontend/src/ui.rs`.
- **Who can reach it:** anyone who can connect to the listener acts as that Loams namespace. Keep
  it on loopback, inside the pod or compose network namespace that `ui-server` shares with the
  gateway, and put the UI itself behind your own sign-in if more than one person uses the host.
  `read_only = false` lets the UI's write buttons work, for a single-user machine only.

## Running the UI in compose

The overlay `conformance/compute/temporal-ui.yaml` adds `temporal-ui` to the `temporal-gateway`
profile:

- **Image:** `temporalio/ui` 2.55.0, pinned by digest.
- **Network:** it runs in the engine's network namespace, the same as the gateway, and reaches the
  UI listener on `127.0.0.1:7234`.
- **Port:** port 8233 is published on the host's loopback only.

The gateway's `gateway.toml` needs the `[ui]` block above, with `loams_namespace_id` set to the
namespace of its API key.

```sh
podman compose -f deploy/release/compose.yaml -f deploy/release/compose.compute.yaml \
  -f conformance/compute/temporal-gateway.yaml -f conformance/compute/temporal-ui.yaml \
  --profile temporal-gateway up -d
# then open http://127.0.0.1:8233
```

Outside compose, run the image next to `loams-temporal` with
`TEMPORAL_ADDRESS=127.0.0.1:7234`, sharing its network namespace (for example a Kubernetes pod
with both containers).

## The `temporal` CLI

The CLI sends an API key as `authorization: Bearer …`, which the gateway's main listener
accepts. The CLI turns TLS on whenever an API key is set. On a plaintext listener, turn it off:

```sh
export KEY=lk_...            # a key listed under [[keys]] in gateway.toml
temporal --address 127.0.0.1:7233 --api-key "$KEY" --tls=false operator cluster describe
temporal --address 127.0.0.1:7233 --api-key "$KEY" --tls=false operator namespace list
temporal --address 127.0.0.1:7233 --api-key "$KEY" --tls=false operator search-attribute list -n default
temporal --address 127.0.0.1:7233 --api-key "$KEY" --tls=false workflow start \
  -n default --task-queue q --type MyWorkflow --workflow-id w1
temporal --address 127.0.0.1:7233 --api-key "$KEY" --tls=false workflow show -n default -w w1
```

A gateway in `insecure_dev` mode (loopback, no keys) needs neither flag. The CLI at the time of
writing is `temporalio/temporal` 1.9.1 (2026-09-14). Any release at least 14 days old should
work, because these methods have not changed shape for a long time.

## Tests

- `crates/loams-temporal-frontend/tests/ui.rs` covers:
  - `GetClusterInfo`;
  - reverse history pages, compared with the forward history;
  - `ListSearchAttributes`;
  - the Nexus and matrix answers;
  - a read-only UI listener that reads without a key and refuses writes.
- `tests/capabilities.rs` checks that every `WorkflowService` method outside the implemented set
  answers `UNIMPLEMENTED`.

These tests run the gateway in-process. No test starts the real UI container or the real CLI yet.
