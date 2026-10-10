package dev.loams;

import java.util.Collections;
import java.util.LinkedHashMap;
import java.util.Map;

/**
 * The fields of {@code loams.errors.v1.ErrorInfo} the SDK reads.
 *
 * <p>It is a shape of its own rather than the generated type so that a caller reading a reason
 * does not have to import the stub package, and so the SDK's own error surface does not change
 * when a field is added to the proto.
 *
 * @param reason the stable, machine-readable cause. Never branch on the message instead.
 * @param metadata structured context, for example {@code {"variant": "standard"}}. Never
 *     carries secrets.
 * @param hint a short next step in the caller's locale, or the empty string.
 */
public record ErrorInfoShape(String reason, Map<String, String> metadata, String hint) {

    public ErrorInfoShape {
        reason = reason == null ? "" : reason;
        hint = hint == null ? "" : hint;
        metadata =
                metadata == null
                        ? Collections.emptyMap()
                        : Collections.unmodifiableMap(new LinkedHashMap<>(metadata));
    }
}