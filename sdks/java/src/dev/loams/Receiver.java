package dev.loams;

/**
 * A source of stream messages: a real wire stream or a stub.
 *
 * <p>It is an interface so {@link Stream}'s resume logic can be tested against a scripted
 * receiver — which is what {@code java_stream_resume_with_cursor} does for the half of R7 that has
 * no server to run against, since the only server stream in the API
 * ({@code loams.live.v1.LiveService/Watch}) is never served in any variant.
 *
 * @param <T> the stream's message type
 */
public interface Receiver<T> extends AutoCloseable {

    /**
     * The next message.
     *
     * @return the next message, or {@code null} at a <b>clean</b> end of stream
     * @throws LoamsException when the stream broke. A broken stream and a finished one are
     *     different outcomes and this is where the difference lives: a caller that treats
     *     {@code null} as "no more messages" will believe a stream that refused to open yielded
     *     an empty result rather than an error.
     */
    T next();

    /** Release the stream. Safe to call more than once. */
    @Override
    void close();
}