package dev.loams.app.ui.approvals

import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import dev.loams.app.backend.Backend
import dev.loams.app.backend.DecideOutcome
import dev.loams.app.signing.DecisionSigner
import dev.loams.app.signing.SigningUnavailableException
import dev.loams.core.decision.Decision
import dev.loams.core.decision.DecisionClaims
import dev.loams.core.decision.DecisionProblem
import dev.loams.core.decision.DecisionRules
import dev.loams.core.decision.Jws
import dev.loams.core.errors.Reason
import dev.loams.core.watch.StreamStatus
import dev.loams.data.keys.SigningCancelledException
import dev.loams.proto.loams.approvals.v1.Approval
import dev.loams.proto.loams.approvals.v1.ApprovalState
import dev.loams.proto.loams.approvals.v1.Risk
import java.util.UUID
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch

/**
 * The approvals inbox (design §37 §7.3, AP2 Task 6). One ViewModel per screen, no shared store.
 * A decision is checked ([DecisionRules]), signed with the biometric-bound decision key, and sent
 * once; a send that fails for network reasons can be retried with the same idempotency key.
 */
class ApprovalsViewModel(
    private val backend: Backend,
    private val signer: DecisionSigner,
    private val clockMillis: () -> Long = System::currentTimeMillis,
) : ViewModel() {
    val approvals: StateFlow<List<Approval>> = backend.approvals
    val status: StateFlow<StreamStatus> = backend.approvalsStatus

    private val _busy = MutableStateFlow<String?>(null)
    val busy: StateFlow<String?> = _busy.asStateFlow()

    private val _message = MutableStateFlow<String?>(null)
    val message: StateFlow<String?> = _message.asStateFlow()

    /** Idempotency keys by (approval, revision, decision), reused by Retry (AP2 Ruling 7). */
    private val attempts = mutableMapOf<String, String>()

    /** Holds the stream open while the screen is visible. */
    suspend fun watch() = backend.watchApprovals()

    fun dismissMessage() {
        _message.value = null
    }

    fun decide(approval: Approval, decision: Decision, reason: String, typed: String) {
        viewModelScope.launch { _message.value = decideNow(approval, decision, reason, typed) }
    }

    /** Returns the message to show. */
    suspend fun decideNow(approval: Approval, decision: Decision, reason: String, typed: String): String {
        val problem = DecisionRules.check(
            decision = decision,
            pending = approval.state == ApprovalState.APPROVAL_STATE_PENDING,
            destructive = approval.risk == Risk.RISK_DESTRUCTIVE,
            confirmText = approval.confirmText,
            typed = typed,
            reason = reason,
            live = status.value == StreamStatus.Live,
        )
        if (problem != null) return messageFor(problem, approval)

        _busy.value = approval.id
        try {
            val claims = DecisionClaims(approval.id, approval.revision, decision, clockMillis() / 1000, UUID.randomUUID().toString())
            val input = Jws.signingInput(Jws.DECISION_HEADER, claims.canonicalJson())
            val verb = if (decision == Decision.APPROVE) "Approve" else "Reject"
            val raw = try {
                signer.sign(backend.instanceId, input.toByteArray(Charsets.US_ASCII), "$verb ${approval.kind}", "in ${approval.environment.name}")
            } catch (e: SigningCancelledException) {
                return "Not signed: ${e.message}"
            } catch (e: SigningUnavailableException) {
                return e.message ?: "This phone cannot sign decisions."
            }
            val key = attempts.getOrPut("${approval.id}:${approval.revision}:$decision") { UUID.randomUUID().toString() }
            return when (val outcome = backend.decide(approval, decision, reason.trim(), Jws.compact(input, raw), key)) {
                is DecideOutcome.Decided -> {
                    attempts.remove("${approval.id}:${approval.revision}:$decision")
                    if (decision == Decision.APPROVE) "Approved: ${approval.summary}" else "Rejected: ${approval.summary}"
                }
                is DecideOutcome.NotSent -> "${outcome.message}. Tap ${verb.lowercase()} again to retry; it will not be applied twice."
                is DecideOutcome.Refused -> when (outcome.reason) {
                    Reason.APPROVAL_ALREADY_DECIDED -> "Someone already decided this approval."
                    Reason.APPROVAL_EXPIRED -> "This approval expired."
                    Reason.STEP_UP_REQUIRED, Reason.DECISION_PROOF_INVALID ->
                        "The server did not accept this phone's signature. Pair the phone again."
                    else -> "Refused: ${outcome.message}"
                }
            }
        } finally {
            _busy.value = null
        }
    }

    companion object {
        fun messageFor(problem: DecisionProblem, approval: Approval): String = when (problem) {
            DecisionProblem.REASON_REQUIRED -> "Say why you are rejecting it."
            DecisionProblem.CONFIRMATION_MISMATCH -> "Type ${approval.confirmText} to confirm."
            DecisionProblem.OFFLINE -> "Offline: decisions need a live connection and are never queued."
            DecisionProblem.NOT_PENDING -> "This approval is no longer pending."
        }
    }
}
