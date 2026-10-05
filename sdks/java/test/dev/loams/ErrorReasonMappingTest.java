package dev.loams;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;
import static org.junit.Assert.fail;

import com.google.protobuf.Descriptors;
import com.google.protobuf.Message;
import dev.loams.facade.CallBinding;
import dev.loams.facade.Facade;
import dev.loams.gen.loams.errors.v1.ErrorInfo;
import dev.loams.gen.loams.instance.v1.GetInstanceRequest;
import dev.loams.gen.loams.live.v1.MutateRequest;
import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.atomic.AtomicInteger;
import org.junit.Test;

/**
 * SDK2 Task 6's {@code java_error_reason_mapping} (design §44 §7.4, D611; runtime contract R8).
 *
 * <p>It walks <b>every</b> reason in the registry rather than a hand-picked few, because the thing
 * being pinned is that the registry and the mapping agree — and a test that listed three reasons
 * would agree with a registry that had lost twenty.
 *
 * <p>Three cases are distinct and are not conflated, and each has its own test below: a reason
 * from a <b>newer</b> server, a failure from <b>below the API</b>, and a mapped failure mapped
 * twice.
 */
public class ErrorReasonMappingTest {

    /**
     * SDK2 Task 6's {@code java_error_reason_mapping}: every reason in the registry maps to the
     * code {@code docs/api/reasons.md} gives it, and to a typed exception.
     */
    @Test
    public void java_error_reason_mapping() {
        int checked = 0;
        for (var reason : dev.loams.facade.Reason.all()) {
            String codeWire = dev.loams.facade.ReasonCodes.of(reason);
            assertNotNull(
                    "reason " + reason.wire() + " has no code in the registry", codeWire);
            Code code = Code.fromWire(codeWire);
            assertEquals(
                    "reason " + reason.wire() + " names code \"" + codeWire
                            + "\", which Code does not know",
                    codeWire,
                    code.wire());

            ErrorInfo info =
                    ErrorInfo.newBuilder()
                            .setReason(reason.wire())
                            .putMetadata("variant", "standard")
                            .setHint("a hint for " + reason.wire())
                            .build();
            ConnectErrorCase wire = wireError(codeWire, reason.wire(), info);

            LoamsException mapped = Errors.toLoamsException(wire.failure(), "loams.instance.v1.InstanceService/WhoAmI");
            assertEquals(
                    "reason " + reason.wire() + " came back as " + mapped.reason(),
                    reason,
                    mapped.reason());
            assertEquals(
                    "reason " + reason.wire() + " lost its code",
                    code,
                    mapped.code());
            assertEquals(
                    "reason " + reason.wire() + " lost its hint",
                    "a hint for " + reason.wire(),
                    mapped.hint());
            assertEquals(
                    "reason " + reason.wire() + " lost its metadata",
                    "standard",
                    mapped.metadata().get("variant"));
            assertEquals(
                    "reason " + reason.wire() + " did not land on the class its code names",
                    expectedClass(code, reason),
                    mapped.getClass());
            checked++;
        }
        // The expected count is the registry page's own count rather than a number written down
        // here, so a reason added to the page is covered the day it lands and one dropped is a
        // failure rather than a silently shorter suite.
        assertEquals(
                "the mapping loop covered " + checked + " reasons but docs/api/reasons.md lists "
                        + documentedReasons().size(),
                documentedReasons().size(),
                checked);
    }

    /**
     * The two reason-bearing subclasses are reached through their parents too.
     *
     * <p>A caller who branches on {@code UnimplementedException} must still match a
     * {@code feature_not_in_variant} refusal, and one who branches on
     * {@code UnauthenticatedException} must still match a {@code token_expired}. In Go this needs a
     * hand-written {@code As} method; in Java it is inheritance, and this is the test that says so.
     */
    @Test
    public void theSubclassesAreAlsoTheirParents() {
        LoamsException refused =
                Errors.toLoamsException(
                        wireError(
                                        "unimplemented",
                                        "feature_not_in_variant",
                                        ErrorInfo.newBuilder().putMetadata("variant", "standard").build())
                                .failure(),
                        "loams.live.v1.LiveService/Query");
        assertTrue(refused instanceof FeatureNotInVariantException);
        assertTrue(
                "a feature_not_in_variant refusal must also be an UnimplementedException",
                refused instanceof UnimplementedException);
        assertTrue(
                "and it must also be a LoamsException",
                Errors.isLoamsError(refused));

        LoamsException expired =
                Errors.toLoamsException(
                        wireError("unauthenticated", "token_expired", ErrorInfo.getDefaultInstance())
                                .failure(),
                        "loams.instance.v1.InstanceService/WhoAmI");
        assertTrue(expired instanceof TokenExpiredException);
        assertTrue(
                "a token_expired must also be an UnauthenticatedException",
                expired instanceof UnauthenticatedException);
    }

    /**
     * A reason from a <b>newer</b> server is surfaced, not dropped (R8).
     *
     * <p>Losing it would leave a caller unable to tell "not supported here" from "not supported at
     * all", which is the specific harm the clause names.
     */
    @Test
    public void anUnknownReasonIsSurfacedRatherThanDropped() {
        LoamsException mapped =
                Errors.toLoamsException(
                        wireError(
                                        "unimplemented",
                                        "a_reason_this_sdk_has_never_heard_of",
                                        ErrorInfo.newBuilder()
                                                .setReason("a_reason_this_sdk_has_never_heard_of")
                                                .setHint("a newer server")
                                                .build())
                                .failure(),
                        "loams.instance.v1.InstanceService/WhoAmI");
        assertNull(
                "a reason this registry does not have must not become a known one",
                mapped.reason());
        assertEquals(
                "the unknown reason was dropped rather than surfaced",
                "a_reason_this_sdk_has_never_heard_of",
                mapped.unknownReason());
        assertFalse(
                "a failure carrying an unknown reason is not a below-the-API failure",
                mapped.isBelowApi());
        assertEquals(
                "the hint on an unknown-reason failure was lost", "a newer server", mapped.hint());
        // The code still classifies it, so a caller who only wants the coarse class is served.
        assertEquals(Code.UNIMPLEMENTED, mapped.code());
    }

    /**
     * A failure from <b>below the API</b> carries no reason, and says so.
     *
     * <p>A socket, a refused connection, an interrupt: no service answered, so there is no reason
     * to report — and that is a different thing from a service refusing.
     */
    @Test
    public void aFailureFromBelowTheApiCarriesNoReason() {
        LoamsException mapped =
                Errors.toLoamsException(new java.net.ConnectException("connection refused"), "loams.instance.v1.InstanceService/GetInstance");
        assertNull("a socket failure has no reason", mapped.reason());
        assertEquals("", mapped.unknownReason());
        assertTrue("a socket failure is a below-the-API failure", mapped.isBelowApi());
        assertFalse(
                "and it is not retryable-by-code, which is the point of the distinction",
                Retry.isRetryableCode(mapped.code()));

        LoamsException interrupted =
                Errors.toLoamsException(new InterruptedException("cancelled"), "loams.instance.v1.InstanceService/GetInstance");
        assertEquals(
                "an interrupt must keep its own code rather than collapsing into unknown",
                Code.CANCELED,
                interrupted.code());
    }

    /**
     * A mapped failure mapped twice comes back unchanged.
     *
     * <p>Wrapping an SDK's own failure must never lose its reason, and that is exactly what
     * happens if the mapper re-maps.
     */
    @Test
    public void mappingAMappedErrorChangesNothing() {
        // The mapper reports the failure's own RPC, the way Go's does, so the second mapping is
        // handed a *different* one to prove it does not overwrite it.
        LoamsException first =
                Errors.toLoamsException(
                        wireError(
                                        "not_found",
                                        "not_found",
                                        ErrorInfo.getDefaultInstance(),
                                        "loams.instance.v1.InstanceService/GetInstance")
                                .failure(),
                        "loams.instance.v1.InstanceService/GetInstance");
        LoamsException second = Errors.toLoamsException(first, "a.different.RPC/Method");
        assertTrue("the mapper rebuilt the error", first == second);
        assertEquals(
                "re-mapping overwrote the RPC", "loams.instance.v1.InstanceService/GetInstance", second.rpc());
        assertEquals("re-mapping dropped the reason", dev.loams.facade.Reason.NOT_FOUND, second.reason());
    }

    /**
     * The registry in the SDK matches {@code docs/api/reasons.md}.
     *
     * <p>The page is the authority; the enum is a transcription of it, and this is what makes the
     * transcription verifiable rather than a promise.
     */
    @Test
    public void theReasonRegistryMatchesTheRegistryPage() {
        List<String> documented = documentedReasons();
        List<String> inSdk = new ArrayList<>();
        for (var reason : dev.loams.facade.Reason.all()) {
            inSdk.add(reason.wire());
        }
        assertEquals(
                "docs/api/reasons.md and dev.loams.facade.Reason have drifted apart",
                documented,
                inSdk);
    }

    /**
     * The reason a binding marks keyed is the one the schema declares.
     *
     * <p>R3's decision is read off the generated schema rather than off a hand-written list, and
     * this is the test that says the list and the schema agree.
     */
    @Test
    public void keyedBindingsMatchTheGeneratedSchema() {
        for (var module : Facade.MODULES) {
            for (CallBinding call : module.calls()) {
                Message message = requestMessageFor(call);
                if (message == null) {
                    continue;
                }
                assertEquals(
                        "binding "
                                + call.module()
                                + "."
                                + call.name()
                                + " says takesIdempotencyKey="
                                + call.takesIdempotencyKey()
                                + ", which the generated schema disagrees with",
                        call.takesIdempotencyKey(),
                        Idempotency.declaresIdempotencyKey(message));
            }
        }
    }

    /** A request message the binding names, or {@code null} for one this SDK has no message for. */
    private static Message requestMessageFor(CallBinding call) {
        String rpc = call.rpc();
        if (rpc.equals("loams.live.v1.LiveService/Mutate")) {
            return MutateRequest.getDefaultInstance();
        }
        if (rpc.equals("loams.live.v1.LiveService/Deploy")) {
            return dev.loams.gen.loams.live.v1.DeployRequest.getDefaultInstance();
        }
        if (rpc.equals("loams.live.v1.LiveService/Query")) {
            return dev.loams.gen.loams.live.v1.QueryRequest.getDefaultInstance();
        }
        if (rpc.equals("loams.instance.v1.InstanceService/GetInstance")) {
            return GetInstanceRequest.getDefaultInstance();
        }
        if (rpc.equals("loams.live.v1.LiveService/Watch")) {
            return dev.loams.gen.loams.live.v1.WatchRequest.getDefaultInstance();
        }
        return null;
    }

    /** Every reason in {@code docs/api/reasons.md}, in the page's order. */
    private static List<String> documentedReasons() {
        java.nio.file.Path page =
                Fixtures.repositoryRoot().resolve("docs/api/reasons.md");
        List<String> reasons = new ArrayList<>();
        try {
            for (String line : java.nio.file.Files.readAllLines(page)) {
                // The registry's rows are the table's first-column backticked tokens, one per
                // line, and each row is `| `reason` | ...`.
                String trimmed = line.trim();
                if (!trimmed.startsWith("| `")) {
                    continue;
                }
                int close = trimmed.indexOf('`', 3);
                if (close < 0) {
                    continue;
                }
                String value = trimmed.substring(3, close);
                if (value.matches("[a-z][a-z0-9_]*")) {
                    reasons.add(value);
                }
            }
        } catch (java.io.IOException e) {
            throw new AssertionError("reading " + page + ": " + e, e);
        }
        assertFalse("docs/api/reasons.md yielded no reasons; its table's shape changed", reasons.isEmpty());
        return reasons;
    }

    /**
     * A wire error as the server would send it: a Connect JSON body whose {@code ErrorInfo} detail
     * carries the reason.
     *
     * <p>Built through the real JSON rather than by constructing the exception, so the test goes
     * through the same parse the runtime does.
     */
    private static ConnectErrorCase wireError(String code, String reason, ErrorInfo info) {
        return wireError(code, reason, info, "loams.instance.v1.InstanceService/WhoAmI");
    }

    /**
     * A wire error carrying {@code reason}.
     *
     * <p>The reason is stamped onto the {@link ErrorInfo} here rather than left to each call site,
     * because a body whose {@code ErrorInfo} has no reason is a body the mapper is right to report
     * with no reason — and a test that built one by accident would fail on the wrong thing.
     */
    private static ConnectErrorCase wireError(
            String code, String reason, ErrorInfo info, String rpc) {
        ErrorInfo stamped = info.toBuilder().setReason(reason).build();
        String json =
                "{\"code\":\""
                        + code
                        + "\",\"message\":\"a message for "
                        + reason
                        + "\",\"details\":[{\"type\":\"loams.errors.v1.ErrorInfo\",\"value\":\""
                        + java.util.Base64.getEncoder()
                                .encodeToString(stamped.toByteArray())
                        + "\"}]}";
        dev.loams.connect.ConnectError parsed =
                dev.loams.connect.ConnectError.fromUnaryBody(json);
        assertNotNull("the mapper could not read its own error body: " + json, parsed);
        return new ConnectErrorCase(new dev.loams.connect.ConnectFailure(rpc, parsed, null));
    }

    /** The exception class a code maps to, so the registry and the hierarchy are pinned together. */
    private static Class<?> expectedClass(Code code, dev.loams.facade.Reason reason) {
        // Two reasons land on a subclass of their code's class, because the clause is about the
        // reason rather than the code: an unavailable feature and an expired token are both
        // catchable by their code, and both are more specific than their code.
        if (reason == dev.loams.facade.Reason.FEATURE_NOT_IN_VARIANT) {
            return FeatureNotInVariantException.class;
        }
        if (reason == dev.loams.facade.Reason.TOKEN_EXPIRED) {
            return TokenExpiredException.class;
        }
        return expectedClass(code);
    }

    private static Class<?> expectedClass(Code code) {
        return switch (code) {
            case INVALID_ARGUMENT -> InvalidArgumentException.class;
            case NOT_FOUND -> NotFoundException.class;
            case ALREADY_EXISTS -> AlreadyExistsException.class;
            case PERMISSION_DENIED -> PermissionDeniedException.class;
            case UNAUTHENTICATED -> UnauthenticatedException.class;
            case FAILED_PRECONDITION -> FailedPreconditionException.class;
            case RESOURCE_EXHAUSTED -> ResourceExhaustedException.class;
            case UNAVAILABLE -> UnavailableException.class;
            case DEADLINE_EXCEEDED -> DeadlineExceededException.class;
            case ABORTED -> AbortedException.class;
            case INTERNAL -> InternalException.class;
            case UNIMPLEMENTED -> UnimplementedException.class;
            default -> LoamsException.class;
        };
    }

    /** A counted wire failure, so a test can assert on what it built. */
    private record ConnectErrorCase(dev.loams.connect.ConnectFailure failure) {}

    /** A counter, for a test that needs one. */
    static final class Counter {

        private final AtomicInteger count = new AtomicInteger();

        int increment() {
            return count.incrementAndGet();
        }

        int get() {
            return count.get();
        }
    }

    /** The schema field {@code R3} keys on, for a test that reads it directly. */
    static Descriptors.FieldDescriptor keyField(Message message) {
        return message.getDescriptorForType()
                .findFieldByName(Idempotency.IDEMPOTENCY_KEY_FIELD);
    }

    /** Guard against a test silently asserting nothing. */
    static void requireTrue(boolean condition, String message) {
        if (!condition) {
            fail(message);
        }
    }
}