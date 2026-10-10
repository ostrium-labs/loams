package dev.loams;

import dev.loams.facade.Reason;
import java.util.Collections;
import java.util.LinkedHashMap;
import java.util.Map;

/**
 * What every Loams failure carries (design §44 §7.4, decision D611).
 *
 * <p>A failed RPC carries a Connect {@link Code} and one {@code loams.errors.v1.ErrorInfo} in
 * its details. The {@link #reason()} is what a caller branches on: a stable {@code snake_case}
 * string, registered in {@code docs/api/reasons.md} and mirrored into
 * {@link dev.loams.facade.Reason}, so the branch is exhaustive and a reason the registry has
 * lost stops compiling. The message is for people and may change; nothing in an SDK branches on
 * it.
 *
 * <p>The subclasses below are one per code D611 names, so the branch in Java is a plain
 * {@code instanceof} — the thing Go needs {@code errors.As} for:
 *
 * <pre>{@code
 * try {
 *     client.tables().query(request);
 * } catch (NotFoundException e) {
 *     // the named resource does not exist
 * } catch (LoamsException e) {
 *     // some other Loams failure, with a reason on it
 * }
 * }</pre>
 *
 * <p>and for a reason rather than a class:
 *
 * <pre>{@code
 * } catch (LoamsException e) {
 *     if (e.reason() == Reason.NOT_FOUND) { ... }
 * }</pre>
 *
 * <p>Three cases are distinct and are not conflated:
 *
 * <ul>
 *   <li>a reason from a <em>newer</em> server, which this SDK's registry does not have: it is
 *       surfaced as text in {@link #unknownReason()} and flagged, not dropped;
 *   <li>a failure from <em>below the API</em> — a socket, a refused connection, an interrupt —
 *       which carries no {@link #reason()} at all;
 *   <li>a {@code LoamsException}, which is returned unchanged if it is mapped twice, so
 *       wrapping an SDK's own exception never loses its reason.
 * </ul>
 */
public class LoamsException extends RuntimeException {

    private static final long serialVersionUID = 1L;

    private final Code code;
    private final Reason reason;
    private final String unknownReason;
    private final Map<String, String> metadata;
    private final String hint;
    private final String rpc;
    private final boolean callerGaveUp;

    /**
     * @param code the Connect code
     * @param reason the stable cause, or {@code null} when the failure came from below the API
     * @param unknownReason a reason off the wire this SDK's registry does not have
     * @param metadata the structured context the server sent. Never secrets.
     * @param hint a short next step in the caller's locale
     * @param rpc the failing RPC, as {@code package.Service/Method}
     * @param cause the failure below this one: a transport exception, an interrupt, a bug
     */
    public LoamsException(
            Code code,
            Reason reason,
            String unknownReason,
            Map<String, String> metadata,
            String hint,
            String rpc,
            Throwable cause,
            boolean callerGaveUp) {
        super(message(code, reason, unknownReason, rpc, cause), cause);
        this.code = code;
        this.reason = reason;
        this.unknownReason = unknownReason;
        this.metadata = metadata == null
                ? Collections.emptyMap()
                : Collections.unmodifiableMap(new LinkedHashMap<>(metadata));
        this.hint = hint == null ? "" : hint;
        this.rpc = rpc == null ? "" : rpc;
        this.callerGaveUp = callerGaveUp;
    }

    private static String message(
            Code code, Reason reason, String unknownReason, String rpc, Throwable cause) {
        StringBuilder out = new StringBuilder();
        if (rpc != null && !rpc.isEmpty()) {
            out.append(rpc).append(": ");
        }
        out.append(code.wire());
        String shown = reason != null ? reason.wire() : unknownReason;
        if (shown != null && !shown.isEmpty()) {
            out.append(" (").append(shown).append(')');
        }
        out.append(": ");
        out.append(cause == null ? code.wire() : String.valueOf(cause.getMessage()));
        return out.toString();
    }

    /** The Connect code, the canonical classification. */
    public final Code code() {
        return code;
    }

    /**
     * The stable cause, or {@code null} when there is none.
     *
     * <p>{@code null} means the failure came from <em>below the API</em> — a socket, a
     * timeout, an interrupt — not from a Loams service. That is a different thing from a
     * service refusing, and {@link Errors#reasonOf(Throwable)} returns {@code null} for it too.
     */
    public final Reason reason() {
        return reason;
    }

    /**
     * A reason off the wire that this SDK's registry does not have, meaning the server is newer
     * than the SDK.
     *
     * <p>It is surfaced rather than dropped: losing it would leave a caller unable to tell
     * "not supported here" from "not supported at all" (R8). It is empty rather than
     * {@code null} when the server sent no such reason.
     */
    public final String unknownReason() {
        return unknownReason == null ? "" : unknownReason;
    }

    /** Whether {@link #reason()} is empty because the failure came from below the API. */
    public final boolean isBelowApi() {
        return reason == null && unknownReason.isEmpty();
    }

    /**
     * The structured context the server sent, for example {@code {"variant": "standard"}}. Never
     * secrets.
     */
    public final Map<String, String> metadata() {
        return metadata;
    }

    /** A short next step in the caller's locale, or the empty string. */
    public final String hint() {
        return hint;
    }

    /** The RPC that failed, as {@code package.Service/Method}, or the empty string. */
    public final String rpc() {
        return rpc;
    }

    /**
     * Whether the caller's own deadline or interrupt ended the call while a retryable failure
     * was still outstanding.
     *
     * <p>Both facts matter and only one of them is obvious: "the node was unavailable" is the
     * diagnosis and "you did not get an answer" is what the caller must act on, so this flag
     * carries the second rather than replacing the first with it. The underlying failure is
     * also attached as a {@linkplain #addSuppressed suppressed} exception, so the context error
     * is still reachable for a caller who wants it.
     */
    public final boolean callerGaveUp() {
        return callerGaveUp;
    }

    /**
     * Record that the caller's context ended this call, joining the context failure into the
     * chain.
     *
     * <p>The same {@code LoamsException} is mutated and returned rather than a new one being
     * built, which is safe because the mapper built it for this call and nothing else holds it.
     * A subclass's own type is preserved by the return value; the caller keeps the reference it
     * already has, so this only adds to the message's causes.
     */
    public LoamsException withCallerGaveUp(Throwable contextFailure) {
        addSuppressed(contextFailure);
        return this;
    }

    @Override
    public String toString() {
        return getClass().getSimpleName() + ": " + getMessage();
    }
}