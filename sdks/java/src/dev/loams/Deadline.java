package dev.loams;

import java.time.Duration;
import java.util.Map;

/**
 * One logical call's budget (runtime contract R12, SDKP §2.1).
 *
 * <p>A unary call has one budget, 30 s unless the client or the call names another, covering every
 * attempt and every backoff. Each attempt tells the server what is left ({@code
 * connect-timeout-ms}). When the budget runs out the call fails with {@code deadline_exceeded},
 * reason {@code client_deadline_exceeded}, and is not retried: there is no time left to retry in.
 */
public final class Deadline {

    /** The client-side reason a spent budget carries (docs/api/reasons.md, client-side). */
    public static final String CLIENT_DEADLINE_EXCEEDED = "client_deadline_exceeded";

    /** No budget. */
    static final Deadline NONE = new Deadline(Duration.ZERO);

    private final Duration budget;
    private final long end;

    private Deadline(Duration budget) {
        this.budget = budget;
        this.end = active(budget) ? System.nanoTime() + budget.toNanos() : 0;
    }

    private static boolean active(Duration budget) {
        return budget != null && !budget.isZero() && !budget.isNegative();
    }

    /** Starts a budget; zero or null means none. */
    public static Deadline start(Duration budget) {
        return active(budget) ? new Deadline(budget) : NONE;
    }

    /** Whether there is a budget at all. */
    public boolean active() {
        return end != 0;
    }

    /** What is left, or null when there is no budget. */
    public Duration remaining() {
        if (end == 0) {
            return null;
        }
        long left = end - System.nanoTime();
        return Duration.ofNanos(Math.max(left, 1_000_000L));
    }

    /** Whether the budget is spent. */
    public boolean expired() {
        return end != 0 && System.nanoTime() - end >= 0;
    }

    /** Whether waiting {@code wait} would spend the budget. */
    boolean wouldPass(Duration wait) {
        return end != 0 && System.nanoTime() + wait.toNanos() - end >= 0;
    }

    /** The error a spent budget raises. */
    public DeadlineExceededException exceeded(String rpc, Throwable cause) {
        String ms = Long.toString(budget.toMillis());
        return new DeadlineExceededException(
                Code.DEADLINE_EXCEEDED,
                null,
                CLIENT_DEADLINE_EXCEEDED,
                Map.of("timeout_ms", ms),
                "",
                rpc,
                new java.util.concurrent.TimeoutException(
                        "the call did not finish in " + ms + " ms"
                                + (cause == null ? "" : "; the last attempt ended with " + cause)));
    }

    /** Whether an error is a spent client budget, which is never retried. */
    static boolean isClientDeadline(Throwable error) {
        return error instanceof LoamsException e && CLIENT_DEADLINE_EXCEEDED.equals(e.unknownReason());
    }

    /** Whether a transport failure is the request's own timeout firing. */
    static boolean isTimeout(Throwable error) {
        for (Throwable t = error; t != null; t = t.getCause()) {
            if (t instanceof java.net.http.HttpTimeoutException) {
                return true;
            }
        }
        return false;
    }
}
