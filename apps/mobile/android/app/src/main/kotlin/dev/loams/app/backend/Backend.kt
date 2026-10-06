package dev.loams.app.backend

import dev.loams.core.decision.Decision
import dev.loams.core.errors.Reason
import dev.loams.core.watch.StreamStatus
import dev.loams.proto.loams.approvals.v1.Approval
import dev.loams.proto.loams.operations.v1.Operation
import kotlinx.coroutines.flow.StateFlow

/** What the instance says about itself, for the status screen. */
data class InstanceSummary(
    val issuer: String,
    val instanceId: String,
    val serverVersion: String,
    val edition: String,
    val apiVersions: List<String>,
    val deviceId: String,
    val trust: String,
)

sealed interface DecideOutcome {
    data class Decided(val approval: Approval) : DecideOutcome

    /** The server refused; [reason] is the stable ErrorInfo reason. */
    data class Refused(val reason: Reason, val message: String) : DecideOutcome

    /** It may not have reached the server; retry reuses the same idempotency key. */
    data class NotSent(val message: String) : DecideOutcome
}

/**
 * Everything the screens need from an instance. [RemoteBackend] talks Connect to a server (the
 * mock today); [DemoBackend] is in-memory, for demo mode (Q435), previews and UI tests.
 */
interface Backend {
    val instanceId: String
    val isDemo: Boolean

    val approvals: StateFlow<List<Approval>>
    val approvalsStatus: StateFlow<StreamStatus>
    val operations: StateFlow<List<Operation>>
    val operationsStatus: StateFlow<StreamStatus>

    /** Hold the approvals stream open; returns when cancelled (the screen left or the app stopped). */
    suspend fun watchApprovals()

    suspend fun watchOperations()

    suspend fun instance(): InstanceSummary

    suspend fun decide(approval: Approval, decision: Decision, reason: String, proof: String, idempotencyKey: String): DecideOutcome

    suspend fun approval(id: String): Approval?

    /** Null on success, else a message. */
    suspend fun cancel(operationId: String, idempotencyKey: String): String?

    suspend fun registerPush(token: String, hpkePublicKey: ByteArray): Result<String>

    suspend fun sendTestNotification(): Result<String>
}
