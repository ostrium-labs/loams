package dev.loams;

import com.google.protobuf.Message;
import com.google.protobuf.Parser;
import dev.loams.connect.ConnectStream;
import dev.loams.connect.ConnectTransport;
import dev.loams.facade.CallBinding;
import java.io.IOException;
import java.io.UncheckedIOException;
import java.util.LinkedHashMap;
import java.util.Map;
import java.util.function.Function;

/**
 * The call path: the one place a facade method becomes an RPC (design §44 §7.4; runtime contract
 * R1–R4).
 *
 * <p>It does four things the generated facade cannot, and nothing else:
 *
 * <ul>
 *   <li>attaches the bearer from the client's {@link TokenSource};
 *   <li>retries on the call's class from the generated bindings, with M1.6's backoff numbers, and
 *       refreshes the token once on {@code token_expired};
 *   <li>gives a mutating call an idempotency key once per logical call and reuses it on every
 *       retry, so a retried write is the same write (D610);
 *   <li>turns whatever comes back into the typed {@link LoamsException} hierarchy, so a caller
 *       branches on a {@code reason} and never on a message.
 * </ul>
 *
 * <h2>The deadline, and where it shows up</h2>
 *
 * <p>Every attempt is made with the caller's own thread, and so is every wait between attempts,
 * which buys the same three properties the Go SDK gets from its context:
 *
 * <ul>
 *   <li><b>A cancelled call costs at most the attempt in flight.</b> The retry loop checks the
 *       thread's interrupt before spending another attempt and sleeps in slices, so a shutdown
 *       does not leave a thread waiting out a 2 s backoff.
 *   <li><b>A deadline covers the whole call, not each attempt.</b> The caller wraps the whole
 *       iteration in one future or one timeout, which bounds the retries and the backoff
 *       together — that is what "this call may take at most five seconds" means.
 *   <li><b>A cancellation keeps its own code.</b> An interrupt becomes {@link Code#CANCELED}
 *       rather than {@link Code#UNKNOWN}, because a cancellation is the one failure the caller
 *       can always act on.
 * </ul>
 *
 * <p>The Java-specific hazard is that an {@link InterruptedException} is checked and can be
 * swallowed by a lambda. {@link #send} therefore never catches it, and every loop that sleeps
 * re-sets the interrupt flag before giving up, so the cancellation is still visible to code
 * above.
 */
public final class CallInvoker {

    /** The client-wide default for how long one attempt waits for its response headers. */
    private final ConnectTransport transport;

    /** The bearer source, or {@code null} for an unauthenticated client. */
    private final TokenSource source;

    /** The client's default retry budget; a call overrides it. */
    private final int maxRetries;

    /** The client's session store, or {@code null} when session consistency is off. */
    private final ConsistencySession clientSession;

    CallInvoker(
            ConnectTransport transport,
            TokenSource source,
            int maxRetries,
            ConsistencySession clientSession) {
        this.transport = transport;
        this.source = source;
        this.maxRetries = maxRetries;
        this.clientSession = clientSession;
    }

    /** The client's default retry budget. */
    public int maxRetries() {
        return maxRetries;
    }

    /**
     * Make one unary call with the whole runtime contract applied.
     *
     * <p>The request is keyed <b>once</b>, before the first attempt, and the same message goes to
     * every attempt: that is R3, and it is why a retried mutation is one write rather than two.
     *
     * @param binding the generated binding, which supplies the RPC path and the retry class
     * @param request the caller's message. Never mutated.
     * @param options the per-call overrides
     * @param parser the response message's parser
     * @param <Res> the response message type
     * @return the response message
     */
    public <Res extends Message> Res unary(
            CallBinding binding, Message request, CallOptions options, Parser<Res> parser) {
        Idempotency.KeyedRequest keyed =
                Idempotency.applyIdempotencyKey(
                        request, options.idempotencyKey(), binding.takesIdempotencyKey());
        // The keyed message is the *same value* on every attempt, which is the whole of R3.
        Message message = keyed.request();

        ConsistencySession session = sessionFor(options);
        RetryPlan plan = plan(binding, options, keyed.keyed());
        String token = consistencyToken(options, session);

        CallInvoker.Attempt send =
                (attempt, refreshed) -> {
                            try {
                                return transport.unary(
                                        binding.service(),
                                        binding.method(),
                                        message.toByteArray(),
                                        headers(options, token),
                                        false);
                            } catch (IOException e) {
                                throw new UncheckedIOException(e);
                            } catch (InterruptedException e) {
                                // An interrupt is the caller's own cancellation reaching the
                                // transport. It is re-set on the thread before it is wrapped, so
                                // the cancellation is still visible to code above this loop.
                                Thread.currentThread().interrupt();
                                throw new UncheckedIOException(new java.io.InterruptedIOException(
                                        "the caller interrupted the call: " + e.getMessage()));
                            }
                        };
        Object result = retryLoop(binding.rpc(), plan, send);

        ConnectTransport.UnaryResponse response = (ConnectTransport.UnaryResponse) result;
        if (session != null) {
            // A store that cannot merge a token counts it and carries on: the RPC succeeded, and
            // turning that into a failure would make a caller that retries on it perform the
            // write twice.
            String returned =
                    response.header(ConsistencySession.HEADER.toLowerCase(java.util.Locale.ROOT));
            if (returned != null && !returned.isEmpty()) {
                session.record(returned);
            }
        }
        try {
            return parser.parseFrom(response.body());
        } catch (IOException e) {
            throw Errors.internal(
                    binding.rpc(),
                    "the response from " + binding.rpc() + " is not a " + parser.getClass().getSimpleName(),
                    e);
        }
    }

    /**
     * Open one server stream, with the errors mapped and R1's single refresh applied.
     *
     * <p>There is no resume here and no cursor tracking; {@link CallOptions#withStreamResume}
     * adds both, and it is an option so the same module method serves both.
     *
     * <p>The error mapping is not optional, though: a refusal on a stream arrives <b>inside</b>
     * the Connect envelope rather than as an HTTP status — the corpus's {@code live_watch} case
     * is exactly that, a 200 with an end-of-stream frame carrying the error — so a caller
     * iterating the raw frames would see a failure with no {@code reason}, which is the one place
     * in an SDK where {@code reason} could go missing.
     */
    public <T> Receiver<T> openServerStream(
            CallBinding binding,
            Message request,
            CallOptions options,
            Function<byte[], T> decode,
            Message messageToSend) {
        String token = consistencyToken(options, sessionFor(options));

        // R1 on a stream: one refresh and one re-open, and only while nothing has been yielded.
        // Once messages are flowing the caller is holding a position in the stream, and replaying
        // from the start would duplicate everything they have already seen — the resume is the
        // only correct answer, and it is the caller's job.
        Receiver<T> receiver;
        boolean refreshed = false;
        while (true) {
            try {
                receiver = openOnce(binding, options, token, decode, messageToSend);
                break;
            } catch (LoamsException mapped) {
                if (refreshed
                        || source == null
                        || !(mapped instanceof TokenExpiredException)) {
                    throw mapped;
                }
                refreshed = true;
                try {
                    source.refresh();
                } catch (RuntimeException refreshFailure) {
                    throw Errors.toLoamsException(refreshFailure, binding.rpc());
                }
            }
        }
        return receiver;
    }

    /** Open the wire stream once and wrap it. */
    private <T> Receiver<T> openOnce(
            CallBinding binding,
            CallOptions options,
            String consistency,
            Function<byte[], T> decode,
            Message message) {
        ConnectStream stream;
        try {
            stream =
                    transport.openStream(
                            binding.service(),
                            binding.method(),
                            message.toByteArray(),
                            headers(options, consistency));
        } catch (IOException e) {
            throw Errors.toLoamsException(e, binding.rpc());
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            throw Errors.toLoamsException(e, binding.rpc());
        }
        return new WireReceiver<>(binding.rpc(), stream, decode);
    }

    /**
     * Run {@code send} until it answers or the plan says stop, and return what it threw as a
     * {@link LoamsException}.
     *
     * <p>{@link RetryPlan} and the {@link Attempt} record are public so the conformance suite can
     * drive this with a stub send: the retry policy, the idempotency-key lifecycle and the
     * refresh-once behaviour are the SDK's own logic and are worth pinning without a server.
     */
    static Object retryLoop(String rpc, RetryPlan plan, Attempt send) {
        boolean refreshed = false;
        for (int attempt = 0; ; attempt++) {
            try {
                return send.attempt(attempt, refreshed);
            } catch (RuntimeException thrown) {
                LoamsException mapped = Errors.toLoamsException(thrown, rpc);
                boolean interrupted = Thread.currentThread().isInterrupted();
                // R1: a rejection whose reason is `token_expired` gets exactly one refresh and
                // one retry. A source that cannot refresh (an API key) makes this a no-op, and a
                // second expiry is reported rather than looped on.
                if (mapped instanceof TokenExpiredException && !refreshed && plan.onRefresh() != null) {
                    refreshed = true;
                    try {
                        plan.onRefresh().run();
                    } catch (RuntimeException refreshFailure) {
                        throw Errors.toLoamsException(refreshFailure, rpc);
                    }
                    // The retry is not charged to the call's budget: it is the same logical
                    // call, and a caller who asked for three retries asked for three retries of
                    // the request, not three including a credential refresh.
                    attempt--;
                    continue;
                }
                if (interrupted) {
                    // The caller gave up while a retryable failure was outstanding. Both facts
                    // matter, so the error carries both — see callerGaveUp().
                    throw mapped.withCallerGaveUp(
                            new InterruptedException("the caller interrupted the call"));
                }
                if (!Retry.shouldRetry(interrupted, mapped, plan.retrySafe(), attempt, plan.maxRetries())) {
                    throw mapped;
                }
                try {
                    Retry.sleep(Retry.backoff(attempt, java.time.Duration.ZERO));
                } catch (InterruptedException e) {
                    // The caller gave up during the backoff rather than before it. Same
                    // treatment: the retry still happened, the deadline still ended the call.
                    Thread.currentThread().interrupt();
                    throw mapped.withCallerGaveUp(e);
                }
            }
        }
    }

    /**
     * The retry plan for one call, given whether the call ended up keyed.
     *
     * <p>D610: a {@code safe} call always retries; a mutation retries once it carries an
     * idempotency key, which {@link Idempotency} has just decided.
     */
    RetryPlan plan(CallBinding binding, CallOptions options, boolean keyed) {
        int budget = options.maxRetries() == null ? maxRetries : Math.max(0, options.maxRetries());
        boolean safe = binding.retrySafe() || keyed;
        if (options.retrySafe() != null) {
            safe = options.retrySafe();
        }
        return new RetryPlan(safe, budget, source == null ? null : source::refresh);
    }

    /**
     * The {@code Authorization} header value for one attempt, plus this call's own headers.
     *
     * <p>The token is fetched <b>per attempt</b>, not per call, because R1's refresh has to
     * change it between two attempts of the same logical call.
     *
     * <p>{@code Authorization} is set last and overwrites anything the caller put in the map: a
     * caller-supplied bearer would be one the runtime cannot refresh, and R1 would be
     * unreachable.
     */
    private Map<String, String> headers(CallOptions options, String consistency) {
        Map<String, String> headers = new LinkedHashMap<>(options.headers());
        if (source != null) {
            String token = source.token();
            if (token != null && !token.isEmpty()) {
                headers.put("Authorization", "Bearer " + token);
            }
        }
        if (consistency != null && !consistency.isEmpty()) {
            headers.put(ConsistencySession.HEADER, consistency);
        }
        return headers;
    }

    /** The store a call asked for: its own, the client's, or none. */
    private ConsistencySession sessionFor(CallOptions options) {
        if (options.session() != null) {
            return options.session();
        }
        return clientSession;
    }

    /**
     * Which token a read goes out with: an explicit one wins, otherwise the session's — which is
     * what makes {@code withSessionConsistency} read-your-writes rather than record a token and
     * never send it (R4).
     */
    private static String consistencyToken(CallOptions options, ConsistencySession session) {
        if (!options.consistencyToken().isEmpty()) {
            return options.consistencyToken();
        }
        return session == null ? "" : session.current();
    }

    /**
     * One attempt of a call.
     *
     * @param attempt the zero-based attempt number
     * @param refreshed whether R1's single refresh has already happened
     */
    @FunctionalInterface
    public interface Attempt {
        Object attempt(int attempt, boolean refreshed);
    }

    /**
     * Everything the retry loop needs, so the policy can be tested without a transport.
     *
     * @param retrySafe the call's class from the generated binding, or {@code true} once the call
     *     has been keyed
     * @param maxRetries the retries after the first attempt
     * @param onRefresh the client's refresh, or {@code null} for a client with no
     *     {@link TokenSource}. A {@code 401 token_expired} triggers it exactly once.
     */
    public record RetryPlan(boolean retrySafe, int maxRetries, Runnable onRefresh) {}

    /** Decodes a wire stream into messages, and turns its end-of-stream failure into a mapped one. */
    private static final class WireReceiver<T> implements Receiver<T> {

        private final String rpc;
        private final ConnectStream stream;
        private final Function<byte[], T> decode;

        WireReceiver(String rpc, ConnectStream stream, Function<byte[], T> decode) {
            this.rpc = rpc;
            this.stream = stream;
            this.decode = decode;
        }

        @Override
        public T next() {
            try {
                byte[] bytes = stream.next();
                return bytes == null ? null : decode.apply(bytes);
            } catch (IOException e) {
                throw Errors.toLoamsException(e, rpc);
            }
        }

        @Override
        public void close() {
            stream.close();
        }
    }
}