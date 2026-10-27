package dev.loams;

import java.net.http.HttpClient;

/**
 * How to build a {@link Client}.
 *
 * <p>A builder rather than a constructor with six arguments, because every field is optional and
 * a caller who wants to set the fourth should not have to name the other three.
 *
 * <pre>{@code
 * Client client = Client.builder()
 *         .endpoint("https://acme.loams.dev")
 *         .auth(TokenSources.apiKey(System.getenv("LOAMS_API_KEY")))
 *         .build();
 * }</pre>
 */
public final class Options {

    private final String endpoint;
    private final TokenSource auth;
    private final Protocol protocol;
    private final HttpClient httpClient;
    private final Integer maxRetries;
    private final boolean noRetries;
    private final boolean sessionConsistency;
    private final String namespace;
    private final java.time.Duration timeout;
    private final boolean json;

    private Options(Builder builder) {
        this.endpoint = builder.endpoint;
        this.auth = builder.auth;
        this.protocol = builder.protocol;
        this.httpClient = builder.httpClient;
        this.maxRetries = builder.maxRetries;
        this.noRetries = builder.noRetries;
        this.sessionConsistency = builder.sessionConsistency;
        this.namespace = builder.namespace;
        this.timeout = builder.timeout;
        this.json = builder.json;
    }

    /**
     * Whether unary calls use the proto3 JSON mapping ({@code application/json}) rather than
     * binary protobuf. Off by default: binary is smaller and faster. JSON is for a proxy or a log
     * that has to read the bodies, and for replaying a JSON recording. Server streams stay
     * binary either way.
     */
    public boolean json() {
        return json;
    }

    public static Builder builder() {
        return new Builder();
    }

    /**
     * The instance's base URL, for example {@code https://acme.loams.dev}.
     *
     * <p>A loopback stack is {@code http://127.0.0.1:8080}. Required: an empty endpoint is a usage
     * error rather than a request to localhost, because a client that quietly talks to the wrong
     * instance is worse than one that does not start.
     */
    public String endpoint() {
        return endpoint;
    }

    /**
     * The bearer source: {@link TokenSources#apiKey} for a script or a CI job,
     * {@link TokenSources#staticToken}, {@link TokenSources#env()}, an {@link OidcExchange}, or
     * any {@link TokenSource} of your own.
     *
     * <p>Omitted means an unauthenticated client, which is what {@code client.instance()
     * .getInstance(...)} needs anyway.
     */
    public TokenSource auth() {
        return auth;
    }

    /**
     * The wire protocol. Unset means {@link Protocol#CONNECT}, which is what the design asks every
     * language to default to (D612).
     */
    public Protocol protocol() {
        return protocol == null ? Protocol.CONNECT : protocol;
    }

    /**
     * The HTTP client the transport uses. It wins over everything else, so a caller with its own
     * TLS config, proxy or timeout supplies it.
     */
    public HttpClient httpClient() {
        return httpClient;
    }

    /**
     * The retries after the first attempt, for every call.
     *
     * <p><b>{@code null} means {@link Retry#DEFAULT_MAX_RETRIES} (3), not none.</b> The design's
     * number is the client's number: an SDK that silently did not retry unless it was configured to
     * would be an SDK whose safe-by-default behaviour is a sharp edge, and a caller writing
     * {@code Options.builder().endpoint("…")} would get something other than what every other SDK
     * does. That is exactly the problem Java's {@code int} zero value causes, and it is solved by
     * an {@link Integer} plus an explicit opt-out:
     *
     * <pre>{@code
     * Client.builder().endpoint("…").noRetries(true).build();
     * }</pre>
     *
     * <p>Note the asymmetry with {@link CallOptions#withMaxRetries(int)}, where zero is
     * unambiguous: writing the number on <em>a call</em> is a deliberate choice, so there zero
     * means none; here it is indistinguishable from not asking.
     */
    public Integer maxRetries() {
        return maxRetries;
    }

    /** Whether automatic retries are off for the whole client. */
    public boolean noRetries() {
        return noRetries;
    }

    /**
     * Whether to hold a session consistency token across calls (D609).
     *
     * <p><b>Off by default</b>: every read is then {@code STRONG} on its own, which is correct but
     * does not give read-your-writes across processes. Turn it on when one process is both writing
     * and reading, and remember that the store keeps the token it was given rather than merging —
     * see {@link ConsistencySession}.
     */
    public boolean sessionConsistency() {
        return sessionConsistency;
    }

    /** Collects an {@link Options}. */
    /**
     * The namespace an app request with an empty {@code namespace} field goes to (R11), or {@code
     * null}. An explicit argument always wins; with neither, an app call raises {@code
     * invalid_argument} before any network call.
     */
    public String namespace() {
        return namespace;
    }

    /**
     * The unary deadline (R12), or {@code null} for 30 s. {@link java.time.Duration#ZERO} turns it
     * off. A call that runs out of it raises {@link DeadlineExceededException} with reason {@code
     * client_deadline_exceeded} and is not retried.
     */
    public java.time.Duration timeout() {
        return timeout;
    }

    /**
     * A builder from the environment (R1): {@code LOAMS_ENDPOINT}, then {@code LOAMS_API_KEY} or
     * {@code LOAMS_TOKEN}, and {@code LOAMS_NAMESPACE}.
     *
     * @throws IllegalStateException {@code LOAMS_ENDPOINT} is not set
     */
    public static Builder fromEnv() {
        String endpoint = System.getenv("LOAMS_ENDPOINT");
        if (endpoint == null || endpoint.isBlank()) {
            throw new IllegalStateException("loams: LOAMS_ENDPOINT is not set; it is the instance's base URL");
        }
        Builder builder = builder().endpoint(endpoint);
        String key = System.getenv("LOAMS_API_KEY");
        String token = System.getenv("LOAMS_TOKEN");
        if (key != null && !key.isEmpty()) {
            builder.auth(TokenSources.apiKey(key));
        } else if (token != null && !token.isEmpty()) {
            builder.auth(TokenSources.staticToken(token));
        }
        String ns = System.getenv("LOAMS_NAMESPACE");
        if (ns != null && !ns.isEmpty()) {
            builder.namespace(ns);
        }
        return builder;
    }

    public static final class Builder {

        private String endpoint;
        private TokenSource auth;
        private Protocol protocol;
        private HttpClient httpClient;
        private Integer maxRetries;
        private boolean noRetries;
        private boolean sessionConsistency;
        private String namespace;
        private java.time.Duration timeout;
        private boolean json;

        private Builder() {}

        /** Required. See {@link Options#endpoint()}. */
        public Builder endpoint(String value) {
            this.endpoint = value;
            return this;
        }

        /** See {@link Options#auth()}. */
        public Builder auth(TokenSource value) {
            this.auth = value;
            return this;
        }

        /** See {@link Options#protocol()}. */
        public Builder protocol(Protocol value) {
            this.protocol = value;
            return this;
        }

        /** See {@link Options#httpClient()}. */
        public Builder httpClient(HttpClient value) {
            this.httpClient = value;
            return this;
        }

        /** See {@link Options#maxRetries()}. */
        public Builder maxRetries(Integer value) {
            this.maxRetries = value;
            return this;
        }

        /** See {@link Options#noRetries()}. */
        public Builder noRetries(boolean value) {
            this.noRetries = value;
            return this;
        }

        /** See {@link Options#sessionConsistency()}. */
        public Builder sessionConsistency(boolean value) {
            this.sessionConsistency = value;
            return this;
        }

        /**
         * @throws IllegalArgumentException when no endpoint was given
         */
        public Builder namespace(String value) {
            this.namespace = value;
            return this;
        }

        public Builder timeout(java.time.Duration value) {
            this.timeout = value;
            return this;
        }

        /** See {@link Options#json()}. */
        public Builder json(boolean value) {
            this.json = value;
            return this;
        }

        public Options build() {
            if (endpoint == null || endpoint.isBlank()) {
                // A usage error, not a request to localhost: a client that quietly talks to the
                // wrong instance is worse than one that does not start.
                throw new IllegalArgumentException(
                        "loams: Options.endpoint is empty; it is the instance's base URL, for"
                                + " example https://acme.loams.dev");
            }
            return new Options(this);
        }
    }
}