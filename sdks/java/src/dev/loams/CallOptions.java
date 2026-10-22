package dev.loams;

import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

/**
 * The per-call overrides, and the resolved settings they produce.
 *
 * <p>Everything is optional: the client's defaults apply and the generated binding supplies the
 * retry class. In Java the options are a {@link List} rather than Go's variadic slice, because
 * that is the shape {@code java.util.function.Consumer} gives an {@code options(...)} parameter
 * at the call site:
 *
 * <pre>{@code
 * client.tables().mutate(request, CallOptions.withIdempotencyKey("order-4711"));
 * client.instance().getInstance(request, CallOptions.withMaxRetries(0));
 * }</pre>
 *
 * <p>It is one class rather than a functional interface per option on purpose: a caller passes
 * several at once and has to be able to combine them, and a builder is the Java shape that makes
 * that readable.
 */
public final class CallOptions {

    private final Integer maxRetries;
    private final Boolean retrySafe;
    private final String idempotencyKey;
    private final Map<String, String> headers;
    private final String consistencyToken;
    private final ConsistencySession session;
    private final StreamResume<?> streamResume;
    private final String namespace;
    private final java.time.Duration timeout;
    private final Boolean mintIdempotencyKey;

    private CallOptions(Builder builder) {
        this.maxRetries = builder.maxRetries;
        this.retrySafe = builder.retrySafe;
        this.idempotencyKey = builder.idempotencyKey;
        this.headers = Map.copyOf(builder.headers);
        this.consistencyToken = builder.consistencyToken;
        this.session = builder.session;
        this.streamResume = builder.streamResume;
        this.namespace = builder.namespace;
        this.timeout = builder.timeout;
        this.mintIdempotencyKey = builder.mintIdempotencyKey;
    }

    /** No overrides at all: the client's defaults and the binding's retry class. */
    public static CallOptions none() {
        return new Builder().build();
    }

    /**
     * A builder, for a call that needs more than one override.
     *
     * <p>The static factories below cover the common cases and are what most callers want; the
     * builder is for the rest.
     */
    public static Builder builder() {
        return new Builder();
    }

    /**
     * Bound the retries after the first attempt for this call.
     *
     * <p>Zero disables them, which is what a caller wants for a mutation they would rather see
     * fail than repeat.
     */
    public static CallOptions withMaxRetries(int maxRetries) {
        return builder().maxRetries(maxRetries).build();
    }

    /**
     * Override the call's retry class, which otherwise comes from the generated bindings.
     *
     * <p>Setting it false on a read stops the SDK retrying it. Setting it true on a mutation
     * retries something the proto says is not safe to repeat, which is only correct if the call
     * carries an idempotency key.
     */
    public static CallOptions withRetrySafe(boolean retrySafe) {
        return builder().retrySafe(retrySafe).build();
    }

    /**
     * Supply the idempotency key for this call, making the retry yours rather than the SDK's.
     *
     * <p>Omit it and the runtime mints a UUIDv7 per logical call and reuses it on every retry
     * (R3).
     */
    public static CallOptions withIdempotencyKey(String key) {
        return builder().idempotencyKey(key).build();
    }

    /** Add one header to this call. */
    public static CallOptions withHeader(String name, String value) {
        return builder().header(name, value).build();
    }

    /**
     * Address the Live app {@code name} (the {@code loams-live-app} header, LV1 Task 13). To address
     * one app on every call, send the header from your {@code HttpClient}.
     */
    public static CallOptions withLiveApp(String name) {
        return withHeader(dev.loams.facade.Facade.LIVE_APP_HEADER, name);
    }

    /**
     * Read at a consistency token, which is how a caller who just wrote reads its own write
     * (R4).
     */
    public static CallOptions withConsistencyToken(String token) {
        return builder().consistencyToken(token).build();
    }

    /**
     * Fold this call's responses into a session store and attach the stored token to later reads.
     *
     * @param session the store, or {@code null} for the client's own session — which
     *     {@link Options#sessionConsistency()} turned on. Passing {@code null} rather than a
     *     separate "use the client's" marker is the Java answer to a nullable option: the
     *     invoker resolves it, so a caller cannot end up with a store that is neither.
     */
    public static CallOptions withSessionConsistency(ConsistencySession session) {
        return builder().sessionConsistency(session).build();
    }

    /**
     * Make a server stream reconnect from its last cursor on a retryable failure (R7).
     *
     * <p>Without it a stream is a plain iterator: it stops on a failure and reports it, which is
     * correct and is what R7 asks for when the call's retry class does not cover the failure.
     * With it, the stream re-opens from the last cursor the caller applied and carries on, so a
     * node restart is a hiccup rather than a gap.
     */
    public static CallOptions withStreamResume(StreamResume<?> resume) {
        return builder().streamResume(resume).build();
    }

    /** The per-call retry budget, or {@code null} for the client's. */
    public Integer maxRetries() {
        return maxRetries;
    }

    /** The per-call retry class override, or {@code null} for the binding's. */
    public Boolean retrySafe() {
        return retrySafe;
    }

    /** The caller's own idempotency key, or the empty string to have one minted. */
    public String idempotencyKey() {
        return idempotencyKey == null ? "" : idempotencyKey;
    }

    /** The headers this call adds. Never contains {@code Authorization}. */
    public Map<String, String> headers() {
        return headers;
    }

    /** The explicit consistency token, or the empty string. */
    public String consistencyToken() {
        return consistencyToken == null ? "" : consistencyToken;
    }

    /** The store this call folds its responses into, or {@code null} for none. */
    public ConsistencySession session() {
        return session;
    }

    /** The stream's resume, or {@code null} for none. */
    public StreamResume<?> streamResume() {
        return streamResume;
    }

    /**
     * The namespace this call fills into a request whose {@code namespace} field is empty (R11),
     * over the client's default; an explicit field on the request wins over both. {@code null}
     * follows the client.
     */
    public String namespace() {
        return namespace;
    }

    /**
     * This call's deadline (R12), or {@code null} for the client's (30 s for a unary call). {@link
     * java.time.Duration#ZERO} means none.
     */
    public java.time.Duration timeout() {
        return timeout;
    }

    /**
     * Whether the SDK may mint an idempotency key for this call (R3). {@code null} means yes; false
     * sends the request as built, unkeyed and so never retried unless it is safe.
     */
    public Boolean mintIdempotencyKey() {
        return mintIdempotencyKey;
    }

    /** These options with a per-call namespace. */
    public static CallOptions withNamespace(String namespace) {
        return builder().namespace(namespace).build();
    }

    /** These options with a per-call deadline. */
    public static CallOptions withTimeout(java.time.Duration timeout) {
        return builder().timeout(timeout).build();
    }

    /** Whether anything was overridden at all, so the common path skips a merge. */
    public boolean isEmpty() {
        return maxRetries == null
                && retrySafe == null
                && idempotencyKey == null
                && headers.isEmpty()
                && consistencyToken == null
                && session == null
                && streamResume == null
                && namespace == null
                && timeout == null
                && mintIdempotencyKey == null;
    }

    /** Collects overrides into one {@link CallOptions}. */
    public static final class Builder {

        private Integer maxRetries;
        private Boolean retrySafe;
        private String idempotencyKey;
        private final Map<String, String> headers = new LinkedHashMap<>();
        private String consistencyToken;
        private ConsistencySession session;
        private StreamResume<?> streamResume;
        private String namespace;
        private java.time.Duration timeout;
        private Boolean mintIdempotencyKey;

        private Builder() {}

        public Builder maxRetries(int value) {
            this.maxRetries = value;
            return this;
        }

        public Builder retrySafe(boolean value) {
            this.retrySafe = value;
            return this;
        }

        public Builder idempotencyKey(String value) {
            this.idempotencyKey = value;
            return this;
        }

        public Builder header(String name, String value) {
            this.headers.put(name, value);
            return this;
        }

        public Builder consistencyToken(String value) {
            this.consistencyToken = value;
            return this;
        }

        public Builder sessionConsistency(ConsistencySession value) {
            this.session = value;
            return this;
        }

        public Builder streamResume(StreamResume<?> value) {
            this.streamResume = value;
            return this;
        }

        public Builder namespace(String value) {
            this.namespace = value;
            return this;
        }

        public Builder timeout(java.time.Duration value) {
            this.timeout = value;
            return this;
        }

        public Builder mintIdempotencyKey(boolean value) {
            this.mintIdempotencyKey = value;
            return this;
        }

        /** Copies every override of {@code options} into this builder. */
        public Builder from(CallOptions options) {
            if (options == null) {
                return this;
            }
            if (options.maxRetries != null) {
                maxRetries = options.maxRetries;
            }
            if (options.retrySafe != null) {
                retrySafe = options.retrySafe;
            }
            if (options.idempotencyKey != null) {
                idempotencyKey = options.idempotencyKey;
            }
            headers.putAll(options.headers);
            if (options.consistencyToken != null) {
                consistencyToken = options.consistencyToken;
            }
            if (options.session != null) {
                session = options.session;
            }
            if (options.streamResume != null) {
                streamResume = options.streamResume;
            }
            if (options.namespace != null) {
                namespace = options.namespace;
            }
            if (options.timeout != null) {
                timeout = options.timeout;
            }
            if (options.mintIdempotencyKey != null) {
                mintIdempotencyKey = options.mintIdempotencyKey;
            }
            return this;
        }

        public CallOptions build() {
            return new CallOptions(this);
        }
    }

    /**
     * Every option, so a variadic-style signature can pass them through.
     *
     * <p>This is how the module methods accept {@code CallOptions...} without each of them having
     * to merge: they hand the array straight here.
     */
    public static CallOptions merge(CallOptions... options) {
        if (options == null || options.length == 0) {
            return none();
        }
        List<CallOptions> present = new ArrayList<>();
        for (CallOptions option : options) {
            if (option != null && !option.isEmpty()) {
                present.add(option);
            }
        }
        if (present.isEmpty()) {
            return none();
        }
        if (present.size() == 1) {
            return present.get(0);
        }
        Builder builder = builder();
        for (CallOptions option : present) {
            if (option.maxRetries != null) {
                builder.maxRetries(option.maxRetries);
            }
            if (option.retrySafe != null) {
                builder.retrySafe(option.retrySafe);
            }
            if (option.idempotencyKey != null) {
                builder.idempotencyKey(option.idempotencyKey);
            }
            builder.headers.putAll(option.headers);
            if (option.consistencyToken != null) {
                builder.consistencyToken(option.consistencyToken);
            }
            if (option.session != null) {
                builder.sessionConsistency(option.session);
            }
            if (option.streamResume != null) {
                builder.streamResume(option.streamResume);
            }
            if (option.namespace != null) {
                builder.namespace(option.namespace);
            }
            if (option.timeout != null) {
                builder.timeout(option.timeout);
            }
            if (option.mintIdempotencyKey != null) {
                builder.mintIdempotencyKey(option.mintIdempotencyKey);
            }
        }
        return builder.build();
    }
}