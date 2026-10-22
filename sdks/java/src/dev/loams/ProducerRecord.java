package dev.loams;

import java.util.Collections;
import java.util.LinkedHashMap;
import java.util.Map;

/**
 * A record to produce with a key and headers (SDKP-25). The key (partitioning, compaction), the
 * value and each header value are bytes, a string (UTF-8) or a value (as JSON), as
 * {@link StreamsModule#encode(Object)} encodes them; a null key produces an unkeyed record.
 */
public record ProducerRecord(Object key, Object value, Map<String, ?> headers) {
    public ProducerRecord {
        if (value == null) {
            throw new IllegalArgumentException("loams: a stream record needs a value");
        }
        headers = headers == null ? Map.of() : Collections.unmodifiableMap(new LinkedHashMap<>(headers));
    }

    /** A keyed record without headers. */
    public ProducerRecord(Object key, Object value) {
        this(key, value, Map.of());
    }

    /** A keyed record without headers. */
    public static ProducerRecord of(Object key, Object value) {
        return new ProducerRecord(key, value);
    }
}
