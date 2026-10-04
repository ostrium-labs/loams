package dev.loams;

/** The token sources the SDK ships, and the ones a caller supplies. */
public final class TokenSources {

    private TokenSources() {}

    /**
     * A Loams API key: what a script or a CI job has.
     *
     * <p>The key does not expire, so {@link #refresh()} is a no-op — which is exactly the no-op
     * R1 describes for "a source that cannot refresh". A {@code 401 token_expired} therefore
     * reaches the caller as a {@link TokenExpiredException} rather than being retried, because
     * retrying with the same key would only fail again.
     */
    public static TokenSource apiKey(String key) {
        String value = key == null ? "" : key;
        return new TokenSource() {
            @Override
            public String token() {
                return value;
            }

            @Override
            public void refresh() {
                // An API key does not expire.
            }

            @Override
            public String toString() {
                // Never the key itself: this string ends up in logs.
                return "apiKey(***)";
            }
        };
    }

    /** A bearer that is already valid, for a caller that manages its own. */
    public static TokenSource staticToken(String token) {
        String value = token == null ? "" : token;
        return new TokenSource() {
            @Override
            public String token() {
                return value;
            }

            @Override
            public void refresh() {
                // The caller owns this token's lifetime.
            }

            @Override
            public String toString() {
                return "staticToken(***)";
            }
        };
    }

    /**
     * A Loams API key read from the environment, {@code LOAMS_API_KEY} then {@code LOAMS_TOKEN}.
     *
     * <p>The environment is read on <b>every</b> call rather than once at construction, so a
     * process that receives its credentials after the client is built — a sidecar, a test — still
     * authenticates.
     *
     * @param lookup reads one variable. A field so a test can supply an environment without
     *     touching the process's own, and so a caller who keeps its configuration somewhere else
     *     can say so.
     */
    public static TokenSource env(java.util.function.Function<String, String> lookup) {
        return new EnvTokenSource(lookup == null ? System::getenv : lookup);
    }

    /** The environment's own variables. */
    public static TokenSource env() {
        return env(null);
    }

    /**
     * A source that caches a token and calls {@code fetch} when asked to refresh.
     *
     * <p>This is the shape every refreshing source has. One in-flight refresh is shared by
     * concurrent callers, so a burst of {@code 401}s produces <b>one</b> token exchange rather
     * than one per request. That is not a micro-optimisation: an instance rejecting every token
     * because it is stale would otherwise be hit with one exchange per in-flight call, which is
     * how a credential rotation turns into a self-inflicted denial of service.
     */
    public static RefreshingTokenSource refreshing(java.util.function.Supplier<String> fetch) {
        return new RefreshingTokenSource(fetch);
    }

    /** The variables {@link #env()} reads, in order. */
    public static String[] envNames() {
        return new String[] {"LOAMS_API_KEY", "LOAMS_TOKEN"};
    }

    private static final class EnvTokenSource implements TokenSource {

        private final java.util.function.Function<String, String> lookup;

        EnvTokenSource(java.util.function.Function<String, String> lookup) {
            this.lookup = lookup;
        }

        @Override
        public String token() {
            for (String name : envNames()) {
                String value = lookup.apply(name);
                if (value != null && !value.isEmpty()) {
                    return value;
                }
            }
            return "";
        }

        @Override
        public void refresh() {
            // An environment variable is not something the SDK can renew. A caller who wants a
            // refreshed token points this at their own source.
        }

        @Override
        public String toString() {
            return "env" + java.util.List.of(envNames());
        }
    }

}
