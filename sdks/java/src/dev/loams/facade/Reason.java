package dev.loams.facade;

/**
 * The stable, machine-readable cause of a failed RPC (design §44 §7.4, D611).
 *
 * <p>It is generated from {@code docs/api/reasons.md} in a renderer that does not
 * exist yet, so this is the <b>Q604 hand-written-facade fallback</b> that design §44
 * §7.3 permits ("If the plugin proves too costly for a language, that language falls back
 * to a hand-written facade checked by the same conformance suite"). {@code
 * sdks/go/gen/facade/reason.go} is the same fallback in Go. See {@code README.md}.
 *
 * <p>It is an {@code enum}, not a string, which is the whole point: a caller switching on
 * it is exhaustive over the registry this SDK was built against, and a reason the registry
 * has lost <em>stops compiling</em> rather than silently becoming a default arm.
 *
 * <p>{@link #fromWire(String)} is how a value off the wire becomes one of these, and it
 * returns {@code null} for a reason this registry does not have. That case is a server
 * newer than this SDK, and the runtime surfaces it rather than dropping it — see R8 in
 * {@code docs/sdk/runtime-contract.md}.
 */
public enum Reason {
    APPROVAL_EXPIRED("approval_expired"),
    APPROVAL_ALREADY_DECIDED("approval_already_decided"),
    APPROVAL_STALE_REVISION("approval_stale_revision"),
    REQUESTER_CANNOT_APPROVE("requester_cannot_approve"),
    DECISION_PROOF_INVALID("decision_proof_invalid"),
    STEP_UP_REQUIRED("step_up_required"),
    REASON_REQUIRED("reason_required"),
    INVALID_DECISION("invalid_decision"),
    PAIRING_EXPIRED("pairing_expired"),
    PAIRING_USED("pairing_used"),
    DEVICE_REVOKED("device_revoked"),
    PUSH_TARGET_UNKNOWN("push_target_unknown"),
    NOT_IMPLEMENTED("not_implemented"),
    FEATURE_NOT_IN_VARIANT("feature_not_in_variant"),
    INVALID_ARGUMENT("invalid_argument"),
    NOT_FOUND("not_found"),
    ALREADY_EXISTS("already_exists"),
    PERMISSION_DENIED("permission_denied"),
    TOKEN_EXPIRED("token_expired"),
    UNAUTHENTICATED("unauthenticated"),
    FAILED_PRECONDITION("failed_precondition"),
    RESOURCE_EXHAUSTED("resource_exhausted"),
    UNAVAILABLE("unavailable"),
    DEADLINE_EXCEEDED("deadline_exceeded"),
    ABORTED("aborted"),
    INTERNAL("internal");

    private final String wire;

    Reason(String wire) {
        this.wire = wire;
    }

    /** The snake_case value on the wire, and in {@code docs/api/reasons.md}. */
    public String wire() {
        return wire;
    }

    @Override
    public String toString() {
        return wire;
    }

    /**
     * The reason a {@code reason} string names, or {@code null} when this SDK's registry
     * does not have it.
     *
     * <p>A {@code null} return means <em>the server is newer than this SDK</em>. It is not
     * an error and it is not the same as "no reason at all": a failure from below the API
     * carries no reason because no service answered, while this one carries a reason that
     * simply postdates the registry. Conflating them would leave a caller unable to tell
     * "not supported here" from "not supported at all" (R8).
     */
    public static Reason fromWire(String value) {
        if (value == null || value.isEmpty()) {
            return null;
        }
        for (Reason reason : VALUES) {
            if (reason.wire.equals(value)) {
                return reason;
            }
        }
        return null;
    }

    /** Every reason in the registry, in {@code docs/api/reasons.md}'s order. */
    public static Reason[] all() {
        return VALUES.clone();
    }

    private static final Reason[] VALUES = values();
}