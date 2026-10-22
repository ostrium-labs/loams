package dev.loams;

import java.util.ArrayList;
import java.util.Comparator;
import java.util.List;
import java.util.Map;
import java.util.Optional;

/**
 * A parsed consistency token (runtime contract R4 as amended by SDKP §2.1): the log positions a
 * write reached, which a read can wait for ({@code Consistency.at_least}).
 *
 * <p>The encoding is {@code crates/loams-collection/src/token.rs}'s, {@code
 * v1:s<stream>/p<partition>@<offset>,…}: one entry per (stream, partition), sorted, with {@link
 * #merge} keeping the max offset per pair. {@code v1:} alone is the empty token. Every number is
 * canonical decimal, a partition fits in 32 bits and a stream and offset in 64 (unsigned), and
 * anything else is {@code invalid_argument} raised here, before the network.
 */
public final class ConsistencyToken {

    /** One entry: a stream, a partition and an offset (stream and offset are unsigned 64-bit). */
    public record Position(long stream, int partition, long offset) {
        @Override
        public String toString() {
            return "s" + Long.toUnsignedString(stream) + "/p" + Integer.toUnsignedString(partition)
                    + "@" + Long.toUnsignedString(offset);
        }
    }

    private static final String PREFIX = "v1:";
    private static final Comparator<Position> ORDER =
            Comparator.comparing(Position::stream, Long::compareUnsigned)
                    .thenComparing(Position::partition, Integer::compareUnsigned)
                    .thenComparing(Position::offset, Long::compareUnsigned);

    /** The empty token, {@code v1:}. */
    public static final ConsistencyToken EMPTY = new ConsistencyToken(List.of());

    private final List<Position> positions;

    private ConsistencyToken(List<Position> positions) {
        this.positions = List.copyOf(positions);
    }

    /** A token over these positions, normalised: sorted, the max offset per pair. */
    public static ConsistencyToken of(List<Position> positions) {
        List<Position> sorted = new ArrayList<>(positions);
        sorted.sort(ORDER);
        List<Position> normal = new ArrayList<>(sorted.size());
        for (Position p : sorted) {
            int last = normal.size() - 1;
            if (last >= 0 && normal.get(last).stream() == p.stream() && normal.get(last).partition() == p.partition()) {
                normal.set(last, p);
            } else {
                normal.add(p);
            }
        }
        return new ConsistencyToken(normal);
    }

    /**
     * Parses a token.
     *
     * @throws InvalidArgumentException the text is not a {@code v1:} token
     */
    public static ConsistencyToken parse(String text) {
        return tryParse(text)
                .orElseThrow(
                        () ->
                                new InvalidArgumentException(
                                        Code.INVALID_ARGUMENT,
                                        null,
                                        "invalid_consistency_token",
                                        Map.of("token", text == null ? "" : text),
                                        "",
                                        "",
                                        new IllegalArgumentException(
                                                "invalid consistency token \"" + text
                                                        + "\": expected v1:s<stream>/p<partition>@<offset>,…")));
    }

    /** Parses a token, or answers empty. */
    public static Optional<ConsistencyToken> tryParse(String text) {
        if (text == null || !text.startsWith(PREFIX)) {
            return Optional.empty();
        }
        String body = text.substring(PREFIX.length());
        if (body.isEmpty()) {
            return Optional.of(EMPTY);
        }
        List<Position> positions = new ArrayList<>();
        for (String item : body.split(",", -1)) {
            Position p = item(item);
            if (p == null) {
                return Optional.empty();
            }
            positions.add(p);
        }
        return Optional.of(new ConsistencyToken(positions));
    }

    private static Position item(String item) {
        if (!item.startsWith("s")) {
            return null;
        }
        int slash = item.indexOf("/p");
        if (slash < 0) {
            return null;
        }
        int at = item.indexOf('@', slash);
        if (at < 0) {
            return null;
        }
        Long stream = canonical(item.substring(1, slash));
        Long partition = canonical(item.substring(slash + 2, at));
        Long offset = canonical(item.substring(at + 1));
        if (stream == null || partition == null || offset == null
                || Long.compareUnsigned(partition, 0xFFFF_FFFFL) > 0) {
            return null;
        }
        return new Position(stream, (int) (long) partition, offset);
    }

    private static Long canonical(String digits) {
        if (digits.isEmpty() || (digits.length() > 1 && digits.charAt(0) == '0')) {
            return null;
        }
        for (int i = 0; i < digits.length(); i++) {
            char c = digits.charAt(i);
            if (c < '0' || c > '9') {
                return null;
            }
        }
        try {
            return Long.parseUnsignedLong(digits);
        } catch (NumberFormatException e) {
            return null;
        }
    }

    /** The entries, sorted by stream then partition, one per pair. */
    public List<Position> positions() {
        return positions;
    }

    /** The union of two tokens, keeping the max offset per (stream, partition). */
    public ConsistencyToken merge(ConsistencyToken other) {
        List<Position> all = new ArrayList<>(positions);
        all.addAll(other.positions);
        return of(all);
    }

    /** The offset this token holds for a pair. */
    public Optional<Long> offset(long stream, int partition) {
        Long best = null;
        for (Position p : positions) {
            if (p.stream() == stream && p.partition() == partition
                    && (best == null || Long.compareUnsigned(p.offset(), best) > 0)) {
                best = p.offset();
            }
        }
        return Optional.ofNullable(best);
    }

    /** The wire form. */
    @Override
    public String toString() {
        StringBuilder out = new StringBuilder(PREFIX);
        for (int i = 0; i < positions.size(); i++) {
            if (i > 0) {
                out.append(',');
            }
            out.append(positions.get(i));
        }
        return out.toString();
    }

    @Override
    public boolean equals(Object other) {
        return other instanceof ConsistencyToken token && token.positions.equals(positions);
    }

    @Override
    public int hashCode() {
        return positions.hashCode();
    }
}
