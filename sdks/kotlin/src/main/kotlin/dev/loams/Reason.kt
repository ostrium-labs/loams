package dev.loams

/**
 * The `ErrorInfo.reason` registry (design §44 §7.4, D611; runtime contract R8).
 *
 * A failed RPC carries a Connect code and one `loams.errors.v1.ErrorInfo` in its
 * details. The **code** gives the class and the **`reason`** is the branch: a stable
 * `snake_case` string, registered in `docs/api/reasons.md` and never renamed within
 * an API major version. A caller branches on [Reason], never on the message, which
 * may change.
 *
 * Three states are deliberately distinct rather than collapsed (R8):
 *
 *  - [Reason.NONE] — the failure came from **below the API** (a socket, a timeout, a
 *    cancelled call) or the server sent no `ErrorInfo`. It carries no reason, which
 *    is not the same thing as any reason in the registry;
 *  - a reason off the registry, from a server newer than this SDK: it is surfaced as
 *    the **text** in [LoamsException.unknownReason] rather than dropped, because
 *    dropping it leaves a caller unable to tell "not supported here" from "not
 *    supported at all";
 *  - a reason in the registry, which is [LoamsException.reason].
 *
 * # Provenance
 *
 * Hand-written, and it is the same hand-written answer `sdks/go` and `sdks/java`
 * give: design §44 §7.3's **Q604 fallback**, which permits a hand-written facade
 * checked by the same conformance suite where the generator is too costly for a
 * language. **A registry cannot be derived** the way a facade can: the wire names are
 * `snake_case` strings with no proto to read them from, and the reason a registry
 * can be written out and a binding table cannot is that the registry is the
 * *authority* — `docs/api/reasons.md` — rather than a copy of something else.
 * `kotlin_the_reason_registry_matches_the_registry_page` reads that page and fails on
 * a drift, so this enum cannot quietly fall behind it. **Do not add a reason here
 * without adding its row to that page first.**
 */
enum class Reason {
    /**
     * The failure carried no reason: it came from below the API, or the server sent
     * an error with no `ErrorInfo`.
     *
     * Distinct from every registry reason, and R8 requires it to stay distinct —
     * conflating it with [TOKEN_EXPIRED] would make a client refresh a request that
     * has no credential to refresh.
     */
    NONE,

    /** The approval was still pending and is no longer. */
    APPROVAL_EXPIRED,

    /** The approval was already decided, by this or another caller. */
    APPROVAL_ALREADY_DECIDED,

    /** The decision named a revision the server is not on. */
    APPROVAL_STALE_REVISION,

    /** An agent requested the operation, so its requester cannot approve it. */
    REQUESTER_CANNOT_APPROVE,

    /** The decision proof did not verify against the paired device key. */
    DECISION_PROOF_INVALID,

    /**
     * The credential is valid but too old for this decision. R1's refresh trigger;
     * distinct from [UNAUTHENTICATED].
     */
    STEP_UP_REQUIRED,

    /** Rejecting, or approving something destructive, needs a reason. */
    REASON_REQUIRED,

    /** The decision is not one the policy allows. */
    INVALID_DECISION,

    /** The pairing grant is past its expiry. */
    PAIRING_EXPIRED,

    /** The pairing grant was already redeemed; it is single-use. */
    PAIRING_USED,

    /** The device the token is bound to has been revoked. */
    DEVICE_REVOKED,

    /** The push target the request named is not registered. */
    PUSH_TARGET_UNKNOWN,

    /** A stub handler whose service has not landed yet. */
    NOT_IMPLEMENTED,

    /**
     * A catalogue package whose engine is not in this build variant (design §44 §4,
     * D600). The variant is in `metadata.variant`.
     */
    FEATURE_NOT_IN_VARIANT,

    /** A malformed field or an unparseable value. */
    INVALID_ARGUMENT,

    /** The named resource does not exist. */
    NOT_FOUND,

    /** The named resource already exists. */
    ALREADY_EXISTS,

    /** The caller's role may not make this call. */
    PERMISSION_DENIED,

    /**
     * A rejected access token, which R1 refreshes once and retries once. A second
     * expiry reaches the caller as a [TokenExpiredException].
     */
    TOKEN_EXPIRED,

    /** No credential, or one that cannot be used. */
    UNAUTHENTICATED,

    /** The call's preconditions do not hold. */
    FAILED_PRECONDITION,

    /** A backpressure or quota refusal. */
    RESOURCE_EXHAUSTED,

    /** A dependency is down, or this node cannot serve the read. */
    UNAVAILABLE,

    /** The caller's deadline passed. */
    DEADLINE_EXCEEDED,

    /** A concurrent write won; the caller retries. */
    ABORTED,

    /** A bug in the server. The message and the request id go to the log. */
    INTERNAL,
}

/**
 * The registry itself: the wire name of every [Reason], the Connect code each is
 * raised under, and the reverse lookup.
 *
 * Three lookups, and they answer different questions. [fromWire] answers "what did
 * the server say" and returns null for a reason this SDK does not have, which the
 * runtime surfaces as `unknownReason` rather than dropping. [name] answers "what do I
 * write in a log" and returns the empty string for [Reason.NONE], because a failure
 * with no reason has no reason to print. [codeOf] answers "what class is it", which
 * is what the retry policy and the error hierarchy branch on.
 */
object ReasonRegistry {
    /**
     * The registry in the registry's own order, which is the order of the table in
     * `docs/api/reasons.md`: the AP0-specific causes first, then the generic
     * code-to-class rows.
     *
     * Kept as one ordered list rather than derived from a map, because "the
     * registry's order" is a property [all] promises and a hash map does not have
     * one.
     */
    private val IN_REGISTRY_ORDER: List<Reason> = listOf(
        Reason.APPROVAL_EXPIRED,
        Reason.APPROVAL_ALREADY_DECIDED,
        Reason.APPROVAL_STALE_REVISION,
        Reason.REQUESTER_CANNOT_APPROVE,
        Reason.DECISION_PROOF_INVALID,
        Reason.STEP_UP_REQUIRED,
        Reason.REASON_REQUIRED,
        Reason.INVALID_DECISION,
        Reason.PAIRING_EXPIRED,
        Reason.PAIRING_USED,
        Reason.DEVICE_REVOKED,
        Reason.PUSH_TARGET_UNKNOWN,
        Reason.NOT_IMPLEMENTED,
        Reason.FEATURE_NOT_IN_VARIANT,
        Reason.INVALID_ARGUMENT,
        Reason.NOT_FOUND,
        Reason.ALREADY_EXISTS,
        Reason.PERMISSION_DENIED,
        Reason.TOKEN_EXPIRED,
        Reason.UNAUTHENTICATED,
        Reason.FAILED_PRECONDITION,
        Reason.RESOURCE_EXHAUSTED,
        Reason.UNAVAILABLE,
        Reason.DEADLINE_EXCEEDED,
        Reason.ABORTED,
        Reason.INTERNAL,
    )

    private val NAMES: Map<Reason, String> = mapOf(
        Reason.APPROVAL_EXPIRED to "approval_expired",
        Reason.APPROVAL_ALREADY_DECIDED to "approval_already_decided",
        Reason.APPROVAL_STALE_REVISION to "approval_stale_revision",
        Reason.REQUESTER_CANNOT_APPROVE to "requester_cannot_approve",
        Reason.DECISION_PROOF_INVALID to "decision_proof_invalid",
        Reason.STEP_UP_REQUIRED to "step_up_required",
        Reason.REASON_REQUIRED to "reason_required",
        Reason.INVALID_DECISION to "invalid_decision",
        Reason.PAIRING_EXPIRED to "pairing_expired",
        Reason.PAIRING_USED to "pairing_used",
        Reason.DEVICE_REVOKED to "device_revoked",
        Reason.PUSH_TARGET_UNKNOWN to "push_target_unknown",
        Reason.NOT_IMPLEMENTED to "not_implemented",
        Reason.FEATURE_NOT_IN_VARIANT to "feature_not_in_variant",
        Reason.INVALID_ARGUMENT to "invalid_argument",
        Reason.NOT_FOUND to "not_found",
        Reason.ALREADY_EXISTS to "already_exists",
        Reason.PERMISSION_DENIED to "permission_denied",
        Reason.TOKEN_EXPIRED to "token_expired",
        Reason.UNAUTHENTICATED to "unauthenticated",
        Reason.FAILED_PRECONDITION to "failed_precondition",
        Reason.RESOURCE_EXHAUSTED to "resource_exhausted",
        Reason.UNAVAILABLE to "unavailable",
        Reason.DEADLINE_EXCEEDED to "deadline_exceeded",
        Reason.ABORTED to "aborted",
        Reason.INTERNAL to "internal",
    )

    private val BY_WIRE: Map<String, Reason> = NAMES.entries.associate { (reason, wire) -> wire to reason }

    /**
     * The rows above `invalid_argument` in `docs/api/reasons.md` are AP0's specific
     * causes and do not map to the code named in their own tail; the rows below it are
     * the generic code-to-class rows and map to the code their name carries.
     */
    private val CODES: Map<Reason, Code> = mapOf(
        Reason.APPROVAL_EXPIRED to Code.FAILED_PRECONDITION,
        Reason.APPROVAL_ALREADY_DECIDED to Code.FAILED_PRECONDITION,
        Reason.APPROVAL_STALE_REVISION to Code.FAILED_PRECONDITION,
        Reason.REQUESTER_CANNOT_APPROVE to Code.PERMISSION_DENIED,
        Reason.DECISION_PROOF_INVALID to Code.PERMISSION_DENIED,
        Reason.STEP_UP_REQUIRED to Code.UNAUTHENTICATED,
        Reason.REASON_REQUIRED to Code.INVALID_ARGUMENT,
        Reason.INVALID_DECISION to Code.INVALID_ARGUMENT,
        Reason.PAIRING_EXPIRED to Code.FAILED_PRECONDITION,
        Reason.PAIRING_USED to Code.FAILED_PRECONDITION,
        Reason.DEVICE_REVOKED to Code.UNAUTHENTICATED,
        Reason.PUSH_TARGET_UNKNOWN to Code.NOT_FOUND,
        Reason.NOT_IMPLEMENTED to Code.UNIMPLEMENTED,
        Reason.FEATURE_NOT_IN_VARIANT to Code.UNIMPLEMENTED,
        Reason.INVALID_ARGUMENT to Code.INVALID_ARGUMENT,
        Reason.NOT_FOUND to Code.NOT_FOUND,
        Reason.ALREADY_EXISTS to Code.ALREADY_EXISTS,
        Reason.PERMISSION_DENIED to Code.PERMISSION_DENIED,
        Reason.TOKEN_EXPIRED to Code.UNAUTHENTICATED,
        Reason.UNAUTHENTICATED to Code.UNAUTHENTICATED,
        Reason.FAILED_PRECONDITION to Code.FAILED_PRECONDITION,
        Reason.RESOURCE_EXHAUSTED to Code.RESOURCE_EXHAUSTED,
        Reason.UNAVAILABLE to Code.UNAVAILABLE,
        Reason.DEADLINE_EXCEEDED to Code.DEADLINE_EXCEEDED,
        Reason.ABORTED to Code.ABORTED,
        Reason.INTERNAL to Code.INTERNAL,
    )

    /** Every reason in the registry, in the registry's order. */
    val all: List<Reason> get() = IN_REGISTRY_ORDER

    /** Every registry reason's wire name, in the registry's order. */
    val allNames: List<String> get() = IN_REGISTRY_ORDER.map { name(it) }

    /** The stable `snake_case` wire name, or the empty string for [Reason.NONE]. */
    fun name(reason: Reason): String = NAMES[reason] ?: ""

    /**
     * The reason a wire string names, or null for one this SDK's registry does not
     * have — which means the server is newer, and the caller must be told rather than
     * handed a default.
     */
    fun fromWire(wire: String): Reason? = BY_WIRE[wire]

    /** Whether a wire string is in this SDK's registry. */
    fun isKnown(wire: String): Boolean = BY_WIRE.containsKey(wire)

    /** The Connect code a reason is raised under, from the same registry. */
    fun codeOf(reason: Reason): Code = CODES[reason] ?: Code.UNKNOWN
}