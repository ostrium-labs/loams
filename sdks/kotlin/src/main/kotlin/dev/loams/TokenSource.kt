package dev.loams

/**
 * Credentials: where a bearer comes from, and how R1's refresh is applied (D608).
 *
 * # The two halves of R1
 *
 * **Where the token comes from** is [TokenSource]. **When it is refreshed** is the
 * retry loop's, and it is deliberately *not* here: a refresh costs a network call, so
 * it belongs where the retry decision is made rather than inside the token getter,
 * where a caller reading the token three times would trigger three refreshes.
 *
 * ## Why the bearer is fetched per attempt
 *
 * That is the whole of the mechanism, and it is why [TokenSource.token] is called
 * inside the attempt rather than once before the loop: the refresh changes the token,
 * and the retry has to carry the new one. A bearer computed once per call would be the
 * stale one on every retry after a refresh, and the failure would look like "the
 * refresh did not work" rather than "the refresh was never used".
 */
interface TokenSource {
    /**
     * Whether this source can refresh at all.
     *
     * A source that cannot — an API key, a service-account token the process holds —
     * makes R1's refresh a no-op **and** makes the retry skipped rather than spent on
     * a request that cannot work.
     */
    val canRefresh: Boolean

    /**
     * The current bearer, or the empty string for none.
     *
     * Called **once per attempt**, so an implementation must be safe to call repeatedly
     * within one call. It must not itself refresh: a refresh that costs a network call
     * belongs behind [refresh], which the retry loop calls at most once.
     */
    fun token(): String

    /**
     * Obtains a new token, or throws.
     *
     * A failure here is reported as itself rather than swallowed: it is a real failure
     * with its own cause, and reporting the original expiry would hide why nothing
     * improved. Only called when [canRefresh] is true.
     */
    fun refresh()
}

/**
 * A token that never changes: an API key, or a bearer the process was handed at
 * startup.
 *
 * [canRefresh] is false, which is the whole difference from [RefreshingTokenSource] and
 * the reason R1's retry is skipped for it: repeating a request authenticated with an
 * API key that the server rejected asks the same question again.
 */
class StaticTokenSource(private val value: String) : TokenSource {
    override val canRefresh: Boolean get() = false

    override fun token(): String = value

    override fun refresh() {
        throw UnsupportedOperationException(
            "a static token source cannot refresh; a caller that needs R1's refresh must supply a source " +
                "whose canRefresh is true"
        )
    }
}

/**
 * A source that mints a token and reuses it until something asks for a new one.
 *
 * # Why it caches
 *
 * A token that changed on every read would be a refresh storm the server sees: the
 * client would mint a new credential per attempt, per call, forever. So a minted token
 * is reused until [refresh] — which is exactly the "within its window" half the
 * conformance test asserts — and a token source's job is to be cheap to ask.
 *
 * # Why it has no clock
 *
 * There is no expiry claim here, because the SDK does not know one: the token's own
 * `exp` is inside a JWS this code does not verify, and guessing an expiry from a local
 * clock would refresh early forever or late once. So "expired" means **the server said
 * so** — `token_expired` — and that is a question the retry loop answers. This class
 * mints on demand and on [refresh], and nothing else, which means it has no clock to
 * get wrong.
 */
class RefreshingTokenSource(private val mint: () -> String) : TokenSource {
    override val canRefresh: Boolean get() = true

    /** The minted token, or null before the first read. */
    @Volatile
    private var current: String? = null

    /** The token, minting one if this is the first read. */
    override fun token(): String {
        val existing = current
        if (existing != null) {
            return existing
        }
        return synchronized(this) {
            // Re-checked inside the lock: two threads arriving together must not both
            // mint, or one call's refresh would silently invalidate the other's.
            current ?: mint().also { current = it }
        }
    }

    /** Obtains a new token for the next read. */
    override fun refresh() {
        synchronized(this) {
            current = mint()
        }
    }
}

/**
 * A source that delegates to callbacks, for a caller whose token lives somewhere the
 * SDK does not know about — a Keystore, a session manager, an OAuth library.
 *
 * [mint] is optional: a source that cannot refresh simply says so, which is how an
 * API-key-backed credential ends a retry rather than spending it.
 */
class CallbackTokenSource(
    override val canRefresh: Boolean,
    private val read: () -> String,
    private val mint: (() -> String)? = null,
) : TokenSource {
    override fun token(): String = read()

    override fun refresh() {
        val mint = mint ?: throw UnsupportedOperationException(
            "this token source was built without a mint, so it cannot refresh"
        )
        mint()
    }
}