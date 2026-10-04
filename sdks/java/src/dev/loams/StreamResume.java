package dev.loams;

/**
 * How a server stream re-opens from a cursor (design §44 §7.4, D610; runtime contract R7).
 *
 * <p>A stream is the one call where "send it again" is not enough. The server hands out cursors,
 * and a reconnect has to resume from the last one the client applied, or the client silently
 * misses everything that changed in between — which is worse than an error, because a sync UI
 * that is quietly stale looks exactly like one that works.
 *
 * <p>So a stream with a resume tracks the cursor of every message, re-opens from it on a
 * retryable failure, and does not re-yield what it already yielded. The re-open request is the
 * stream's own business — {@code loams.live}'s {@code WatchRequest} resume, for instance — so the
 * caller supplies a {@link #resumeFrom} function and the runtime supplies the cursor.
 *
 * <p>The cursor reader is <b>required</b>, with no default. A default would have to guess a field
 * name, and the API's one server stream carries no {@code cursor} field at all: its cursor is a
 * state version. Guessing would be a method that silently resumes from nothing, which is exactly
 * the failure R7 exists to prevent.
 *
 * <p>It is generic in the request and message types rather than carrying them as erased type
 * parameters, because Java's type system cannot express "the compiler checks that
 * {@code resumeFrom} returns the message the RPC takes" for a value stored in an options object —
 * so the check happens once, in the module method that reads it back. See
 * {@link StreamResume#forMessages}.
 *
 * @param <T> the server stream's message type
 * @param cursorOf reads the cursor off a message
 * @param resumeFrom the request to re-open with, given the last cursor seen
 * @param maxRetries bounds the re-opens in one <b>run</b> of disconnects, or {@code null} to
 *     leave the client's default in place
 * @param onCursor called after each message, with the cursor it carried
 */
public record StreamResume<T>(
        java.util.function.Function<T, String> cursorOf,
        java.util.function.BiFunction<String, com.google.protobuf.Message, com.google.protobuf.Message>
                resumeFrom,
        Integer maxRetries,
        java.util.function.BiConsumer<String, T> onCursor) {

    /**
     * A resume over typed messages.
     *
     * @param cursorOf reads the cursor off a message
     * @param resumeFrom builds the request to re-open with, from the last cursor and the original
     *     request
     * @param maxRetries the re-open budget for one run of disconnects, or {@code null}
     * @param onCursor called after each message
     */
    public static <T> StreamResume<T> forMessages(
            java.util.function.Function<T, String> cursorOf,
            java.util.function.BiFunction<String, com.google.protobuf.Message, com.google.protobuf.Message>
                    resumeFrom,
            Integer maxRetries,
            java.util.function.BiConsumer<String, T> onCursor) {
        return new StreamResume<>(cursorOf, resumeFrom, maxRetries, onCursor);
    }

    /** The re-open budget for one run of disconnects, or {@code null} for the client's default. */
    public int maxRetriesOr(int clientDefault) {
        return maxRetries == null ? clientDefault : maxRetries;
    }
}