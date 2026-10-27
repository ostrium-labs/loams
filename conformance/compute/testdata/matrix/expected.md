### Temporal API (gateway)

Run: fixture, 2026-10-27.

SDK legs: 4 of 7 cases passed. 6 of the 8 `WorkflowService` methods are implemented (design 74 §5.8, DX1).

| Method | Go SDK | TypeScript SDK | Exercised by |
|---|---|---|---|
| `GetSystemInfo` | pass | pass | `gateway_dx1_scenarios_<sdk>` |
| `StartWorkflowExecution` | pass | **fail** | `gateway_dx1_scenarios_<sdk>`, `gateway_hello_<sdk>` |
| `SignalWorkflowExecution` | pass | expected failure (#2) | `gateway_signal_<sdk>` |
| `QueryWorkflow` | skipped | not run | `gateway_query_<sdk>`; the legacy query on a workflow-task poll |
| `SignalWithStartWorkflowExecution` | pass | not exercised | `gateway_dx1_scenarios_go`; the Go client |
| `RegisterNamespace` | not exercised | not exercised | admin only |

Not implemented (2; each answers `UNIMPLEMENTED` with a link to this page): `ListWorkflowExecutions`, `UpdateWorkflowExecution`.
