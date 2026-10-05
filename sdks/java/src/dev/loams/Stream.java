package dev.loams;

import java.time.Duration;
import java.util.Iterator;
import java.util.NoSuchElementException;

/**
 * A server stream, driven by the caller's loop (design §44 §7.4, D610; runtime contract R7).
 *
 * <p>The API has server streams only (D420): no client streaming and no bidi, because a browser
 * cannot stream full duplex over {@code fetch} and half-duplex works through every proxy. The
 * shape is {@link #receive()}, {@link #message()} and {@link #error()}:
 *
 * <pre>{@code
 * try (Stream<Transition> stream = client.live().watch(request, resume)) {
 *     while (stream.receive()) {
 *         apply(stream.message());
 *     }
 * }
 * if (stream.error() != null) {
 *     // the stream broke; nothing more will arrive
 * }
 * }</pre>
 *
 * <p>{@code receive} returning {@code false} at the end of the stream <b>or</b> on a failure is
 * the same mistake Java's {@link Iterator} would let you make — {@code hasNext} cannot report an
 * error — so {@link #error()} is what says which, and {@link #iterator()} exists for the callers
 * who want {@code for (Transition t : stream)} anyway.
 *
 * <p>Java's idiom for "an iteration that can fail" is for the iterator to throw rather than for
 * the loop to carry a second error channel, so {@link #iterator()} throws the mapped
 * {@link LoamsException} at the point of failure. {@link #receive()} plus {@link #error()} is the
 * form that mirrors what the other SDKs do, and both are here.
 *
 * <h2>Why a stream is not just a retry</h2>
 *
 * <p>See {@link StreamResume}. With a resume, this class tracks the cursor of every message,
 * re-opens from it on a retryable failure, and does not re-yield what it already yielded. The
 * cursor reader is required rather than defaulted, because the API's one server stream carries no
 * {@code cursor} field at all.
 *
 * @param <T> the stream's message type
 */
public final class Stream<T> implements AutoCloseable {

    /** Re-opens a stream from a cursor. Absent when the caller passed no resume. */
    @FunctionalInterface
    interface Reopener<T> {
        Receiver<T> open(String cursor);
    }

    private final String rpc;
    private final boolean retrySafe;
    private final int maxRetries;

    /** The stream currently being read. {@link #receive()} replaces it on a resume. */
    private Receiver<T> source;

    /** Non-null only when the caller passed {@link CallOptions#withStreamResume}. */
    private final StreamResume<T> resume;

    private final Reopener<T> reopener;

    private String cursor = "";
    private T current;
    private int attempt;
    private LoamsException error;
    private boolean closed;

    /**
     * @param rpc the failing RPC, as {@code package.Service/Method}
     * @param retrySafe the call's class from the generated bindings. A server stream is
     *     {@code manual} today, so a stream only resumes when the caller says so.
     * @param source the opened stream
     * @param resume the caller's resume, or {@code null}
     * @param reopener how to re-open from a cursor, or {@code null} when there is no resume. A
     *     null reopener is what turns a broken stream into a reported error instead of a spin.
     * @param maxRetries the client's default, used when the resume names none
     */
    Stream(
            String rpc,
            boolean retrySafe,
            Receiver<T> source,
            StreamResume<T> resume,
            Reopener<T> reopener,
            int maxRetries) {
        this.rpc = rpc;
        this.retrySafe = retrySafe;
        this.source = source;
        this.resume = resume;
        this.reopener = reopener;
        this.maxRetries = resume == null ? maxRetries : resume.maxRetriesOr(maxRetries);
    }

    /**
     * Advance to the next message, re-opening from the cursor if the stream broke in a way a
     * retry covers.
     *
     * @return whether there was a message
     */
    public boolean receive() {
        if (closed) {
            return false;
        }
        while (source != null) {
            try {
                T message = source.next();
                if (message != null) {
                    // Progress earns a fresh budget: maxRetries bounds the re-opens in one *run*
                    // of disconnects, not for the life of the stream.
                    attempt = 0;
                    if (resume != null) {
                        String seen = resume.cursorOf().apply(message);
                        if (seen != null && !seen.isEmpty()) {
                            cursor = seen;
                        }
                        if (resume.onCursor() != null) {
                            resume.onCursor().accept(cursor, message);
                        }
                    }
                    current = message;
                    return true;
                }
                // A clean end. Not an error, and not something to resume from: the server
                // finished.
                source = null;
                return false;
            } catch (LoamsException broken) {
                if (reopener == null
                        || !Retry.shouldRetry(
                                false, broken, retrySafe, attempt, maxRetries)) {
                    // A failure the retry class does not cover — notably an `unimplemented`
                    // stream, which is what every loams.live.v1 RPC answers in the standard
                    // variant — is reported rather than spun on.
                    error = broken;
                    source = null;
                    return false;
                }
                source.close();
                try {
                    Retry.sleep(Retry.backoff(attempt, Duration.ZERO));
                } catch (InterruptedException e) {
                    Thread.currentThread().interrupt();
                    error = (LoamsException) broken.withCallerGaveUp(e);
                    source = null;
                    return false;
                }
                try {
                    source = reopener.open(cursor);
                } catch (RuntimeException openFailure) {
                    error = Errors.toLoamsException(openFailure, rpc);
                    source = null;
                    return false;
                }
                attempt++;
            }
        }
        return false;
    }

    /** The message the last {@link #receive()} advanced to, or {@code null}. */
    public T message() {
        return current;
    }

    /** Why the stream ended, or {@code null} when it ended cleanly. */
    public LoamsException error() {
        return error;
    }

    /** The last cursor the stream applied, or the empty string when it carries none. */
    public String cursor() {
        return cursor;
    }

    /**
     * The stream as an {@link Iterator}, for a caller who wants {@code for (T t : stream)}.
     *
     * <p>It throws the mapped {@link LoamsException} at the point of failure rather than
     * swallowing it into a flag, because an iterator has nowhere else to put it and a silently
     * short iteration is indistinguishable from a finished one.
     */
    public Iterator<T> iterator() {
        return new Iterator<>() {
            private Boolean ready;

            @Override
            public boolean hasNext() {
                if (ready == null) {
                    ready = receive();
                }
                return ready;
            }

            @Override
            public T next() {
                if (!hasNext()) {
                    throw new NoSuchElementException("the stream has ended");
                }
                ready = null;
                return message();
            }
        };
    }

    /** Release the stream. Safe to call more than once, and after the stream has ended. */
    @Override
    public void close() {
        if (closed) {
            return;
        }
        closed = true;
        if (source != null) {
            source.close();
            source = null;
        }
    }

}
