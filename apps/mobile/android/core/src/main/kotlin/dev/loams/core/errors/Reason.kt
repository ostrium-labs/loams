package dev.loams.core.errors

/**
 * The stable `loams.errors.v1.ErrorInfo.reason` values (AP0 Ruling 6). Apps branch on these,
 * never on message text. A reason this build does not know maps to [UNKNOWN].
 */
enum class Reason(val wire: String) {
    APPROVAL_EXPIRED("approval_expired"),
    APPROVAL_ALREADY_DECIDED("approval_already_decided"),
    DECISION_PROOF_INVALID("decision_proof_invalid"),
    STEP_UP_REQUIRED("step_up_required"),
    PAIRING_EXPIRED("pairing_expired"),
    PAIRING_USED("pairing_used"),
    DEVICE_REVOKED("device_revoked"),
    PUSH_TARGET_UNKNOWN("push_target_unknown"),
    UNKNOWN(""),
    ;

    companion object {
        private val byWire = entries.associateBy { it.wire }

        fun fromWire(reason: String?): Reason = byWire[reason.orEmpty()]?.takeIf { it != UNKNOWN } ?: UNKNOWN
    }
}
