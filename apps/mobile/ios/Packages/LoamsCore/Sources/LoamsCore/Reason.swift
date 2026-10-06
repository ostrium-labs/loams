/// The stable `loams.errors.v1.ErrorInfo.reason` values (AP0 Ruling 6).
public enum Reason: String, Sendable, CaseIterable {
    case approvalExpired = "approval_expired"
    case approvalAlreadyDecided = "approval_already_decided"
    case decisionProofInvalid = "decision_proof_invalid"
    case stepUpRequired = "step_up_required"
    case pairingExpired = "pairing_expired"
    case pairingUsed = "pairing_used"
    case deviceRevoked = "device_revoked"
    case pushTargetUnknown = "push_target_unknown"
    case unknown = ""

    public static func fromWire(_ reason: String?) -> Reason {
        guard let r = reason, let known = Reason(rawValue: r), known != .unknown else { return .unknown }
        return known
    }
}
