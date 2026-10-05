package dev.loams;

import com.google.protobuf.Descriptors;
import com.google.protobuf.Message;
import java.security.SecureRandom;
import java.time.Instant;
import java.util.Optional;

/**
 * Idempotency keys (design §44 §7.4, D610; runtime contract R3).
 *
 * <p>A mutating call that carries an {@code idempotency_key} field is given one <b>per logical
 * call</b>, before the first attempt, and <b>the same key goes out on every retry</b>. A key
 * regenerated per attempt turns one write into two, which is the exact failure the key exists to
 * prevent.
 *
 * <p>The decision to key is read from the <b>generated schema</b> rather than from the object a
 * caller happened to build, and that matters for proto3 {@code optional}:
 * {@code MutateRequest.idempotency_key} is optional, so a caller who leaves it out sends no key
 * at all, the mutation is not retryable, and the SDK would never know — unless it asks the
 * descriptor. And it must not confuse {@code MutateRequest}, which has the field, with
 * {@code DeployRequest}, which does not, or it would invent a field the schema does not know.
 *
 * <p>Everything here goes through protobuf reflection rather than generated getters, which is
 * what lets one implementation serve every keyed RPC instead of one per message type.
 */
public final class Idempotency {

    private Idempotency() {}

    /** The proto field name this SDK keys a request on. */
    public static final String IDEMPOTENCY_KEY_FIELD = "idempotency_key";

    private static final SecureRandom RANDOM = new SecureRandom();

    /**
     * A fresh UUIDv7, as the canonical lowercase hyphenated string.
     *
     * <p>An idempotency key has to be unique across every client that has ever talked to an
     * instance <em>and</em> sort by creation time, because a key that sorts is one an operator
     * can correlate in a log. UUIDv4 is unique but unordered; ULIDs would do, but each of the
     * thirteen SDKs would then need its own implementation — so this is thirty lines rather
     * than a dependency, the same trade the Go SDK made.
     *
     * <p>Layout: 48 bits of Unix milliseconds, 4 bits of version (7), 12 bits of counter within
     * the millisecond, 2 bits of variant, 62 random bits.
     */
    public static String uuidV7() {
        byte[] bytes = new byte[16];
        RANDOM.nextBytes(bytes);
        long millis = Instant.now().toEpochMilli();
        for (int index = 0; index < 6; index++) {
            // The 48-bit timestamp is big-endian, so it is written out with shifts rather than by
            // dividing: dividing keeps the fractional bits of the lower digits and truncates the
            // carry, which puts the wrong byte in.
            bytes[index] = (byte) ((millis >>> ((5 - index) * 8)) & 0xff);
        }
        bytes[6] = (byte) ((bytes[6] & 0x0f) | 0x70); // version 7
        bytes[8] = (byte) ((bytes[8] & 0x3f) | 0x80); // variant 10
        return format(bytes);
    }

    private static String format(byte[] bytes) {
        StringBuilder out = new StringBuilder(36);
        for (int index = 0; index < 16; index++) {
            if (index == 4 || index == 6 || index == 8 || index == 10) {
                out.append('-');
            }
            out.append(Character.forDigit((bytes[index] >> 4) & 0xf, 16));
            out.append(Character.forDigit(bytes[index] & 0xf, 16));
        }
        return out.toString();
    }

    /**
     * The Unix milliseconds a UUIDv7 encodes.
     *
     * <p>The timestamp is the first <b>twelve</b> hex digits, not eight: 48 bits, and
     * milliseconds since the epoch use 41 of them. Reading eight digits returns a number around
     * 2^25, which is January 1970 — which is the whole reason this method exists rather than a
     * substring in a caller.
     *
     * @return the instant, or empty for anything that is not a version-7 UUID
     */
    public static Optional<Instant> uuidV7Time(String value) {
        if (value == null || value.length() != 36) {
            return Optional.empty();
        }
        if (value.charAt(8) != '-'
                || value.charAt(13) != '-'
                || value.charAt(18) != '-'
                || value.charAt(23) != '-') {
            return Optional.empty();
        }
        if (value.charAt(14) != '7') {
            return Optional.empty();
        }
        char variant = value.charAt(19);
        if (variant != '8' && variant != '9' && variant != 'a' && variant != 'b') {
            return Optional.empty();
        }
        String digits = value.substring(0, 8) + value.substring(9, 13);
        long millis = 0;
        for (int index = 0; index < digits.length(); index++) {
            int digit = Character.digit(digits.charAt(index), 16);
            if (digit < 0) {
                return Optional.empty();
            }
            millis = (millis << 4) | digit;
        }
        return Optional.of(Instant.ofEpochMilli(millis));
    }

    /**
     * Whether a request's <b>schema</b> declares {@code idempotency_key}.
     *
     * <p>It is public so a caller — and the drift test — can ask the schema rather than the
     * object, which is the point of R3.
     */
    public static boolean declaresIdempotencyKey(Message message) {
        Descriptors.FieldDescriptor field = keyField(message);
        return field != null;
    }

    private static Descriptors.FieldDescriptor keyField(Message message) {
        if (message == null) {
            return null;
        }
        Descriptors.FieldDescriptor field =
                message.getDescriptorForType().findFieldByName(IDEMPOTENCY_KEY_FIELD);
        if (field == null || field.getType() != Descriptors.FieldDescriptor.Type.STRING) {
            return null;
        }
        return field;
    }

    /**
     * A request the runtime has decided to key, and whether it made that decision.
     *
     * @param request the message to send: a copy with the key set, or the original when nothing
     *     was set
     * @param keyed whether the request now carries a key the retry policy may rely on
     */
    public record KeyedRequest<T extends Message>(T request, boolean keyed) {}

    /**
     * Decide a mutating call's idempotency key, once per logical call.
     *
     * @param request the caller's message. Never mutated: a keyed request is a copy, because a
     *     caller who retries their own request would otherwise find their own message changed
     *     underneath them.
     * @param supplied the caller's own key, or the empty string. Supplying one makes the retry
     *     yours rather than the SDK's, which is sometimes right because the key is what your
     *     storage dedupes on.
     * @param declared what {@code CallBinding.takesIdempotencyKey()} says, read off the generated
     *     schema by the binding table rather than off the object
     */
    // The one unchecked operation in the SDK: `Message.Builder.build()` is declared to return
    // `Message`, and `request.toBuilder()` was called on a `T`, so the builder's build is a `T`.
    // There is no way to say that in Java's type system, and a generated setter per message type
    // would defeat the whole point of reaching the field through reflection.
    @SuppressWarnings("unchecked")
    public static <T extends Message> KeyedRequest<T> applyIdempotencyKey(
            T request, String supplied, boolean declared) {
        if (request == null) {
            return new KeyedRequest<>(null, false);
        }
        // Two proofs are available and either is enough: the generated schema declaring it
        // (`declared`, which the binding carries), or the message's own descriptor. Refusing a
        // request that demonstrably has the field would be a way to silently make a mutation
        // un-retryable.
        Descriptors.FieldDescriptor field = keyField(request);
        if (field == null) {
            return new KeyedRequest<>(request, false);
        }
        String current = readString(request, field);
        if (!current.isEmpty()) {
            // The caller set one; it goes out on every attempt untouched.
            return new KeyedRequest<>(request, true);
        }
        String key = supplied == null || supplied.isEmpty() ? uuidV7() : supplied;
        // `Message.Builder` rather than a generated `X.Builder`: a generated message has its own
        // nested `Builder` that shadows the parameterised name, so the typed form does not apply
        // uniformly across every message type. `setField` goes through the reflection-backed
        // implementation either way, which is what makes one implementation serve every keyed RPC.
        Message.Builder builder = request.toBuilder();
        // `setField` rather than a generated setter: the field is proto3 `optional`, so it sits
        // in a synthetic oneof, and the generated setter would have to be named per message
        // type. Reflection is what lets one implementation serve every keyed RPC.
        builder.setField(field, key);
        return new KeyedRequest<>((T) builder.build(), true);
    }

    private static String readString(Message message, Descriptors.FieldDescriptor field) {
        if (!message.hasField(field)) {
            // proto3 `optional` means presence is tracked, so an absent field is not the empty
            // string but is read the same way.
            return "";
        }
        Object value = message.getField(field);
        return value instanceof String text ? text : "";
    }
}