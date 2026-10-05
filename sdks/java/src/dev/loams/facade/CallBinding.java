package dev.loams.facade;

import java.util.Optional;

/**
 * One facade call, as the runtime dispatches it.
 *
 * <p>Provenance is the Q604 hand-written-facade fallback; see {@link Reason}. Every field is
 * a transcription of the {@code loams.options.v1} annotations in {@code proto/}, field for
 * field, so the runtime behind it is surface-agnostic: when the Java renderer lands this
 * class is deleted and the generated one takes its place with the same accessors.
 *
 * @param module the SDK module the call is exposed on, in snake_case
 * @param name the call's name in the SDK, in PascalCase
 * @param protoName the name {@code FacadeOptions} gave it, verbatim
 * @param method the method that backs the call
 * @param rpc {@code package.Service/Method}, the path curl and grpcurl use
 * @param service the fully qualified service name
 * @param protoPackage the proto package, which is the module catalogue's key (D600)
 * @param idempotency the method's {@code idempotency_level}
 * @param retry the class derived from {@code idempotency}, or from
 *     {@code FacadeOptions.retry_safe}
 * @param streaming {@link Streaming#UNARY} or {@link Streaming#SERVER}
 * @param pagination {@code null} for a call that does not page
 * @param takesIdempotencyKey whether the request <em>message</em> declares an
 *     {@code idempotency_key} field. It is a property of the message rather than of the call,
 *     and it is what makes a mutation retryable (D610), so it is read from the generated
 *     schema rather than from what a caller passed.
 */
public record CallBinding(
        String module,
        String name,
        String protoName,
        String method,
        String rpc,
        String service,
        String protoPackage,
        IdempotencyLevel idempotency,
        RetryClass retry,
        Streaming streaming,
        Pagination pagination,
        boolean takesIdempotencyKey) {

    /** The call's retry class as a predicate: a {@code safe} call always retries. */
    public boolean retrySafe() {
        return retry == RetryClass.SAFE;
    }

    /** Whether this call is a server stream. */
    public boolean isServerStream() {
        return streaming == Streaming.SERVER;
    }

    /** The call's pagination, if it has any. */
    public Optional<Pagination> paginationFields() {
        return Optional.ofNullable(pagination);
    }
}