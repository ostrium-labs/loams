package dev.loams.app.backend

import com.connectrpc.Code
import com.connectrpc.ConnectException
import com.connectrpc.ResponseMessage
import com.google.protobuf.ByteString
import dev.loams.core.decision.Decision
import dev.loams.core.errors.Reason
import dev.loams.core.watch.ResumingWatch
import dev.loams.core.watch.StreamStatus
import dev.loams.core.watch.WatchState
import dev.loams.proto.loams.approvals.v1.Approval
import dev.loams.proto.loams.approvals.v1.DecideApprovalRequest
import dev.loams.proto.loams.approvals.v1.DecisionKind
import dev.loams.proto.loams.approvals.v1.GetApprovalRequest
import dev.loams.proto.loams.devices.v1.PushProvider
import dev.loams.proto.loams.devices.v1.RegisterPushTargetRequest
import dev.loams.proto.loams.devices.v1.SendTestNotificationRequest
import dev.loams.proto.loams.instance.v1.GetInstanceRequest
import dev.loams.proto.loams.operations.v1.CancelOperationRequest
import dev.loams.proto.loams.operations.v1.Operation
import dev.loams.transport.Clients
import dev.loams.transport.TrustPolicy
import dev.loams.transport.reason
import dev.loams.transport.watchApprovals
import dev.loams.transport.watchOperations
import java.io.IOException
import java.util.UUID
import dev.loams.core.watch.WatchEvent
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.emitAll
import kotlinx.coroutines.flow.flow

/** A paired instance over Connect. */
class RemoteBackend(
    private val clients: Clients,
    override val instanceId: String,
    private val deviceId: String,
    private val trust: TrustPolicy,
    /** Gets a fresh access token: true to retry, false when the session is gone; may throw. */
    private val reauthenticate: suspend () -> Boolean = { false },
) : Backend {
    override val isDemo = false

    private val approvalState = WatchState<Approval> { it.id }
    private val operationState = WatchState<Operation> { it.id }

    // A revoked device or a lost session will not heal by retrying.
    private val fatal: (Throwable) -> Boolean = { it is ConnectException && it.code == Code.UNAUTHENTICATED }
    private val approvalWatch = ResumingWatch(approvalState, withReauth { clients.watchApprovals(it) }, isFatal = fatal)
    private val operationWatch = ResumingWatch(operationState, withReauth { clients.watchOperations(it) }, isFatal = fatal)

    /** Signed in again after UNAUTHENTICATED: a retryable failure, so the watch reconnects. */
    private class Reauthenticated : Exception("signed in again")

    private fun <T> withReauth(open: (String?) -> Flow<WatchEvent<T>>): (String?) -> Flow<WatchEvent<T>> = { cursor ->
        flow {
            try {
                emitAll(open(cursor))
            } catch (e: ConnectException) {
                if (e.code == Code.UNAUTHENTICATED && e.reason() != Reason.DEVICE_REVOKED && reauthenticate()) throw Reauthenticated()
                throw e
            }
        }
    }

    override val approvals: StateFlow<List<Approval>> = approvalState.items
    override val approvalsStatus: StateFlow<StreamStatus> = approvalWatch.status
    override val operations: StateFlow<List<Operation>> = operationState.items
    override val operationsStatus: StateFlow<StreamStatus> = operationWatch.status

    override suspend fun watchApprovals() = approvalWatch.run()

    override suspend fun watchOperations() = operationWatch.run()

    override suspend fun instance(): InstanceSummary {
        val i = clients.instance.getInstance(GetInstanceRequest.getDefaultInstance()).orThrow()
        return InstanceSummary(
            issuer = clients.baseUrl,
            instanceId = i.instanceId,
            serverVersion = i.serverVersion,
            edition = i.edition.name.removePrefix("EDITION_"),
            apiVersions = i.apiVersionsList,
            deviceId = deviceId,
            trust = when (trust) {
                is TrustPolicy.Pinned -> "Pinned TLS key (${trust.spki.size} pins) + instance key"
                TrustPolicy.System -> "System CAs + instance key"
            },
        )
    }

    override suspend fun decide(approval: Approval, decision: Decision, reason: String, proof: String, idempotencyKey: String): DecideOutcome {
        val request = DecideApprovalRequest.newBuilder()
            .setApprovalId(approval.id)
            .setRevision(approval.revision)
            .setDecision(if (decision == Decision.APPROVE) DecisionKind.DECISION_KIND_APPROVE else DecisionKind.DECISION_KIND_REJECT)
            .setReason(reason)
            .setDecisionProof(proof)
            .setIdempotencyKey(idempotencyKey)
            .build()
        return when (val r = clients.approvals.decideApproval(request)) {
            is ResponseMessage.Success -> DecideOutcome.Decided(r.message.approval)
            is ResponseMessage.Failure -> {
                val e = r.cause
                if (e.code == Code.UNAVAILABLE || e.code == Code.DEADLINE_EXCEEDED || e.exception is IOException) {
                    DecideOutcome.NotSent("Not sent: ${e.message ?: "network error"}")
                } else {
                    DecideOutcome.Refused(e.reason(), e.message ?: e.code.codeName)
                }
            }
        }
    }

    override suspend fun approval(id: String): Approval? =
        (clients.approvals.getApproval(GetApprovalRequest.newBuilder().setApprovalId(id).build()) as? ResponseMessage.Success)?.message?.approval

    override suspend fun cancel(operationId: String, idempotencyKey: String): String? =
        when (val r = clients.operations.cancelOperation(CancelOperationRequest.newBuilder().setOperationId(operationId).setIdempotencyKey(idempotencyKey).build())) {
            is ResponseMessage.Success -> null
            is ResponseMessage.Failure -> r.cause.message ?: r.cause.code.codeName
        }

    override suspend fun registerPush(token: String, hpkePublicKey: ByteArray): Result<String> = runCatching {
        clients.devices.registerPushTarget(
            RegisterPushTargetRequest.newBuilder()
                .setProvider(PushProvider.PUSH_PROVIDER_FCM)
                .setTokenOrEndpoint(token)
                .setAppId("dev.loams.app")
                .setHpkePublicKey(ByteString.copyFrom(hpkePublicKey))
                .setIdempotencyKey(UUID.randomUUID().toString())
                .build(),
        ).orThrow().pushTargetId
    }

    override suspend fun sendTestNotification(): Result<String> = runCatching {
        clients.devices.sendTestNotification(SendTestNotificationRequest.newBuilder().setIdempotencyKey(UUID.randomUUID().toString()).build())
            .orThrow().notificationId
    }

    private fun <T> ResponseMessage<T>.orThrow(): T = when (this) {
        is ResponseMessage.Success -> message
        is ResponseMessage.Failure -> throw cause
    }
}

/** Fatal for the session: the device was revoked or its tokens are gone. */
fun Throwable.isRevoked(): Boolean = this is ConnectException && reason() == Reason.DEVICE_REVOKED
