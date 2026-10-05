package dev.loams.connect;

import dev.loams.internal.Json;
import java.util.ArrayList;
import java.util.List;
import java.util.Map;

/**
 * A failed RPC as the wire described it, before any mapping to the SDK's exception hierarchy.
 *
 * <p>Two shapes produce one of these, and both have to: a unary failure is an HTTP status with
 * a JSON body, while a failure on a server stream arrives <b>inside the Connect envelope</b> on
 * a 200. A client that only reads status codes sees success in the second case, which is why
 * this type is parsed from the end-stream frame as well as from the body.
 *
 * <p>Nothing here is mapped yet. {@link dev.loams.Errors} does that, because the mapping needs
 * the reason registry and the exception hierarchy, and a wire type that knew about them would be
 * a wire type that could not be read on its own.
 *
 * @param code the Connect code's wire name, or {@code null} when the body carried none
 * @param message the human-readable message. For a person; nothing in an SDK branches on it.
 * @param details the details, in the order the server sent them
 * @param metadata the trailing metadata of an end-stream frame, which is usually empty
 */
public record ConnectError(
        String code, String message, List<ConnectErrorDetail> details, Map<String, String> metadata) {

    public ConnectError {
        details = List.copyOf(details);
        metadata = Map.copyOf(metadata);
    }

    /**
     * The unary error shape: the whole body is the error.
     *
     * <p>A body that is not JSON, or is JSON with no {@code code}, yields {@code null}: the
     * caller then reports a below-the-API failure, which is the honest reading of bytes that
     * are not a Connect error at all. A proxy's HTML 502 page is the case this exists for.
     */
    public static ConnectError fromUnaryBody(String body) {
        Map<String, Object> parsed;
        try {
            parsed = Json.parseObject(body);
        } catch (RuntimeException e) {
            return null;
        }
        if (Json.string(parsed, "code") == null) {
            return null;
        }
        return new ConnectError(
                Json.string(parsed, "code"),
                Json.string(parsed, "message", ""),
                details(Json.array(parsed, "details")),
                Map.of());
    }

    /**
     * The end-stream shape: the body is a stream result whose {@code error} may be absent.
     *
     * <p>{@code null} means the stream ended cleanly, which is a real outcome and not a failure:
     * an end-stream frame with no {@code error} is how a server says a stream is over.
     */
    public static ConnectError fromEndStream(String body) {
        Map<String, Object> parsed;
        try {
            parsed = Json.parseObject(body);
        } catch (RuntimeException e) {
            // A frame that is not JSON but was flagged as the end frame is a server bug.
            // Reporting it as a failure with no reason is better than dropping it: R8's one
            // prohibition is a Loams failure with neither reason nor hint.
            return new ConnectError(
                    null,
                    "the stream's end frame is not JSON: " + body,
                    List.of(),
                    Map.of());
        }
        Map<String, String> metadata = Json.stringMap(parsed, "metadata");
        Map<String, Object> error = Json.object(parsed, "error");
        if (error == null) {
            return null;
        }
        return new ConnectError(
                Json.string(error, "code"),
                Json.string(error, "message", ""),
                details(Json.array(error, "details")),
                metadata);
    }

    private static List<ConnectErrorDetail> details(List<Object> raw) {
        List<ConnectErrorDetail> out = new ArrayList<>(raw.size());
        for (Object entry : raw) {
            if (!(entry instanceof Map)) {
                continue;
            }
            Map<String, Object> detail = castMap(entry);
            String type = Json.string(detail, "type");
            String value = Json.string(detail, "value");
            if (type == null || value == null) {
                continue;
            }
            try {
                out.add(new ConnectErrorDetail(type, java.util.Base64.getDecoder().decode(value)));
            } catch (IllegalArgumentException e) {
                // A detail whose value is not base64 is skipped. The reason lives in a
                // *different* detail, and losing the whole error over one bad sibling would
                // cost the caller the reason (R8).
            }
        }
        return out;
    }

    @SuppressWarnings("unchecked")
    private static Map<String, Object> castMap(Object value) {
        return (Map<String, Object>) value;
    }

    /** The first detail of {@code type}, or {@code null} when the server sent none. */
    public ConnectErrorDetail detail(String type) {
        for (ConnectErrorDetail candidate : details) {
            if (candidate.type().equals(type)) {
                return candidate;
            }
        }
        return null;
    }
}