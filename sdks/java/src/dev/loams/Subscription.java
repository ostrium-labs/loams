package dev.loams;

import java.nio.charset.StandardCharsets;
import java.time.Duration;
import java.util.Iterator;
import java.util.List;
import java.util.NoSuchElementException;

/**
 * {@code streams.subscribe} (runtime contract R14, SDKP §2.5, SDKP-2): a resumable subscription to
 * one partition, as an {@link Iterator} that is also {@link AutoCloseable}.
 *
 * <p>A client loop over unary {@code Fetch}, so it works on every transport and through every
 * proxy: fetch from the next offset, yield what came back, and move the offset to the last record's
 * offset + 1. An empty fetch backs off 100 ms, doubling to 1 s, and the first non-empty one resets
 * it. {@link #offset()} is always the next offset to fetch, so a caller that stops can resume from
 * it. {@link #close()} stops the loop; {@code hasNext()} then answers false.
 *
 * <p>{@code client.streams().subscribe(...)} wires this to {@code loams.stream.v1.StreamService/
 * Fetch} when that package is on the main port (SDKP P0c); until then the loop is public so a caller
 * can drive it over any fetch.
 */
public final class Subscription implements Iterator<Subscription.Record>, AutoCloseable {

    /** One record a subscription yields; {@code key} is null when it was produced without one. */
    public record Record(int partition, long offset, byte[] value, long timestampMs, byte[] key) {
        /** A record without a key. */
        public Record(int partition, long offset, byte[] value, long timestampMs) {
            this(partition, offset, value, timestampMs, null);
        }

        /** The value as UTF-8 text. */
        public String text() {
            return new String(value, StandardCharsets.UTF_8);
        }
    }

    /** Fetches the records of one partition from an offset. */
    @FunctionalInterface
    public interface Fetch {
        List<Record> fetch(long offset);
    }

    /** Waits between empty fetches; {@code Thread.sleep} unless a test replaces it. */
    @FunctionalInterface
    public interface Sleeper {
        void sleep(Duration wait) throws InterruptedException;
    }

    /** The first backoff after an empty fetch. */
    public static final Duration INITIAL_BACKOFF = Duration.ofMillis(100);

    /** The longest backoff. */
    public static final Duration MAX_BACKOFF = Duration.ofSeconds(1);

    private final Fetch fetch;
    private final Sleeper sleeper;
    private volatile long offset;
    private volatile boolean closed;
    private Iterator<Record> page = java.util.Collections.emptyIterator();
    private Record ready;
    private Duration backoff = INITIAL_BACKOFF;

    /** A subscription from {@code fromOffset}. */
    public Subscription(Fetch fetch, long fromOffset) {
        this(fetch, fromOffset, wait -> Thread.sleep(wait.toMillis()));
    }

    /** A subscription with its own sleeper. */
    public Subscription(Fetch fetch, long fromOffset, Sleeper sleeper) {
        this.fetch = fetch;
        this.offset = fromOffset;
        this.sleeper = sleeper;
    }

    /** The next offset to fetch: the last yielded record's offset + 1. Resume from it. */
    public long offset() {
        return offset;
    }

    @Override
    public boolean hasNext() {
        while (ready == null && !closed) {
            while (page.hasNext()) {
                Record record = page.next();
                if (Long.compareUnsigned(record.offset(), offset) >= 0) {
                    ready = record;
                    return true;
                }
            }
            List<Record> records = fetch.fetch(offset);
            // A page with nothing at or past the offset is an empty one: back off rather
            // than spin on records already yielded.
            long from = offset;
            if (records.stream().noneMatch(r -> Long.compareUnsigned(r.offset(), from) >= 0)) {
                try {
                    sleeper.sleep(backoff);
                } catch (InterruptedException e) {
                    Thread.currentThread().interrupt();
                    closed = true;
                    return false;
                }
                Duration doubled = backoff.multipliedBy(2);
                backoff = doubled.compareTo(MAX_BACKOFF) > 0 ? MAX_BACKOFF : doubled;
                continue;
            }
            backoff = INITIAL_BACKOFF;
            page = records.iterator();
        }
        return ready != null;
    }

    @Override
    public Record next() {
        if (!hasNext()) {
            throw new NoSuchElementException();
        }
        Record record = ready;
        ready = null;
        offset = record.offset() + 1;
        return record;
    }

    /** Stops the loop. */
    @Override
    public void close() {
        closed = true;
    }
}
