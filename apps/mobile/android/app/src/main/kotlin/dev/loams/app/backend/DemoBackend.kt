package dev.loams.app.backend

import com.google.protobuf.Timestamp
import dev.loams.core.decision.Decision
import dev.loams.core.errors.Reason
import dev.loams.core.watch.StreamStatus
import dev.loams.proto.loams.approvals.v1.Approval
import dev.loams.proto.loams.approvals.v1.ApprovalPolicy
import dev.loams.proto.loams.approvals.v1.ApprovalState
import dev.loams.proto.loams.approvals.v1.Risk
import dev.loams.proto.loams.instance.v1.Environment
import dev.loams.proto.loams.instance.v1.Principal
import dev.loams.proto.loams.instance.v1.PrincipalKind
import dev.loams.proto.loams.operations.v1.Operation
import dev.loams.proto.loams.operations.v1.OperationState
import dev.loams.proto.loams.operations.v1.Progress
import kotlinx.coroutines.awaitCancellation
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update

/**
 * In-memory data with no network: demo mode (Q435's store-review answer), Compose previews and UI
 * tests. Decisions still go through the real biometric signing path; this backend just accepts
 * the proof without verifying it.
 */
class DemoBackend(private val now: () -> Long = System::currentTimeMillis) : Backend {
    override val instanceId = "demo"
    override val isDemo = true

    private val _approvals = MutableStateFlow(seedApprovals())
    private val _operations = MutableStateFlow(seedOperations())
    private val _approvalsStatus = MutableStateFlow<StreamStatus>(StreamStatus.Live)
    private val _operationsStatus = MutableStateFlow<StreamStatus>(StreamStatus.Live)

    override val approvals: StateFlow<List<Approval>> = _approvals.asStateFlow()
    override val approvalsStatus: StateFlow<StreamStatus> = _approvalsStatus.asStateFlow()
    override val operations: StateFlow<List<Operation>> = _operations.asStateFlow()
    override val operationsStatus: StateFlow<StreamStatus> = _operationsStatus.asStateFlow()

    override suspend fun watchApprovals() = awaitCancellation()

    override suspend fun watchOperations() {
        while (true) {
            delay(2_000)
            _operations.update { ops ->
                ops.map { op ->
                    if (op.state != OperationState.OPERATION_STATE_RUNNING) return@map op
                    val done = minOf(op.progress.total, op.progress.done + 5)
                    op.toBuilder()
                        .setProgress(op.progress.toBuilder().setDone(done).setFraction(done.toDouble() / op.progress.total))
                        .setState(if (done >= op.progress.total) OperationState.OPERATION_STATE_SUCCEEDED else op.state)
                        .build()
                }
            }
        }
    }

    override suspend fun instance() = InstanceSummary(
        issuer = "demo (no network)",
        instanceId = instanceId,
        serverVersion = "demo",
        edition = "OSS",
        apiVersions = listOf("loams.approvals.v1", "loams.operations.v1"),
        deviceId = "demo-device",
        trust = "none: in-memory demo data",
    )

    override suspend fun decide(approval: Approval, decision: Decision, reason: String, proof: String, idempotencyKey: String): DecideOutcome {
        val current = _approvals.value.firstOrNull { it.id == approval.id }
            ?: return DecideOutcome.Refused(Reason.APPROVAL_ALREADY_DECIDED, "already decided")
        if (current.revision != approval.revision) return DecideOutcome.Refused(Reason.UNKNOWN, "the approval changed; refresh it")
        val decided = current.toBuilder()
            .setState(if (decision == Decision.APPROVE) ApprovalState.APPROVAL_STATE_APPROVED else ApprovalState.APPROVAL_STATE_REJECTED)
            .setRevision(current.revision + 1)
            .build()
        _approvals.update { list -> list.filterNot { it.id == approval.id } }
        return DecideOutcome.Decided(decided)
    }

    override suspend fun approval(id: String) = _approvals.value.firstOrNull { it.id == id }

    override suspend fun cancel(operationId: String, idempotencyKey: String): String? {
        _operations.update { ops -> ops.map { if (it.id == operationId) it.toBuilder().setState(OperationState.OPERATION_STATE_CANCELED).build() else it } }
        return null
    }

    override suspend fun registerPush(token: String, hpkePublicKey: ByteArray): Result<String> = Result.success("demo-push-target")

    override suspend fun sendTestNotification(): Result<String> = Result.failure(UnsupportedOperationException("demo mode has no server"))

    private fun ts(offsetMillis: Long): Timestamp = Timestamp.newBuilder().setSeconds((now() + offsetMillis) / 1000).build()

    private fun seedApprovals(): List<Approval> {
        val bob = Principal.newBuilder().setId("usr_bob").setKind(PrincipalKind.PRINCIPAL_KIND_USER).setDisplayName("Bob Example").build()
        val agent = Principal.newBuilder().setId("agt_reindexer").setKind(PrincipalKind.PRINCIPAL_KIND_AGENT).setDisplayName("reindex-agent").build()
        val prod = Environment.newBuilder().setId("env_prod").setProject("search").setName("production").setNamespace("prod").setProtected(true).build()
        val staging = Environment.newBuilder().setId("env_staging").setProject("search").setName("staging").setNamespace("staging").build()
        val one = ApprovalPolicy.newBuilder().setRequiredApprovals(1).build()
        return listOf(
            Approval.newBuilder().setId("apr_drop_logs").setRevision(1).setKind("collection.drop").setEnvironment(prod).setRequestedBy(bob)
                .setSummary("Drop collection logs-2026 in production")
                .addAllDetailLines(listOf("Namespace: prod", "Collection: logs-2026", "4.2 million documents, 18.3 GiB", "This cannot be undone."))
                .setRisk(Risk.RISK_DESTRUCTIVE).setPolicy(one).setState(ApprovalState.APPROVAL_STATE_PENDING)
                .setCreatedAt(ts(-20 * 60_000)).setExpiresAt(ts(72 * 3_600_000)).setConfirmText("logs-2026").build(),
            Approval.newBuilder().setId("apr_agent_reindex").setRevision(1).setKind("agent.action").setEnvironment(staging).setRequestedBy(agent)
                .addAllActorChain(listOf(agent, bob))
                .setSummary("reindex-agent wants to rebuild the products index in staging")
                .addAllDetailLines(listOf("Acting for Bob Example", "Tool: collections.reindex", "Estimated 6 minutes"))
                .setRisk(Risk.RISK_MEDIUM).setPolicy(one).setState(ApprovalState.APPROVAL_STATE_PENDING)
                .setCreatedAt(ts(-5 * 60_000)).setExpiresAt(ts(10 * 60_000)).build(),
        )
    }

    private fun seedOperations(): List<Operation> = listOf(
        Operation.newBuilder().setId("op_reindex").setKind("collection.reindex").setNamespace("staging")
            .setState(OperationState.OPERATION_STATE_RUNNING)
            .setProgress(Progress.newBuilder().setTotal(100).setDone(35).setFraction(0.35).setMessage("Rebuilding segments")).build(),
        Operation.newBuilder().setId("op_drop_logs").setKind("collection.drop").setNamespace("prod")
            .setState(OperationState.OPERATION_STATE_AWAITING_APPROVAL).setApprovalId("apr_drop_logs").build(),
        Operation.newBuilder().setId("op_backup").setKind("namespace.backup").setNamespace("prod")
            .setState(OperationState.OPERATION_STATE_SUCCEEDED).build(),
    )
}
