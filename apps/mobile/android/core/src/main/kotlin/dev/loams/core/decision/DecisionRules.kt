package dev.loams.core.decision

/** Why a decision cannot be sent as entered (design §37 §7.3, AP2 Task 6). */
enum class DecisionProblem {
    /** Rejecting needs a reason. */
    REASON_REQUIRED,

    /** A DESTRUCTIVE approval needs the target's name typed exactly. */
    CONFIRMATION_MISMATCH,

    /** Decisions are never queued: the stream must be live (AP2 Ruling 7). */
    OFFLINE,

    /** Only pending approvals can be decided. */
    NOT_PENDING,
}

object DecisionRules {
    fun check(
        decision: Decision,
        pending: Boolean,
        destructive: Boolean,
        confirmText: String,
        typed: String,
        reason: String,
        live: Boolean,
    ): DecisionProblem? = when {
        !pending -> DecisionProblem.NOT_PENDING
        !live -> DecisionProblem.OFFLINE
        decision == Decision.REJECT && reason.isBlank() -> DecisionProblem.REASON_REQUIRED
        decision == Decision.APPROVE && destructive && (confirmText.isEmpty() || typed.trim() != confirmText) ->
            DecisionProblem.CONFIRMATION_MISMATCH
        else -> null
    }
}
