package dev.loams;

import java.util.concurrent.atomic.AtomicInteger;

/**
 * A session's consistency token (design §44 §7.4, D609; runtime contract R4).
 *
 * <p>A write answers with a {@code consistency_token}; a read accepts one, so a caller that just
 * wrote can read its own write. Threading those by hand is the caller's job today. This store is
 * the alternative §44 §7.4 asks for: <b>off by default</b>, and when a call opts in, every
 * response's token is folded into the session and attached to later reads.
 *
 * <p><b>The token's encoding is not in the protos yet.</b> §05 §5 defines the semantics (offsets
 * per stream and partition, {@code STRONG} as the default, {@code EVENTUAL},
 * {@code AT_LEAST{token}}) and API1's write paths carry it as an opaque {@code v1:} string;
 * §44 §7.4 says it merges by "max offset per stream and partition", which needs the encoding
 * parsed. Until that lands this store keeps the token it was given and <b>refuses to merge two
 * different tokens</b>, counting the refusal rather than picking one.
 *
 * <p>That refusal is the whole point. A silently-wrong consistency token reads stale data, which
 * is worse than a failure: the caller sees an answer and believes it. So the conflict is
 * surfaced through {@link #conflicts()}, and the token the store already holds is kept rather than
 * overwritten by whichever write happened to land last.
 */
public final class ConsistencySession {

    /** The prefix every consistency token carries (§44 §7.4). */
    public static final String TOKEN_PREFIX = "v1:";

    /**
     * The request header a read's token travels in.
     *
     * <p>§44 §7.4 names the <em>response</em> header {@code loams-consistency-token}; the request
     * side is a {@code consistency} field, and until a proto carries it the header is how the
     * token gets there.
     */
    public static final String HEADER = "Loams-Consistency-Token";

    /** The token to attach to the next read, or the empty string for none. */
    private volatile String current = "";

    /** How many unmergeable pairs this session has seen, so the limitation is visible. */
    private final AtomicInteger conflicts = new AtomicInteger();

    /** Whether a string looks like a consistency token. */
    public static boolean isConsistencyToken(String value) {
        return value != null && value.startsWith(TOKEN_PREFIX) && value.length() > TOKEN_PREFIX.length();
    }

    /** The token to attach to the next read, or the empty string. */
    public String current() {
        return current;
    }

    /**
     * Fold a token the server returned into the session's.
     *
     * @return {@code null} when the token was folded in or was nothing, or a message saying why
     *     two different tokens were not merged. Returning the reason rather than throwing is
     *     deliberate: the RPC that produced the token <em>succeeded</em>, and turning that into a
     *     failure would make a caller that retries on it perform the write twice.
     */
    public String record(String token) {
        if (token == null || token.isEmpty()) {
            return null;
        }
        synchronized (this) {
            if (current.isEmpty()) {
                current = token;
                return null;
            }
            if (current.equals(token)) {
                return null;
            }
            conflicts.incrementAndGet();
            return "two different consistency tokens met and this store will not merge them "
                    + "(the encoding is not in the protos yet): keeping "
                    + current
                    + " and reporting "
                    + token;
        }
    }

    /** How many unmergeable pairs the session has seen. */
    public int conflicts() {
        return conflicts.get();
    }

    /**
     * Forget the session's token.
     *
     * <p>The next read is then {@code STRONG} on its own, which is correct and loses only
     * read-your-writes.
     */
    public void clear() {
        synchronized (this) {
            current = "";
        }
    }

    @Override
    public String toString() {
        return "consistencySession(" + (current.isEmpty() ? "empty" : TOKEN_PREFIX + "…") + ")";
    }
}