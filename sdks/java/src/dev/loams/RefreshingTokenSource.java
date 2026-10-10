package dev.loams;

import java.util.function.Supplier;

/**
 * A token source that caches a token and calls {@code fetch} when asked to refresh (R1).
 *
 * <p>This is the shape every refreshing source has. One in-flight refresh is shared by
 * concurrent callers, so a burst of {@code 401}s produces <b>one</b> token exchange rather than
 * one per request. That is not a micro-optimisation: an instance that is rejecting every token
 * because it is stale would otherwise be hit with one exchange per in-flight call, which is how a
 * credential rotation turns into a self-inflicted denial of service.
 *
 * <p>Waiting on someone else's refresh is the trade R1 wants and it is deliberate: the first
 * caller's {@code fetch} runs once, every other caller blocks until it has a token, and a caller
 * that arrives after the exchange has finished starts the next one. So a burst costs one
 * exchange and a steady stream costs one per {@code 401}, which is the intended behaviour.
 */
public final class RefreshingTokenSource implements TokenSource {

    private final Supplier<String> fetch;

    /** Guards {@link #cache} and {@link #inFlight}, and is what waiters block on. */
    private final Object lock = new Object();

    /** The refresh currently running, or {@code null}. Guarded by {@link #lock}. */
    private Exchange inFlight;

    /** The last token minted. Guarded by {@link #lock}. */
    private String cache = "";

    /**
     * @param fetch mints a new token. Called at most once per {@link #refresh()} that starts an
     *     exchange, however many callers are waiting on it.
     */
    public RefreshingTokenSource(Supplier<String> fetch) {
        this.fetch = fetch;
    }

    @Override
    public String token() {
        synchronized (lock) {
            if (!cache.isEmpty()) {
                return cache;
            }
        }
        // A source whose cache starts empty would send no credential at all, and an instance that
        // requires one answers `unauthenticated` — which the call path treats as "the token
        // expired" and retries, with still no credential. So the first `token()` fetches.
        refresh();
        synchronized (lock) {
            return cache;
        }
    }

    @Override
    public void refresh() {
        Exchange mine;
        boolean owns;
        synchronized (lock) {
            if (inFlight != null) {
                mine = inFlight;
                owns = false;
            } else {
                mine = new Exchange();
                inFlight = mine;
                owns = true;
            }
        }
        if (!owns) {
            await(mine);
            return;
        }
        try {
            String token = fetch.get();
            synchronized (lock) {
                if (token != null && !token.isEmpty()) {
                    cache = token;
                }
                inFlight = null;
                mine.done = true;
                lock.notifyAll();
            }
        } catch (RuntimeException | Error e) {
            // The exchange failed, so every waiter must be told rather than left blocked
            // forever. The failure is rethrown to the owner and to each waiter, because a
            // refresh that silently did nothing would leave the call retrying with the stale
            // token it was just told to discard.
            synchronized (lock) {
                inFlight = null;
                mine.done = true;
                mine.failure = e;
                lock.notifyAll();
            }
            throw e;
        }
    }

    /** Wait for the exchange another caller is running. */
    private void await(Exchange exchange) {
        synchronized (lock) {
            while (!exchange.done) {
                try {
                    lock.wait();
                } catch (InterruptedException e) {
                    Thread.currentThread().interrupt();
                    throw Errors.internal(
                            "",
                            "interrupted while waiting for another thread's token refresh",
                            e);
                }
            }
        }
        if (exchange.failure != null) {
            rethrow(exchange.failure);
        }
    }

    /**
     * Re-throw what the exchange failed with, keeping an {@link Error} an {@link Error}.
     *
     * <p>A waiter's {@code refresh()} returns {@code void}, so it cannot hand the caller an
     * {@link Error}; swallowing one would turn an {@link OutOfMemoryError} into a wrong answer.
     */
    private static void rethrow(Throwable failure) {
        if (failure instanceof RuntimeException runtime) {
            throw runtime;
        }
        if (failure instanceof Error error) {
            throw error;
        }
        throw Errors.internal("", "the token refresh failed: " + failure, failure);
    }

    /** The cached token, or the empty string. */
    public String cached() {
        synchronized (lock) {
            return cache;
        }
    }

    @Override
    public String toString() {
        return "refreshing";
    }

    /** One exchange, shared by every caller that arrives while it runs. */
    private static final class Exchange {

        private boolean done;

        private Throwable failure;
    }
}