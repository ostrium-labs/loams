package dev.loams.facade;

import java.util.Arrays;
import java.util.Collections;
import java.util.EnumMap;
import java.util.List;
import java.util.Map;
import java.util.Optional;

/**
 * The reason registry as the runtime sees it: every reason, the Connect code each one is
 * raised under, and the lookup the error mapper needs.
 *
 * <p>The codes come from {@code docs/api/reasons.md} and are the same in every SDK, so a
 * caller that wants the coarse class has it without parsing anything.
 *
 * <p>Provenance is the Q604 hand-written-facade fallback; see {@link Reason}.
 */
public final class ReasonCodes {

    private ReasonCodes() {}

    private static final Map<Reason, String> CODES = codes();

    private static Map<Reason, String> codes() {
        Map<Reason, String> map = new EnumMap<>(Reason.class);
        map.put(Reason.APPROVAL_EXPIRED, "failed_precondition");
        map.put(Reason.APPROVAL_ALREADY_DECIDED, "failed_precondition");
        map.put(Reason.APPROVAL_STALE_REVISION, "failed_precondition");
        map.put(Reason.REQUESTER_CANNOT_APPROVE, "permission_denied");
        map.put(Reason.DECISION_PROOF_INVALID, "permission_denied");
        map.put(Reason.STEP_UP_REQUIRED, "unauthenticated");
        map.put(Reason.REASON_REQUIRED, "invalid_argument");
        map.put(Reason.INVALID_DECISION, "invalid_argument");
        map.put(Reason.PAIRING_EXPIRED, "failed_precondition");
        map.put(Reason.PAIRING_USED, "failed_precondition");
        map.put(Reason.DEVICE_REVOKED, "unauthenticated");
        map.put(Reason.PUSH_TARGET_UNKNOWN, "not_found");
        map.put(Reason.NOT_IMPLEMENTED, "unimplemented");
        map.put(Reason.FEATURE_NOT_IN_VARIANT, "unimplemented");
        map.put(Reason.INVALID_ARGUMENT, "invalid_argument");
        map.put(Reason.NOT_FOUND, "not_found");
        map.put(Reason.ALREADY_EXISTS, "already_exists");
        map.put(Reason.PERMISSION_DENIED, "permission_denied");
        map.put(Reason.TOKEN_EXPIRED, "unauthenticated");
        map.put(Reason.UNAUTHENTICATED, "unauthenticated");
        map.put(Reason.FAILED_PRECONDITION, "failed_precondition");
        map.put(Reason.RESOURCE_EXHAUSTED, "resource_exhausted");
        map.put(Reason.UNAVAILABLE, "unavailable");
        map.put(Reason.DEADLINE_EXCEEDED, "deadline_exceeded");
        map.put(Reason.ABORTED, "aborted");
        map.put(Reason.INTERNAL, "internal");
        return Collections.unmodifiableMap(map);
    }

    /**
     * The Connect code a reason is raised under, or {@code null} for a reason this registry
     * does not have — in which case the wire's own code is the classification, and losing it
     * would be worse than not knowing the reason.
     */
    public static String of(Reason reason) {
        return reason == null ? null : CODES.get(reason);
    }

    /** The reason registry's wire strings, in registry order. */
    public static List<String> wireValues() {
        return Arrays.stream(Reason.all()).map(Reason::wire).toList();
    }

    /**
     * Every reason the registry has, or a reason it does not have, which is the registry's
     * whole vocabulary in one call. The error-reason test iterates it rather than a
     * hand-written list, so a reason added to {@link Reason} is covered the day it lands.
     */
    public static Optional<Reason> lookup(String wire) {
        return Optional.ofNullable(Reason.fromWire(wire));
    }
}