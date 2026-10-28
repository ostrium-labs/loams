### Temporal API (gateway)

Run: fixture, 2026-10-27.

Not covered by this run: load, and the Postgres store.

SDK legs: 4 of 7 cases passed. 6 of the 8 `WorkflowService` methods are implemented (design 74 §5.8, DX1).

| Method | Go SDK | TypeScript SDK | Java SDK | Exercised by |
|---|---|---|---|---|
| `GetSystemInfo` | pass | pass, known issue 1 | not yet run | `gateway_dx1_scenarios_<sdk>` |
| `StartWorkflowExecution` | pass | **fail** | not yet run | `gateway_dx1_scenarios_<sdk>`, `gateway_hello_<sdk>` |
| `SignalWorkflowExecution` | pass | expected failure (#2) | not yet run | `gateway_signal_<sdk>` |
| `QueryWorkflow` | skipped | not run | not yet run | `gateway_query_<sdk>`; the legacy query on a workflow-task poll |
| `SignalWithStartWorkflowExecution` | pass | not exercised | not yet run | `gateway_dx1_scenarios_go`; the Go client |
| `RegisterNamespace` | not exercised | not exercised | not yet run | admin only |

Not yet run: Java SDK. The leg exists, but this run did not include it, so its cells say nothing yet.

Known issues (not shown by the cases above):

1. `GetSystemInfo`, TypeScript SDK: the fixture's known issue, seen only under load.

Not implemented (2; each answers `UNIMPLEMENTED` with a link to this page): `ListWorkflowExecutions`, `UpdateWorkflowExecution`.
