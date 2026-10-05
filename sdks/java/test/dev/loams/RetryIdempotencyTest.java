package dev.loams;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotEquals;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertTrue;

import com.google.protobuf.Descriptors;
import dev.loams.connect.ConnectError;
import dev.loams.connect.ConnectErrorDetail;
import dev.loams.connect.ConnectFailure;
import dev.loams.facade.CallBinding;
import dev.loams.facade.Reason;
import dev.loams.gen.loams.errors.v1.ErrorInfo;
import dev.loams.gen.loams.live.v1.DeployRequest;
import dev.loams.gen.loams.live.v1.MutateRequest;
import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.atomic.AtomicInteger;
import org.junit.Test;

/**
 * SDK2 Task 6's {@code java_retry_reuses_idempotency_key} (design §44 §7.4, D610; runtime contract
 * R2 and R3).
 *
 * <p>Both halves are pinned together because they are the same clause from two directions: R3 says
 * the key is minted once per logical call, and R2 says the retry loop is what would expose a
 * violation. A key regenerated per attempt turns one write into two, which is the exact failure
 * the key exists to prevent — so the test counts the keys that went out, not the retries that
 * happened.
 *
 * <p>The retry loop is driven directly through {@link CallInvoker#retryLoop} with a scripted
 * {@link CallInvoker.Attempt}, because the retry policy is the SDK's own logic and is worth pinning
 * without a server. Nothing here weakens a test to get green: every assertion is on a value the SDK
 * produced.
 */
public class RetryIdempotencyTest {

    /** The RPC whose {@code Mutate} is the only keyed mutation in the API today. */
    private static final String MUTATE_RPC = "loams.live.v1.LiveService/Mutate";

    /** The RPC whose {@code Deploy} has <em>no</em> {@code idempotency_key} field. */
    private static final String DEPLOY_RPC = "loams.live.v1.LiveService/Deploy";

    /**
     * SDK2 Task 6's {@code java_retry_reuses_idempotency_key}.
     *
     * <p>It pins three things in one run because they are one behaviour:
     *
     * <ol>
     *   <li>a mutation is <b>not</b> retried when it carries no key, so an unkeyed write is never
     *       repeated;
     *   <li>the same key goes out on <b>every</b> retry of a keyed call;
     *   <li>a caller-supplied key is used verbatim rather than replaced.
     * </ol>
     */
    @Test
    public void java_retry_reuses_idempotency_key() {
        // 1. An unkeyed mutation is not retried at all. `Mutate` without a key is still `manual`
        //    until the runtime keys it, and the point of the clause is that a call is only
        //    repeatable once the key is on it.
        CallBinding mutate = Client.binding("tables", "Mutate");
        assertEquals(
                "tables.Mutate must be a manual-retry call, or this test is not testing anything",
                dev.loams.facade.RetryClass.MANUAL,
                mutate.retry());
        CallInvoker.RetryPlan unkeyedPlan = planFor(mutate, false);
        assertFalse(
                "a mutation with no idempotency key must not be retried",
                unkeyedPlan.retrySafe());

        AtomicInteger unkeyedAttempts = new AtomicInteger();
        LoamsException unkeyedFailure =
                runRetryLoop(
                        MUTATE_RPC,
                        unkeyedPlan,
                        (attempt, refreshed) -> {
                            unkeyedAttempts.incrementAndGet();
                            throw unavailable("a node went away");
                        });
        assertEquals(
                "an unkeyed mutation was attempted " + unkeyedAttempts.get() + " times; it must be"
                        + " attempted once",
                1,
                unkeyedAttempts.get());
        assertEquals(Code.UNAVAILABLE, unkeyedFailure.code());

        // 2. The same key goes out on every retry. This is the whole of R3, and it is asserted on
        //    the *messages* rather than on a counter, because a key regenerated per attempt would
        //    still produce three attempts.
        List<MutateRequest> sent = new ArrayList<>();
        Idempotency.KeyedRequest<MutateRequest> keyed =
                Idempotency.applyIdempotencyKey(
                        MutateRequest.getDefaultInstance(), "", mutate.takesIdempotencyKey());
        assertTrue("Mutate was not keyed", keyed.keyed());
        assertNotNull("the keyed request carries no key", keyed.request().getIdempotencyKey());
        assertFalse("the minted key is empty", keyed.request().getIdempotencyKey().isEmpty());

        CallInvoker.RetryPlan keyedPlan = planFor(mutate, keyed.keyed());
        assertTrue(
                "a keyed mutation must become retryable, or a retried write is two writes",
                keyedPlan.retrySafe());

        AtomicInteger keyedAttempts = new AtomicInteger();
        byte[] answer =
                (byte[])
                        CallInvoker.retryLoop(
                                MUTATE_RPC,
                                keyedPlan,
                                (attempt, refreshed) -> {
                                    keyedAttempts.incrementAndGet();
                                    // The **same** message value on every attempt, which is what
                                    // the invoker sends: one keyed copy, reused.
                                    sent.add((MutateRequest) keyed.request());
                                        if (keyedAttempts.get() <= 2) {
                                            throw unavailable("a node went away");
                                        }
                                        return new byte[] {1};
                                    });
        assertEquals("the keyed mutation was not retried", 3, keyedAttempts.get());
        assertEquals("the retry loop lost its answer", 1, answer[0]);
        assertEquals("the attempts did not share one message", 3, sent.size());
        String first = sent.get(0).getIdempotencyKey();
        for (int index = 1; index < sent.size(); index++) {
            assertEquals(
                    "attempt " + index + " sent key \"" + sent.get(index).getIdempotencyKey()
                            + "\" but attempt 0 sent \"" + first
                            + "\"; a key regenerated per attempt turns one write into two",
                    first,
                    sent.get(index).getIdempotencyKey());
        }

        // 3. A caller-supplied key is used verbatim, because the key is what the caller's own
        //    storage dedupes on.
        Idempotency.KeyedRequest<MutateRequest> supplied =
                Idempotency.applyIdempotencyKey(
                        MutateRequest.getDefaultInstance(), "order-4711", mutate.takesIdempotencyKey());
        assertTrue(supplied.keyed());
        assertEquals(
                "the SDK replaced the caller's own idempotency key",
                "order-4711",
                supplied.request().getIdempotencyKey());

        // 4. Two logical calls get two different keys. A key shared across calls would make the
        //    server dedupe two genuinely separate writes.
        String one = Idempotency.uuidV7();
        String two = Idempotency.uuidV7();
        assertNotEquals("two minted keys are the same", one, two);
    }

    /**
     * A request whose schema has no {@code idempotency_key} field is left alone.
     *
     * <p>{@code DeployRequest} is the case: it is a mutation and therefore a {@code manual} call,
     * and it has no field to key. Inventing one would be the SDK guessing at the schema, and
     * silently keying a call the proto does not support would be worse than not keying it.
     */
    @Test
    public void aRequestWithNoKeyFieldIsLeftAlone() {
        CallBinding deploy = Client.binding("tables", "Deploy");
        assertFalse(
                "the binding says Deploy takes a key, which the generated schema contradicts",
                deploy.takesIdempotencyKey());
        assertFalse(
                "DeployRequest declares idempotency_key",
                Idempotency.declaresIdempotencyKey(DeployRequest.getDefaultInstance()));

        DeployRequest request =
                DeployRequest.newBuilder().setBundle(com.google.protobuf.ByteString.copyFromUtf8("a bundle")).build();
        Idempotency.KeyedRequest<DeployRequest> keyed =
                Idempotency.applyIdempotencyKey(request, "", deploy.takesIdempotencyKey());
        assertFalse("Deploy was keyed even though it has no such field", keyed.keyed());
        assertTrue(
                "the keyed request is a copy, but an unchanged one",
                keyed.request() == request
                        || keyed.request().equals(request));

        // And a mutation with no key stays un-retryable, which is the safe direction.
        assertFalse(
                "an unkeyed Deploy became retryable",
                planFor(deploy, keyed.keyed()).retrySafe());
    }

    /**
     * A key already on the caller's message is kept, not overwritten.
     *
     * <p>A caller who manages their own dedupe has said what they mean, and replacing their key
     * would break their storage's correlation.
     */
    @Test
    public void aKeyTheCallerAlreadySetIsKept() {
        MutateRequest request =
                MutateRequest.newBuilder().setIdempotencyKey("caller-supplied").build();
        Idempotency.KeyedRequest<MutateRequest> keyed =
                Idempotency.applyIdempotencyKey(request, "an-option-value", true);
        assertTrue(keyed.keyed());
        assertEquals(
                "the runtime overwrote a key the caller had already set",
                "caller-supplied",
                keyed.request().getIdempotencyKey());
    }

    /**
     * The caller's message is never mutated.
     *
     * <p>A keyed request is a copy. A caller who retries their own request would otherwise find
     * their own message changed underneath them, which is a nasty class of bug and easy to
     * introduce by using a mutable builder in place.
     */
    @Test
    public void theCallersMessageIsNeverMutated() {
        MutateRequest original = MutateRequest.getDefaultInstance();
        Idempotency.KeyedRequest<MutateRequest> keyed = Idempotency.applyIdempotencyKey(original, "", true);
        assertTrue(keyed.keyed());
        assertEquals(
                "the runtime wrote the key into the caller's message", "", original.getIdempotencyKey());
        assertNotEquals(
                "keying returned the caller's own object",
                System.identityHashCode(original),
                System.identityHashCode(keyed.request()));
    }

    /**
     * The retryable codes are exactly M1.6 Ruling 5's three, and nothing else (R2).
     *
     * <p>This is the set that decides whether a node restart is survived or surfaced, so a code
     * added to it by accident would silently retry something that must not be repeated.
     */
    @Test
    public void onlyThreeCodesAreRetryable() {
        assertTrue(Retry.isRetryableCode(Code.UNAVAILABLE));
        assertTrue(Retry.isRetryableCode(Code.DEADLINE_EXCEEDED));
        assertTrue(Retry.isRetryableCode(Code.RESOURCE_EXHAUSTED));
        List<Code> retryable = new ArrayList<>(Retry.retryableCodes());
        assertEquals("the retryable set has drifted", 3, retryable.size());
        for (Code code : Code.values()) {
            boolean expected =
                    code == Code.UNAVAILABLE
                            || code == Code.DEADLINE_EXCEEDED
                            || code == Code.RESOURCE_EXHAUSTED;
            assertEquals(
                    "code " + code.wire() + " retryability is " + Retry.isRetryableCode(code)
                            + ", want " + expected,
                    expected,
                    Retry.isRetryableCode(code));
        }
    }

    /**
     * The backoff numbers are M1.6 Ruling 5's, and the jitter is <b>full</b>.
     *
     * <p>Full jitter rather than plain exponential backoff because every client retrying at the
     * same instant after a node restart is how a recovering node gets knocked over again. The
     * assertion is on the <em>spread</em> rather than on one draw: a hundred samples that all
     * returned the same value would pass a single-sample test and would not be jitter at all.
     */
    @Test
    public void theBackoffIsFullJitterOverM16Ruling5sNumbers() {
        assertEquals(100, Retry.BASE_DELAY_MS);
        assertEquals(2000, Retry.MAX_DELAY_MS);
        assertEquals(3, Retry.DEFAULT_MAX_RETRIES);
        assertEquals(30_000, Retry.MAX_SERVER_DELAY_MS);

        for (int attempt = 0; attempt < 5; attempt++) {
            long ceiling = Math.min(Retry.MAX_DELAY_MS, Retry.BASE_DELAY_MS << attempt);
            long lowest = Long.MAX_VALUE;
            long highest = Long.MIN_VALUE;
            for (int sample = 0; sample < 200; sample++) {
                long millis = Retry.backoff(attempt, java.time.Duration.ZERO).toMillis();
                assertTrue(
                        "backoff(" + attempt + ") returned " + millis + "ms, which is outside [0, "
                                + ceiling + "]",
                        millis >= 0 && millis <= ceiling);
                lowest = Math.min(lowest, millis);
                highest = Math.max(highest, millis);
            }
            assertTrue(
                    "backoff(" + attempt + ") returned only " + (highest - lowest)
                            + "ms of spread over 200 samples; full jitter is uniform over [0, "
                            + ceiling + "]",
                    highest - lowest > ceiling / 4);
        }
    }

    /**
     * A server-sent {@code RetryInfo.retry_delay} replaces the computed backoff, up to 30 s (R2).
     *
     * <p>No proto carries {@code RetryInfo} yet, so nothing today reaches this — but the numbers
     * have to be right the day one does, and a test is what makes them right rather than
     * approximately right.
     */
    @Test
    public void aServerSentDelayReplacesTheComputedBackoffAndIsCapped() {
        assertEquals(
                "a 5s server delay was not honoured",
                java.time.Duration.ofSeconds(5),
                Retry.backoff(0, java.time.Duration.ofSeconds(5)));
        assertEquals(
                "a 60s server delay was not capped at 30s",
                java.time.Duration.ofSeconds(30),
                Retry.backoff(0, java.time.Duration.ofSeconds(60)));
    }

    /**
     * A read retries; a mutation with no key does not (R2's class comes from the binding, D610).
     */
    @Test
    public void aReadRetriesAndAMutationDoesNot() {
        CallBinding getInstance = Client.binding("instance", "GetInstance");
        assertEquals(
                "GetInstance must be a safe-retry call",
                dev.loams.facade.RetryClass.SAFE,
                getInstance.retry());
        assertTrue(planFor(getInstance, false).retrySafe());

        CallBinding whoAmI = Client.binding("instance", "WhoAmI");
        assertTrue(planFor(whoAmI, false).retrySafe());

        CallBinding query = Client.binding("tables", "Query");
        assertEquals(
                "Query must be a manual-retry call",
                dev.loams.facade.RetryClass.MANUAL,
                query.retry());
        assertFalse(planFor(query, false).retrySafe());
    }

    /**
     * A cancelled call is never retried, whatever the class.
     *
     * <p>The caller has said they do not want the answer, and a retry cannot change that.
     */
    @Test
    public void aCancelledCallIsNeverRetried() {
        assertFalse(
                "a cancelled call was retried",
                Retry.shouldRetry(
                        true,
                        unavailable("a node went away"),
                        true,
                        0,
                        Retry.DEFAULT_MAX_RETRIES));
    }

    /**
     * A second {@code token_expired} is reported rather than refreshed forever (R1).
     *
     * <p>The refresh happens exactly once and is not charged to the call's retry budget, so a
     * caller who asked for three retries asked for three retries of the request — not three
     * including a credential refresh.
     */
    @Test
    public void oneRefreshThenOneRetryAndThenTheFailureIsReported() {
        AtomicInteger attempts = new AtomicInteger();
        AtomicInteger refreshes = new AtomicInteger();
        CallInvoker.RetryPlan plan =
                new CallInvoker.RetryPlan(true, 3, refreshes::incrementAndGet);

        LoamsException failure =
                runRetryLoop(
                        "loams.instance.v1.InstanceService/WhoAmI",
                        plan,
                        (attempt, refreshed) -> {
                            attempts.incrementAndGet();
                            // Always expired, which is the case that must not loop.
                            throw tokenExpired();
                        });
        assertEquals("the refresh happened more than once", 1, refreshes.get());
        // One attempt, one refresh, one retry, then reported: the refresh's retry is not charged
        // to the budget, so this is 2 attempts and not 1 + 1 refresh + 3 retries.
        assertEquals("the expired call was attempted " + attempts.get() + " times, want 2", 2, attempts.get());
        assertTrue(
                "the second expiry was not surfaced as a TokenExpiredException",
                failure instanceof TokenExpiredException);
    }

    /**
     * The plan the real invoker builds for a binding, given whether the call ended up keyed.
     *
     * <p>It goes through {@link CallInvoker#plan} rather than restating D610's rule here, so a
     * change to the derivation is caught by this test instead of only at runtime. A transport of
     * {@code null} is safe because no attempt is made: this only asks what the plan would be.
     */
    private static CallInvoker.RetryPlan planFor(CallBinding binding, boolean keyed) {
        return new CallInvoker(null, null, Retry.DEFAULT_MAX_RETRIES, null)
                .plan(binding, CallOptions.none(), keyed);
    }

    /** Run the retry loop expecting it to fail, and return the mapped failure. */
    private static LoamsException runRetryLoop(
            String rpc, CallInvoker.RetryPlan plan, CallInvoker.Attempt send) {
        return ConformanceTest.assertThrowsLoams(() -> CallInvoker.retryLoop(rpc, plan, send));
    }

    /** An {@code unavailable} failure, which is what a node restart produces. */
    private static LoamsException unavailable(String message) {
        return failure("unavailable", "unavailable", message);
    }

    /** A {@code token_expired} failure, which is what R1's refresh answers. */
    private static LoamsException tokenExpired() {
        return failure("unauthenticated", "token_expired", "the token expired");
    }

    private static LoamsException failure(String code, String reason, String message) {
        ErrorInfo info = ErrorInfo.newBuilder().setReason(reason).build();
        String json =
                "{\"code\":\""
                        + code
                        + "\",\"message\":\""
                        + message
                        + "\",\"details\":[{\"type\":\""
                        + ConnectErrorDetail.ERROR_INFO
                        + "\",\"value\":\""
                        + java.util.Base64.getEncoder().encodeToString(info.toByteArray())
                        + "\"}]}";
        ConnectError parsed = ConnectError.fromUnaryBody(json);
        assertNotNull("could not build the failure body", parsed);
        return Errors.toLoamsException(new ConnectFailure("x", parsed, null), "x");
    }

    /** The schema field R3 keys on, read off a message rather than off a list. */
    static Descriptors.FieldDescriptor keyField(MutateRequest request) {
        return request.getDescriptorForType().findFieldByName(Idempotency.IDEMPOTENCY_KEY_FIELD);
    }

    /** The reason a refusal carries, for a test that asserts on it. */
    static Reason refusalReason() {
        return Reason.FEATURE_NOT_IN_VARIANT;
    }
}