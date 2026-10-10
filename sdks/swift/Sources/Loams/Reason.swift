// The reason registry (design §44 §7.4, D611; runtime contract R8).
//
// A failed RPC carries a Connect code and one `loams.errors.v1.ErrorInfo`. The
// **code** gives the class — a taxonomy that does not change within a major
// version — and the **`reason`** is the stable branch. A reason is a type in the
// SDK, generated from `docs/api/reasons.md`, so a caller switching on it is
// exhaustive and a reason the registry has lost stops compiling. The `message`
// is for people, may change, and is never branched on.
//
// **Provenance.** Hand-written with the rest of the facade, for the reason
// `Facade.swift`'s header gives: there is no Swift renderer. Transcribed from
// `docs/api/reasons.md` and from `sdks/go/gen/facade/reason.go`.
//
// Swift gets exhaustiveness from an `enum` rather than from a string type, which
// is a better fit than Go's and worse than TypeScript's discriminated union: an
// `enum Reason: String` makes a `switch` over it require a `default`, so the
// compiler will *not* catch a reason added to the registry later. That is why
// ``loamsAllReasons`` exists and why ``ConformanceTests`` asserts that the switch
// in ``loamsReason(from:)`` covers every case — the coverage is pinned by a test
// rather than by the compiler, and this comment says so rather than leaving a
// reader to assume exhaustiveness they do not have.

import Foundation

/// A stable, machine-readable cause of a failed RPC.
///
/// It is a distinct type rather than `String` on purpose: that is what makes a
/// caller's `switch` meaningful, and it is why a wire value has to go through
/// ``loamsReason(from:)`` before it becomes a `Reason`.
public enum Reason: String, Sendable, Hashable, CaseIterable {
    case approvalExpired = "approval_expired"
    case approvalAlreadyDecided = "approval_already_decided"
    case approvalStaleRevision = "approval_stale_revision"
    case requesterCannotApprove = "requester_cannot_approve"
    case decisionProofInvalid = "decision_proof_invalid"
    case stepUpRequired = "step_up_required"
    case reasonRequired = "reason_required"
    case invalidDecision = "invalid_decision"
    case pairingExpired = "pairing_expired"
    case pairingUsed = "pairing_used"
    case deviceRevoked = "device_revoked"
    case pushTargetUnknown = "push_target_unknown"
    case notImplemented = "not_implemented"
    case featureNotInVariant = "feature_not_in_variant"
    case invalidArgument = "invalid_argument"
    case notFound = "not_found"
    case alreadyExists = "already_exists"
    case permissionDenied = "permission_denied"
    case tokenExpired = "token_expired"
    case unauthenticated = "unauthenticated"
    case failedPrecondition = "failed_precondition"
    case resourceExhausted = "resource_exhausted"
    case unavailable = "unavailable"
    case deadlineExceeded = "deadline_exceeded"
    case aborted = "aborted"
    case internalReason = "internal"

    /// The reason a package this build variant does not carry answers with
    /// (design §44 §4, D600).
    ///
    /// Every RPC of such a package refuses with it and names the variant in
    /// `metadata.variant`; the runtime turns that refusal into a
    /// ``FeatureNotInVariantError``, so a caller can branch on the type — or on
    /// this reason — without having to call and read the message.
    public static let featureNotInVariantReason = Reason.featureNotInVariant
}

/// Every reason in the registry, in the registry's order.
///
/// The test `testReasonRegistryIsExhaustive` reads this and fails on a drift, so
/// the set cannot silently fall behind `docs/api/reasons.md`.
public let loamsAllReasons: [Reason] = Reason.allCases

/// The Connect code each reason is raised under, from the same registry.
///
/// A caller that wants the coarse class has it without string parsing, and the
/// two the SDK's own logic branches on — `feature_not_in_variant` and
/// `token_expired` — are the ones R5 and R1 name.
public let loamsReasonCodes: [Reason: Code] = [
    .approvalExpired: .failedPrecondition,
    .approvalAlreadyDecided: .failedPrecondition,
    .approvalStaleRevision: .failedPrecondition,
    .requesterCannotApprove: .permissionDenied,
    .decisionProofInvalid: .permissionDenied,
    .stepUpRequired: .unauthenticated,
    .reasonRequired: .invalidArgument,
    .invalidDecision: .invalidArgument,
    .pairingExpired: .failedPrecondition,
    .pairingUsed: .failedPrecondition,
    .deviceRevoked: .unauthenticated,
    .pushTargetUnknown: .notFound,
    .notImplemented: .unimplemented,
    .featureNotInVariant: .unimplemented,
    .invalidArgument: .invalidArgument,
    .notFound: .notFound,
    .alreadyExists: .alreadyExists,
    .permissionDenied: .permissionDenied,
    .tokenExpired: .unauthenticated,
    .unauthenticated: .unauthenticated,
    .failedPrecondition: .failedPrecondition,
    .resourceExhausted: .resourceExhausted,
    .unavailable: .unavailable,
    .deadlineExceeded: .deadlineExceeded,
    .aborted: .aborted,
    .internalReason: .internal,
]

/// A wire reason string as a ``Reason``, or `nil` when this SDK's registry does
/// not have it.
///
/// `nil` is the **server is newer than this SDK** case, and it is why the wire
/// value is kept alongside: losing it would leave a caller unable to tell "not
/// supported here" from "not supported at all" (R8). The runtime surfaces it as
/// ``LoamsError/unknownReason`` rather than dropping it.
public func loamsReason(from value: String) -> Reason? {
    Reason(rawValue: value)
}

/// Whether a reason is in this SDK's registry.
public func loamsIsKnownReason(_ reason: Reason) -> Bool {
    loamsReason(from: reason.rawValue) != nil
}