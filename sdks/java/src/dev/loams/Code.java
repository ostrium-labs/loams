package dev.loams;

import java.util.Arrays;
import java.util.Optional;

/**
 * The Connect code: the canonical classification of a failure, which does not change within a
 * major version (design §44 §7.4, D611).
 *
 * <p>It is the coarse half of a failure. The fine half is the {@link
 * dev.loams.facade.Reason reason}, and a caller branches on the reason rather than on this —
 * but the code is what the retry policy and the exception hierarchy are built from, so it is
 * part of the SDK's public surface.
 */
public enum Code {
    /** The call was cancelled, usually by the caller's own deadline or interrupt. */
    CANCELED("canceled"),
    /** The failure came from below the API, or from a code the SDK does not name. */
    UNKNOWN("unknown"),
    /** The client specified an invalid argument. */
    INVALID_ARGUMENT("invalid_argument"),
    /** The deadline expired before the operation could complete. */
    DEADLINE_EXCEEDED("deadline_exceeded"),
    /** A requested entity was not found. */
    NOT_FOUND("not_found"),
    /** An entity the client attempted to create already exists. */
    ALREADY_EXISTS("already_exists"),
    /** The caller does not have permission to execute the operation. */
    PERMISSION_DENIED("permission_denied"),
    /** A resource has been exhausted, for example a per-user quota. */
    RESOURCE_EXHAUSTED("resource_exhausted"),
    /** The system is not in a state required for the operation's execution. */
    FAILED_PRECONDITION("failed_precondition"),
    /** The operation was aborted, typically due to a concurrency issue. */
    ABORTED("aborted"),
    /** The operation was attempted past the valid range. */
    OUT_OF_RANGE("out_of_range"),
    /** The operation is not implemented or is not supported in this build. */
    UNIMPLEMENTED("unimplemented"),
    /** An internal error. */
    INTERNAL("internal"),
    /** The service is currently unavailable. */
    UNAVAILABLE("unavailable"),
    /** Unrecoverable data loss or corruption. */
    DATA_LOSS("data_loss"),
    /** The request does not have valid authentication credentials. */
    UNAUTHENTICATED("unauthenticated");

    private final String wire;

    Code(String wire) {
        this.wire = wire;
    }

    /** The lowercase name on the wire, as the Connect protocol's JSON error carries it. */
    public String wire() {
        return wire;
    }

    @Override
    public String toString() {
        return wire;
    }

    /**
     * The code a wire name identifies, or {@link #UNKNOWN} for one this SDK does not name.
     *
     * <p>Falling back to {@code UNKNOWN} rather than throwing is deliberate: a code from a
     * newer server still classifies a failure, and a caller must be able to catch it even when
     * the SDK is older than the server.
     */
    public static Code fromWire(String value) {
        if (value != null) {
            for (Code code : values()) {
                if (code.wire.equals(value)) {
                    return code;
                }
            }
        }
        return UNKNOWN;
    }

    /** The code this SDK does not name but a server may still send, for the mapper's tests. */
    public static Optional<Code> named(Code code) {
        return Arrays.stream(values()).filter(c -> c == code).findFirst();
    }

    /**
     * The code a gRPC status <em>number</em> identifies.
     *
     * <p>gRPC and gRPC-Web report the code numerically, where Connect reports its name; the two
     * are the same canonical set, so this is a lookup rather than a mapping. It matters because
     * the only place the {@code ErrorInfo} travels on those two protocols is inside
     * {@code grpc-status-details-bin}, so a gRPC-Web failure is otherwise a message with no
     * reason — the one outcome R8 forbids.
     *
     * <p>{@code 0} is {@code OK}, which is not a failure and not in this enum; it maps to
     * {@link #UNKNOWN} rather than throwing, because a status of zero on a failed call is a
     * server bug the caller should see as a failure rather than as a crash in the SDK.
     */
    public static Code fromNumber(int number) {
        return switch (number) {
            case 1 -> CANCELED;
            case 2 -> UNKNOWN;
            case 3 -> INVALID_ARGUMENT;
            case 4 -> DEADLINE_EXCEEDED;
            case 5 -> NOT_FOUND;
            case 6 -> ALREADY_EXISTS;
            case 7 -> PERMISSION_DENIED;
            case 8 -> RESOURCE_EXHAUSTED;
            case 9 -> FAILED_PRECONDITION;
            case 10 -> ABORTED;
            case 11 -> OUT_OF_RANGE;
            case 12 -> UNIMPLEMENTED;
            case 13 -> INTERNAL;
            case 14 -> UNAVAILABLE;
            case 15 -> DATA_LOSS;
            case 16 -> UNAUTHENTICATED;
            default -> UNKNOWN;
        };
    }
}