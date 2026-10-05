package dev.loams;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;
import static org.junit.Assert.fail;

import com.google.protobuf.Message;
import dev.loams.gen.loams.instance.v1.GetInstanceRequest;
import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicInteger;
import org.junit.Test;

/**
 * SDK2 Task 6's {@code java_token_source_refresh} (design §44 §7.4, D608; runtime contract R1).
 *
 * <p>R1 has two halves and this covers both:
 *
 * <ul>
 *   <li>the token travels in {@code Authorization: Bearer}, <b>never</b> in a URL — a query string
 *       ends up in proxy logs, in browser history and in {@code Referer};
 *   <li>a {@code 401} carrying {@code reason = token_expired} triggers <b>exactly one</b> refresh
 *       and <b>one</b> retry; a second expiry is reported, and a source that cannot refresh makes
 *       the refresh a no-op.
 * </ul>
 *
 * <p>The bearer placement is asserted against a real server, because "does not put the token in
 * the URL" is only checkable by looking at what actually went out on the wire. The refresh
 * behaviour is driven through the retry loop with a scripted attempt, because it is the SDK's own
 * logic.
 */
public class TokenSourceTest {

    /**
     * SDK2 Task 6's {@code java_token_source_refresh}.
     *
     * <p>Pinned here: one refresh and one retry on the first expiry, a reported failure on the
     * second, and a no-op refresh for a source that has nothing to refresh.
     */
    @Test
    public void java_token_source_refresh() {
        // 1. One refresh, one retry, then success. The retry is not charged to the call's budget:
        //    it is the same logical call, so a caller who asked for three retries asked for three
        //    retries of the request, not three including a credential refresh.
        AtomicInteger attempts = new AtomicInteger();
        AtomicInteger refreshes = new AtomicInteger();
        CallInvoker.RetryPlan plan =
                new CallInvoker.RetryPlan(true, 3, refreshes::incrementAndGet);

        Object answer =
                CallInvoker.retryLoop(
                        "loams.instance.v1.InstanceService/GetInstance",
                        plan,
                        (attempt, refreshed) -> {
                            attempts.incrementAndGet();
                            if (attempts.get() == 1) {
                                throw expiredFailure();
                            }
                            // The second attempt must be carrying the refreshed token, which is
                            // the whole point of fetching the bearer per attempt.
                            if (!refreshed) {
                                fail("the retry went out before the refresh happened");
                            }
                            return new byte[] {7};
                        });
        assertEquals("the refresh happened " + refreshes.get() + " times, want exactly 1", 1, refreshes.get());
        assertEquals("the call was attempted " + attempts.get() + " times, want 2", 2, attempts.get());
        assertEquals("the retry loop lost its answer", 7, ((byte[]) answer)[0]);

        // 2. A second expiry is reported rather than refreshed forever. A refresh loop over a
        //    token that keeps being rejected would spin.
        AtomicInteger secondAttempts = new AtomicInteger();
        AtomicInteger secondRefreshes = new AtomicInteger();
        LoamsException reported =
                ConformanceTest.assertThrowsLoams(
                        () ->
                                CallInvoker.retryLoop(
                                        "loams.instance.v1.InstanceService/WhoAmI",
                                        new CallInvoker.RetryPlan(
                                                true, 3, secondRefreshes::incrementAndGet),
                                        (attempt, refreshed) -> {
                                            secondAttempts.incrementAndGet();
                                            throw expiredFailure();
                                        }));
        assertEquals("the second expiry was refreshed again", 1, secondRefreshes.get());
        assertEquals(
                "a token that keeps expiring was attempted "
                        + secondAttempts.get()
                        + " times; the clause is one refresh and one retry",
                2,
                secondAttempts.get());
        assertTrue(
                "the reported failure is " + reported.getClass().getSimpleName()
                        + ", want a TokenExpiredException",
                reported instanceof TokenExpiredException);
        assertEquals(
                "the reported failure lost its reason", dev.loams.facade.Reason.TOKEN_EXPIRED, reported.reason());

        // 3. A source that cannot refresh makes R1's refresh a no-op, and the expiry is reported
        //    on the first attempt rather than retried. An API key does not expire, so retrying
        //    with it would only fail again.
        TokenSource apiKey = TokenSources.apiKey("sk-not-expiring");
        apiKey.refresh();
        assertEquals("an API key's refresh changed its token", "sk-not-expiring", apiKey.token());
        CallInvoker.RetryPlan noRefresh = new CallInvoker.RetryPlan(true, 3, null);
        AtomicInteger keyAttempts = new AtomicInteger();
        LoamsException keyFailure =
                ConformanceTest.assertThrowsLoams(
                        () ->
                                CallInvoker.retryLoop(
                                        "loams.instance.v1.InstanceService/WhoAmI",
                                        noRefresh,
                                        (attempt, refreshed) -> {
                                            keyAttempts.incrementAndGet();
                                            throw expiredFailure();
                                        }));
        assertEquals(
                "a client with no refresh retried the expired call anyway",
                1,
                keyAttempts.get());
        assertTrue(keyFailure instanceof TokenExpiredException);

        // 4. Every shipped source that cannot refresh says so by returning normally, which is the
        //    contract's no-op rather than a missing capability.
        TokenSources.staticToken("t").refresh();
        TokenSources.env(name -> "").refresh();
    }

    /**
     * The bearer travels in {@code Authorization: Bearer}, and not in the URL (R1).
     *
     * <p>This one needs a real endpoint, because "the token is not in the query string" is only
     * checkable by looking at what went out. It reads the header off the replay server's own
     * answer: the endpoint reports the {@code Authorization} it saw.
     */
    @Test
    public void theBearerTravelsInTheHeaderAndNotInTheUrl() {
        try (HeaderEcho echo = HeaderEcho.start()) {
            try (Client client =
                    Client.of(
                            Options.builder()
                                    .endpoint(echo.endpoint())
                                    .auth(TokenSources.apiKey("sk-test-123"))
                                    .build())) {
                // The call is the point: it is the only way a header gets sent.
                client.instance().getInstance(GetInstanceRequest.getDefaultInstance());
            }
            HeaderEcho.Seen seen = echo.seen();
            assertNotNull("nothing reached the echo endpoint", seen);
            assertEquals(
                    "the bearer is not in Authorization: Bearer as R1 requires",
                    "Bearer sk-test-123",
                    seen.authorization());
            assertNull(
                    "the SDK sent its own Authorization header, which the runtime sets and wins",
                    seen.duplicateAuthorization());
            assertFalse(
                    "the token leaked into the URL: " + seen.uri(),
                    seen.uri().contains("sk-test-123"));
        }
    }

    /**
     * One in-flight refresh is shared, so a burst of {@code 401}s costs one exchange (D608).
     *
     * <p>That is not a micro-optimisation: an instance rejecting every token because it is stale
     * would otherwise be hit with one exchange per in-flight call, which is how a credential
     * rotation turns into a self-inflicted denial of service.
     */
    @Test
    public void concurrentRefreshesCostOneExchange() throws Exception {
        // The exchange is made slow on purpose. Whether overlapping callers share one exchange
        // depends on them actually overlapping, and a fetch that returns instantly lets most of
        // the burst arrive after the first one finished — which is correct behaviour (the next
        // 401 should exchange again) but proves nothing about sharing. Holding the exchange open
        // for the length of the burst is what makes the assertion about the sharing rather than
        // about the scheduler.
        CountDownLatch exchangeEntered = new CountDownLatch(1);
        CountDownLatch releaseExchange = new CountDownLatch(1);
        AtomicInteger exchanges = new AtomicInteger();
        RefreshingTokenSource source =
                new RefreshingTokenSource(
                        () -> {
                            int which = exchanges.incrementAndGet();
                            exchangeEntered.countDown();
                            try {
                                releaseExchange.await(10, TimeUnit.SECONDS);
                            } catch (InterruptedException e) {
                                Thread.currentThread().interrupt();
                            }
                            return "token-" + which;
                        });

        int readers = 32;
        ExecutorService pool = Executors.newFixedThreadPool(readers);
        CountDownLatch allStarted = new CountDownLatch(readers);
        CountDownLatch done = new CountDownLatch(readers);
        try {
            for (int i = 0; i < readers; i++) {
                pool.execute(
                        () -> {
                            // Every thread is inside `refresh()` before any of them can finish,
                            // so they all contend for the same in-flight exchange rather than
                            // each starting its own.
                            allStarted.countDown();
                            try {
                                allStarted.await(10, TimeUnit.SECONDS);
                                source.refresh();
                            } catch (InterruptedException e) {
                                Thread.currentThread().interrupt();
                            } finally {
                                done.countDown();
                            }
                        });
            }
            assertTrue(
                    "the first exchange never started",
                    exchangeEntered.await(10, TimeUnit.SECONDS));
            // Give the rest of the burst time to pile up behind the exchange in flight.
            Thread.sleep(250);
            releaseExchange.countDown();
            assertTrue("the readers did not finish within 30s", done.await(30, TimeUnit.SECONDS));
        } finally {
            releaseExchange.countDown();
            pool.shutdownNow();
        }
        assertEquals(
                "a burst of 32 concurrent refreshes cost " + exchanges.get()
                        + " exchanges, want 1: every caller that arrived while an exchange was in"
                        + " flight must wait for it rather than start a second",
                1,
                exchanges.get());
        assertEquals("the exchange's token was not cached", "token-1", source.cached());
        assertEquals("the source did not return its cached token", "token-1", source.token());
    }

    /**
     * A refresh that fails is reported to every waiter rather than left blocked.
     *
     * <p>A refresh that silently did nothing would leave the call retrying with the stale token it
     * was just told to discard, which is the worst of the three outcomes.
     */
    @Test
    public void aFailedRefreshIsReportedRatherThanSwallowed() {
        RefreshingTokenSource source =
                new RefreshingTokenSource(
                        () -> {
                            throw Errors.internal("", "the token endpoint answered 503", null);
                        });
        try {
            source.refresh();
            fail("a refresh that threw was reported as success");
        } catch (LoamsException expected) {
            assertTrue(
                    "the failure lost its reason",
                    expected.getMessage().contains("the token endpoint answered 503"));
        }
    }

    /**
     * A source whose cache starts empty fetches before answering.
     *
     * <p>Otherwise it would send no credential at all, and an instance that requires one answers
     * {@code unauthenticated} — which the call path treats as "the token expired" and retries,
     * with still no credential.
     */
    @Test
    public void aRefreshingSourceFetchesBeforeItsFirstToken() {
        AtomicInteger fetches = new AtomicInteger();
        RefreshingTokenSource source =
                new RefreshingTokenSource(
                        () -> {
                            fetches.incrementAndGet();
                            return "first";
                        });
        assertEquals("first", source.token());
        assertEquals(1, fetches.get());
        // And it does not fetch again while the cache is warm.
        assertEquals("first", source.token());
        assertEquals("the source refetched on a second token() call", 1, fetches.get());
    }

    /** The environment source reads {@code LOAMS_API_KEY} then {@code LOAMS_TOKEN}, in that order. */
    @Test
    public void theEnvironmentSourceReadsItsVariablesInOrder() {
        assertEquals(
                "the environment variables are not the two R1 names, in order",
                List.of("LOAMS_API_KEY", "LOAMS_TOKEN"),
                List.of(TokenSources.envNames()));
        TokenSource both =
                TokenSources.env(
                        name -> {
                            if (name.equals("LOAMS_API_KEY")) {
                                return "from-key";
                            }
                            return "from-token";
                        });
        assertEquals("LOAMS_API_KEY must win over LOAMS_TOKEN", "from-key", both.token());
        TokenSource onlyToken = TokenSources.env(name -> name.equals("LOAMS_TOKEN") ? "t" : "");
        assertEquals("t", onlyToken.token());
        assertEquals(
                "an unset environment yields no credential",
                "",
                TokenSources.env(name -> "").token());
    }

    /** A source's {@code toString} must not leak the token: it ends up in logs. */
    @Test
    public void aTokenSourceDoesNotLeakItsTokenInToString() {
        assertFalse(
                "apiKey leaked its key",
                TokenSources.apiKey("sk-secret-value").toString().contains("sk-secret-value"));
        assertFalse(
                "staticToken leaked its token",
                TokenSources.staticToken("secret-token-value").toString().contains("secret-token-value"));
    }

    /** A {@code 401 token_expired} as the server sends it. */
    private static LoamsException expiredFailure() {
        dev.loams.gen.loams.errors.v1.ErrorInfo info =
                dev.loams.gen.loams.errors.v1.ErrorInfo.newBuilder()
                        .setReason(dev.loams.facade.Reason.TOKEN_EXPIRED.wire())
                        .build();
        String json =
                "{\"code\":\"unauthenticated\",\"message\":\"the token expired\",\"details\":[{"
                        + "\"type\":\"loams.errors.v1.ErrorInfo\",\"value\":\""
                        + java.util.Base64.getEncoder().encodeToString(info.toByteArray())
                        + "\"}]}";
        dev.loams.connect.ConnectError parsed =
                dev.loams.connect.ConnectError.fromUnaryBody(json);
        return Errors.toLoamsException(
                new dev.loams.connect.ConnectFailure("x", parsed, null), "x");
    }

    /** A message type, for a test that needs one to exist. */
    static Message empty() {
        return GetInstanceRequest.getDefaultInstance();
    }
}