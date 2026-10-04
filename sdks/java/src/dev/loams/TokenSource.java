package dev.loams;

/**
 * Where a call's bearer comes from (design §44 §7.4, D608; runtime contract R1).
 *
 * <p>Tokens travel in {@code Authorization: Bearer} and <b>never</b> in a URL: a query string
 * ends up in proxy logs, in browser history and in {@code Referer}. A {@code 401} carrying
 * {@code reason = token_expired} triggers one refresh and one retry; that logic lives in the call
 * path, so a source stays a source and nothing here has to know about retries.
 *
 * <p>Both methods are on the interface rather than one being optional because Java has no way to
 * ask "does this implementation have a method" without reflection, and an API key — which has
 * nothing to refresh — is expressed by returning {@code null} from {@link #refresh()}. That is
 * the contract's no-op refresh, stated as behaviour rather than as a missing capability.
 */
public interface TokenSource {

    /**
     * The bearer to send, or the empty string to send no credential at all.
     *
     * <p>Called once per <em>attempt</em>, not once per call, because R1's refresh has to change
     * it between two attempts of the same logical call. A source may therefore return a different
     * token each time.
     */
    String token();

    /**
     * Fetch a new token after the server reported the current one expired.
     *
     * <p>Returning {@code null} from a source that cannot refresh — an API key, an environment
     * variable, a token the caller owns — makes the runtime's refresh a no-op, which is exactly
     * what R1 asks for.
     */
    void refresh();
}