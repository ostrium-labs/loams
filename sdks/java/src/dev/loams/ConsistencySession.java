package dev.loams;

import java.util.concurrent.atomic.AtomicInteger;

/**
 * The session consistency token store (design §44 §7.4, D609; runtime contract R4 as amended by
 * SDKP §2.1).
 *
 * <p><strong>Off by default.</strong> Turn it on with {@code Options.sessionConsistency(true)}.
 * Every {@code consistency_token}, {@code token} or {@code read_token} an app response carries, and
 * the {@code Loams-Consistency-Token} response header, merge in ({@link ConsistencyToken#merge}: the
 * max offset per stream and partition), and every collection read whose request has a {@code
 * consistency} field the caller left unset goes out with {@code at_least} = the session's token:
 * read-your-writes in one process. A token that is not a {@code v1:} token is counted in {@link
 * #conflicts()} and not merged.
 */
public final class ConsistencySession {

    /** The token encoding's prefix. */
    public static final String TOKEN_PREFIX = "v1:";

    /** The header a consistency token travels in. */
    public static final String HEADER = "Loams-Consistency-Token";

    private ConsistencyToken current;
    private final AtomicInteger conflicts = new AtomicInteger();
    private final AtomicInteger recorded = new AtomicInteger();

    /** Whether {@code value} looks like a consistency token: the {@code v1:} prefix and a body. */
    public static boolean isConsistencyToken(String value) {
        return value != null && value.startsWith(TOKEN_PREFIX) && value.length() > TOKEN_PREFIX.length();
    }

    /** The merged token later reads go out with, or {@code ""} when none has been recorded. */
    public synchronized String current() {
        return current == null ? "" : current.toString();
    }

    /** The merged token, or {@code null}. */
    public synchronized ConsistencyToken token() {
        return current;
    }

    /**
     * Folds a token in. Answers {@code null} when it merged (or there was nothing to merge), and a
     * description when the token was not a {@code v1:} token and was counted instead.
     */
    public String record(String token) {
        if (token == null || token.isEmpty()) {
            return null;
        }
        var parsed = ConsistencyToken.tryParse(token);
        if (parsed.isEmpty()) {
            conflicts.incrementAndGet();
            return "consistency token " + token + " is not a v1: token and was not merged";
        }
        synchronized (this) {
            current = current == null ? parsed.get() : current.merge(parsed.get());
        }
        recorded.incrementAndGet();
        return null;
    }

    /** How many tokens were not {@code v1:} tokens and were not merged. */
    public int conflicts() {
        return conflicts.get();
    }

    /** How many tokens this session has merged. */
    public int recorded() {
        return recorded.get();
    }

    /** Forgets every token. */
    public void clear() {
        synchronized (this) {
            current = null;
        }
    }

    @Override
    public String toString() {
        return "consistencySession(" + (current().isEmpty() ? "empty" : current()) + ")";
    }
}
