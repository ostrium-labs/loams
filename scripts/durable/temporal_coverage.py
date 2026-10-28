#!/usr/bin/env python3
"""Writes docs/guides/durable/temporal-api-coverage.md: every RPC of the
vendored temporalio/api WorkflowService and OperatorService, and whether the
Loams Temporal gateway implements it.

    scripts/durable/temporal_coverage.py           rewrite the table
    scripts/durable/temporal_coverage.py --check   exit 1 if it is stale

The RPC list comes from the vendored protos (crates/loams-temporal-proto), so
a proto update that adds a method shows up as "not listed" until STATUS names
it. STATUS is the only thing to edit by hand; "implemented" rows must match
`IMPLEMENTED` in crates/loams-temporal-frontend/src/capabilities.rs (checked
here too).
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
PROTO = ROOT / "crates/loams-temporal-proto/proto/temporal/api"
CAPS = ROOT / "crates/loams-temporal-frontend/src/capabilities.rs"
OUT = ROOT / "docs/guides/durable/temporal-api-coverage.md"

DX1 = "implemented (DX1)"
TMP = "implemented (TMP-api)"


def planned(group: str) -> str:
    return f"planned: TMP-api {group}"


SCHED = "lane TMP-sched (schedules and visibility)"
SCHED_DONE = "implemented (TMP-sched)"
LATER = "not planned yet"

# RPC -> (status, note). Groups: A update, B signal-with-start/query/reset,
# C describe and activity-by-ID, D children and continue-as-new (commands
# only), E versioning and namespaces, F batch.
STATUS: dict[str, tuple[str, str]] = {
    # Namespaces
    "RegisterNamespace": (DX1, "admin role only"),
    "DescribeNamespace": (DX1, ""),
    "ListNamespaces": (DX1, "one page"),
    "UpdateNamespace": (TMP, "admin role; description, owner email, data, state, retention (memory store; Postgres store pending TMP-store)"),
    "DeprecateNamespace": (TMP, "admin role"),
    # Workflow lifecycle
    "StartWorkflowExecution": (DX1, "ID reuse and conflict policies (TMP-api B); no start delay, no cron"),
    "ExecuteMultiOperation": (LATER, "update-with-start; needs start and update in one commit"),
    "GetWorkflowExecutionHistory": (DX1, "paged; long poll; close-event filter"),
    "GetWorkflowExecutionHistoryReverse": (TMP, ""),
    "PollWorkflowTaskQueue": (DX1, "no sticky queues (Q1156)"),
    "RespondWorkflowTaskCompleted": (DX1, "update messages (TMP-api A); child workflow, continue-as-new, external signal/cancel, upsert commands (TMP-api D)"),
    "RespondWorkflowTaskFailed": (DX1, ""),
    "PollActivityTaskQueue": (DX1, ""),
    "RecordActivityTaskHeartbeat": (DX1, ""),
    "RecordActivityTaskHeartbeatById": (TMP, ""),
    "RespondActivityTaskCompleted": (DX1, ""),
    "RespondActivityTaskCompletedById": (TMP, ""),
    "RespondActivityTaskFailed": (DX1, ""),
    "RespondActivityTaskFailedById": (TMP, ""),
    "RespondActivityTaskCanceled": (DX1, ""),
    "RespondActivityTaskCanceledById": (TMP, ""),
    "RequestCancelWorkflowExecution": (DX1, ""),
    "SignalWorkflowExecution": (DX1, ""),
    "SignalWithStartWorkflowExecution": (TMP, "ID reuse and conflict policies"),
    "ResetWorkflowExecution": (TMP, "to a workflow task boundary; signals reapplied, updates not yet"),
    "TerminateWorkflowExecution": (DX1, "fails a started workflow task first (TMP-api B, D2084)"),
    "DeleteWorkflowExecution": (LATER, "needs the store's delete path (lane TMP-store)"),
    "QueryWorkflow": (DX1, "to a live poller after any outstanding workflow task; reject conditions (TMP-api B)"),
    "RespondQueryTaskCompleted": (DX1, ""),
    "ResetStickyTaskQueue": (DX1, "no-op: no sticky queues"),
    "ShutdownWorker": (TMP, "no-op: no sticky queues"),
    "DescribeWorkflowExecution": (DX1, "auto reset points, activity attempts and expiry (TMP-api B, C)"),
    "DescribeTaskQueue": (TMP, "pollers seen in the last 5 minutes; backlog with report_stats"),
    "GetClusterInfo": (TMP, ""),
    "GetSystemInfo": (DX1, "advertises only what is built"),
    "ListTaskQueuePartitions": (TMP, "one partition per queue (X1.R8)"),
    # Updates
    "UpdateWorkflowExecution": (TMP, "durable admission; ordinary (not speculative) workflow task"),
    "PollWorkflowExecutionUpdate": (TMP, ""),
    # Visibility and schedules
    "ListOpenWorkflowExecutions": (SCHED, ""),
    "ListClosedWorkflowExecutions": (SCHED, ""),
    "ListWorkflowExecutions": (SCHED_DONE, ""),
    "ListArchivedWorkflowExecutions": (LATER, "no archival"),
    "ScanWorkflowExecutions": (SCHED_DONE, ""),
    "CountWorkflowExecutions": (SCHED_DONE, ""),
    "GetSearchAttributes": (SCHED_DONE, ""),
    "CreateSchedule": (SCHED_DONE, ""),
    "DescribeSchedule": (SCHED_DONE, ""),
    "UpdateSchedule": (SCHED_DONE, ""),
    "PatchSchedule": (SCHED_DONE, ""),
    "ListScheduleMatchingTimes": (SCHED_DONE, ""),
    "DeleteSchedule": (SCHED_DONE, ""),
    "ListSchedules": (SCHED_DONE, ""),
    "CountSchedules": (SCHED, ""),
    # Worker versioning (build IDs)
    "UpdateWorkerBuildIdCompatibility": (LATER, "no routing by build ID yet"),
    "GetWorkerBuildIdCompatibility": (TMP, "no version sets: routing is by task queue only"),
    "UpdateWorkerVersioningRules": (LATER, "no routing by build ID yet"),
    "GetWorkerVersioningRules": (TMP, "no rules"),
    "GetWorkerTaskReachability": (TMP, "every build ID reachable"),
    # Worker deployments
    "DescribeDeployment": (LATER, "worker deployments"),
    "DescribeWorkerDeploymentVersion": (LATER, "worker deployments"),
    "ListDeployments": (LATER, "worker deployments"),
    "GetDeploymentReachability": (LATER, "worker deployments"),
    "GetCurrentDeployment": (LATER, "worker deployments"),
    "SetCurrentDeployment": (LATER, "worker deployments"),
    "SetWorkerDeploymentCurrentVersion": (LATER, "worker deployments"),
    "DescribeWorkerDeployment": (LATER, "worker deployments"),
    "DeleteWorkerDeployment": (LATER, "worker deployments"),
    "DeleteWorkerDeploymentVersion": (LATER, "worker deployments"),
    "SetWorkerDeploymentRampingVersion": (LATER, "worker deployments"),
    "ListWorkerDeployments": (LATER, "worker deployments"),
    "CreateWorkerDeployment": (LATER, "worker deployments"),
    "CreateWorkerDeploymentVersion": (LATER, "worker deployments"),
    "UpdateWorkerDeploymentVersionComputeConfig": (LATER, "worker deployments"),
    "ValidateWorkerDeploymentVersionComputeConfig": (LATER, "worker deployments"),
    "UpdateWorkerDeploymentVersionMetadata": (LATER, "worker deployments"),
    "SetWorkerDeploymentManager": (LATER, "worker deployments"),
    # Batch
    "StartBatchOperation": (TMP, "listed executions; terminate, cancel, signal; no visibility query yet"),
    "StopBatchOperation": (TMP, ""),
    "DescribeBatchOperation": (TMP, "jobs since this gateway started"),
    "ListBatchOperations": (TMP, "jobs since this gateway started; one page"),
    # Nexus (design 74 §2.2: out)
    "PollNexusTaskQueue": (LATER, "Nexus is out of scope (design 74 §2.2)"),
    "RespondNexusTaskCompleted": (LATER, "Nexus"),
    "RespondNexusTaskFailed": (LATER, "Nexus"),
    "StartNexusOperationExecution": (LATER, "Nexus"),
    "DescribeNexusOperationExecution": (LATER, "Nexus"),
    "PollNexusOperationExecution": (LATER, "Nexus"),
    "ListNexusOperationExecutions": (LATER, "Nexus"),
    "CountNexusOperationExecutions": (LATER, "Nexus"),
    "RequestCancelNexusOperationExecution": (LATER, "Nexus"),
    "TerminateNexusOperationExecution": (LATER, "Nexus"),
    "DeleteNexusOperationExecution": (LATER, "Nexus"),
    # Activity and workflow options, pause
    "UpdateActivityOptions": (LATER, ""),
    "UpdateWorkflowExecutionOptions": (LATER, ""),
    "PauseActivity": (LATER, ""),
    "UnpauseActivity": (LATER, ""),
    "ResetActivity": (LATER, ""),
    "PauseWorkflowExecution": (LATER, ""),
    "UnpauseWorkflowExecution": (LATER, ""),
    # Workflow rules
    "CreateWorkflowRule": (LATER, ""),
    "DescribeWorkflowRule": (LATER, ""),
    "DeleteWorkflowRule": (LATER, ""),
    "ListWorkflowRules": (LATER, ""),
    "TriggerWorkflowRule": (LATER, ""),
    # Workers
    "RecordWorkerHeartbeat": (LATER, ""),
    "ListWorkers": (LATER, ""),
    "CountWorkers": (LATER, ""),
    "UpdateTaskQueueConfig": (LATER, ""),
    "FetchWorkerConfig": (LATER, ""),
    "UpdateWorkerConfig": (LATER, ""),
    "DescribeWorker": (LATER, ""),
    # Standalone activities
    "StartActivityExecution": (LATER, "standalone activities"),
    "DescribeActivityExecution": (LATER, "standalone activities"),
    "PollActivityExecution": (LATER, "standalone activities"),
    "ListActivityExecutions": (LATER, "standalone activities"),
    "CountActivityExecutions": (LATER, "standalone activities"),
    "RequestCancelActivityExecution": (LATER, "standalone activities"),
    "TerminateActivityExecution": (LATER, "standalone activities"),
    "DeleteActivityExecution": (LATER, "standalone activities"),
    "PauseActivityExecution": (LATER, "standalone activities"),
    "ResetActivityExecution": (LATER, "standalone activities"),
    "UnpauseActivityExecution": (LATER, "standalone activities"),
    "UpdateActivityExecutionOptions": (LATER, "standalone activities"),
    # Time skipping
    "PollWorkflowExecutionTimeSkipping": (LATER, "test-server feature"),
}

OPERATOR: dict[str, tuple[str, str]] = {
    "AddSearchAttributes": (SCHED, ""),
    "RemoveSearchAttributes": (SCHED, ""),
    "ListSearchAttributes": (SCHED, ""),
    "DeleteNamespace": (LATER, "OperatorService is not served yet"),
    "AddOrUpdateRemoteCluster": (LATER, "no multi-cluster replication"),
    "RemoveRemoteCluster": (LATER, "no multi-cluster replication"),
    "ListClusters": (LATER, "no multi-cluster replication"),
    "GetNexusEndpoint": (LATER, "Nexus"),
    "CreateNexusEndpoint": (LATER, "Nexus"),
    "UpdateNexusEndpoint": (LATER, "Nexus"),
    "DeleteNexusEndpoint": (LATER, "Nexus"),
    "ListNexusEndpoints": (LATER, "Nexus"),
}


def rpcs(service_proto: Path) -> list[str]:
    return re.findall(r"^\s*rpc (\w+)\s*\(", service_proto.read_text(), re.M)


def implemented() -> set[str]:
    text = CAPS.read_text()
    body = text.split("pub const IMPLEMENTED")[1].split("];")[0]
    return set(re.findall(r'"(\w+)"', body))


def table(names: list[str], status: dict[str, tuple[str, str]]) -> list[str]:
    out = ["| RPC | Status | Notes |", "|---|---|---|"]
    for n in names:
        s, note = status.get(n, ("not listed", "new in the vendored protos"))
        out.append(f"| `{n}` | {s} | {note} |")
    return out


def render() -> str:
    ws = rpcs(PROTO / "workflowservice/v1/service.proto")
    ops = rpcs(PROTO / "operatorservice/v1/service.proto")
    impl = implemented()
    marked = {n for n, (s, _) in STATUS.items() if s.startswith("implemented")}
    if impl != marked:
        sys.exit(
            "STATUS and capabilities.rs IMPLEMENTED differ: "
            f"only in IMPLEMENTED {sorted(impl - marked)}, only in STATUS {sorted(marked - impl)}"
        )
    done = sum(1 for n in ws if n in impl)
    lines = [
        "# Temporal API coverage of the Loams gateway",
        "",
        "<!-- Generated by scripts/durable/temporal_coverage.py. Edit STATUS there. -->",
        "",
        "Every RPC of `temporalio/api` v1.63.5 (vendored in `crates/loams-temporal-proto`)"
        " and whether the Loams Temporal gateway (`crates/loams-temporal-frontend`) answers it."
        " Everything else answers `UNIMPLEMENTED` with a link to the compatibility matrix."
        " Status is preview: \"implemented\" means the method is built and has unit tests"
        " (CI runs them); it does not mean every SDK has run it end to end.",
        "",
        f"WorkflowService: {done} of {len(ws)} implemented. OperatorService: none yet"
        " (the gateway does not serve it).",
        "",
        "## WorkflowService",
        "",
        *table(ws, STATUS),
        "",
        "## OperatorService",
        "",
        *table(ops, OPERATOR),
        "",
    ]
    return "\n".join(lines)


def main() -> None:
    text = render()
    if "--check" in sys.argv[1:]:
        if not OUT.exists() or OUT.read_text() != text:
            sys.exit(f"{OUT.relative_to(ROOT)} is stale; run {Path(__file__).relative_to(ROOT)}")
        return
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(text)


if __name__ == "__main__":
    main()
