package dev.loams;

import dev.loams.facade.Reason;
import java.util.Map;

/**
 * {@code unauthenticated} with reason {@code token_expired}: a token the server rejected as
 * expired (D608, R1).
 *
 * <p>The runtime refreshes once and retries once. A caller normally never sees this type at all,
 * because the call path intercepts it; a second expiry reaches them as this, and that is the
 * point — a refresh loop over a token that keeps being rejected would spin.
 *
 * <p>It extends {@link UnauthenticatedException}, so one {@code catch} for the class still
 * covers every expired token. That inheritance is the whole reason it is a subclass rather than
 * a sibling: in Java the branch is {@code instanceof}, and a caller who wrote
 * {@code catch (UnauthenticatedException)} must not silently stop matching.
 */
public class TokenExpiredException extends UnauthenticatedException {

    private static final long serialVersionUID = 1L;

    public TokenExpiredException(
            Code code,
            Reason reason,
            String unknownReason,
            Map<String, String> metadata,
            String hint,
            String rpc,
            Throwable cause) {
        super(code, reason, unknownReason, metadata, hint, rpc, cause);
    }
}