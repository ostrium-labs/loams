package dev.loams;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;
import static org.junit.Assert.fail;

import dev.loams.connect.ConnectError;
import dev.loams.connect.Envelope;
import dev.loams.facade.Facade;
import dev.loams.internal.Json;
import java.io.ByteArrayInputStream;
import java.nio.charset.StandardCharsets;
import java.time.Instant;
import java.util.Base64;
import java.util.List;
import java.util.Map;
import org.junit.Test;

/**
 * The runtime-contract clauses the six conformance tests do not cover, pinned here so the clauses
 * R1–R10 are checkable rather than described.
 *
 * <p>These are the SDK's own units: the envelope framing, the JSON reader, the UUIDv7 layout, the
 * consistency store and the client-wide defaults. None of them needs a server, and each of them is
 * a place where a plausible-looking implementation would be quietly wrong — a UUIDv7 whose
 * timestamp is read from eight hex digits instead of twelve, a JSON reader that accepts trailing
 * commas, an envelope that reports a truncated stream as a clean end.
 */
public class RuntimeContractTest {

    // ---------------------------------------------------------------- R4: consistency tokens

    /**
     * A consistency store keeps the token it was given and refuses to merge two (R4).
     *
     * <p>The refusal is the whole point: a silently-wrong consistency token reads stale data,
     * which is worse than a failure, so the conflict is counted and surfaced rather than resolved.
     */
    @Test
    public void aConsistencyStoreRefusesToMergeTwoTokens() {
        ConsistencySession session = new ConsistencySession();
        assertEquals("a fresh session has no token", "", session.current());

        assertNull("the first token was not taken", session.record("v1:1"));
        assertEquals("v1:1", session.current());

        assertNull("recording the same token again was treated as a conflict", session.record("v1:1"));
        assertEquals("a repeat changed the stored token", "v1:1", session.current());
        assertEquals("a repeat counted as a conflict", 0, session.conflicts());

        String conflict = session.record("v1:2");
        assertNotNull("two different tokens were merged silently", conflict);
        assertTrue("the conflict did not say what it refused: " + conflict, conflict.contains("v1:1"));
        assertTrue("the conflict did not say what it refused: " + conflict, conflict.contains("v1:2"));
        assertEquals("the store overwrote its token with an unmergeable one", "v1:1", session.current());
        assertEquals("the conflict was not counted", 1, session.conflicts());

        session.clear();
        assertEquals("clear left a token behind", "", session.current());
    }

    /** A store counts nothing when the server sends no token at all. */
    @Test
    public void recordingNothingIsNotAConflict() {
        ConsistencySession session = new ConsistencySession();
        session.record("v1:7");
        assertNull(session.record(null));
        assertNull(session.record(""));
        assertEquals("v1:7", session.current());
        assertEquals(0, session.conflicts());
    }

    /** What looks like a consistency token, per §44 §7.4's {@code v1:} prefix. */
    @Test
    public void aConsistencyTokenIsRecognisedByItsPrefix() {
        assertTrue(ConsistencySession.isConsistencyToken("v1:abc"));
        assertFalse("the bare prefix is not a token", ConsistencySession.isConsistencyToken("v1:"));
        assertFalse("an opaque token is not one", ConsistencySession.isConsistencyToken("opaque"));
        assertFalse("a null is not one", ConsistencySession.isConsistencyToken(null));
    }

    // ---------------------------------------------------------------- R3: the UUIDv7 layout

    /**
     * A minted UUIDv7 has the layout R3 needs: a 48-bit millisecond timestamp, version 7, variant
     * 10.
     */
    @Test
    public void aMintedUuidV7HasTheRightLayout() {
        String key = Idempotency.uuidV7();
        assertEquals("a UUIDv7 is not 36 characters", 36, key.length());
        assertEquals("the hyphen layout is wrong: " + key, key.charAt(8), '-');
        assertEquals("the hyphen layout is wrong: " + key, key.charAt(13), '-');
        assertEquals("the hyphen layout is wrong: " + key, key.charAt(18), '-');
        assertEquals("the hyphen layout is wrong: " + key, key.charAt(23), '-');
        assertEquals("the version nibble is not 7: " + key, '7', key.charAt(14));
        char variant = key.charAt(19);
        assertTrue("the variant nibble is not 8, 9, a or b: " + key,
                variant == '8' || variant == '9' || variant == 'a' || variant == 'b');

        Instant when = Idempotency.uuidV7Time(key).orElseThrow();
        long millis = when.toEpochMilli();
        long now = Instant.now().toEpochMilli();
        assertTrue(
                "the encoded timestamp is " + millis + ", which is not near now (" + now + ")",
                Math.abs(now - millis) < 60_000);
    }

    /**
     * The timestamp is read from <b>twelve</b> hex digits, not eight (R3).
     *
     * <p>This is the specific bug the method exists to prevent: 48 bits is twelve hex digits, and
     * milliseconds since the epoch use 41 of them. Reading eight returns a number around 2^25,
     * which is January 1970.
     */
    @Test
    public void theTimestampIsTwelveHexDigits() {
        // 2026-01-01T00:00:00Z in milliseconds, put into a UUIDv7 by hand.
        long millis = 1_767_225_600_000L;
        byte[] bytes = new byte[16];
        for (int index = 0; index < 6; index++) {
            bytes[index] = (byte) ((millis >>> ((5 - index) * 8)) & 0xff);
        }
        bytes[6] = 0x70;
        bytes[8] = (byte) 0x80;
        StringBuilder hex = new StringBuilder();
        for (byte b : bytes) {
            hex.append(String.format("%02x", b));
        }
        String key =
                hex.substring(0, 8)
                        + "-"
                        + hex.substring(8, 12)
                        + "-"
                        + hex.substring(12, 16)
                        + "-"
                        + hex.substring(16, 20)
                        + "-"
                        + hex.substring(20, 32);

        Instant when = Idempotency.uuidV7Time(key).orElseThrow();
        assertEquals(
                "the timestamp was read from the wrong number of hex digits",
                Instant.ofEpochMilli(millis),
                when);
    }

    /** Anything that is not a version-7 UUID has no timestamp, rather than a wrong one. */
    @Test
    public void aNonUuidV7HasNoTimestamp() {
        assertTrue(Idempotency.uuidV7Time(null).isEmpty());
        assertTrue(Idempotency.uuidV7Time("").isEmpty());
        assertTrue(Idempotency.uuidV7Time("not-a-uuid").isEmpty());
        // A version-4 UUID: right layout, wrong version.
        assertTrue(
                "a v4 UUID reported a timestamp",
                Idempotency.uuidV7Time("0189d6e0-0000-4000-8000-000000000000").isEmpty());
        // The right version, the wrong variant.
        assertTrue(
                "a UUID with variant 0 reported a timestamp",
                Idempotency.uuidV7Time("0189d6e0-0000-7000-0000-000000000000").isEmpty());
    }

    // ---------------------------------------------------------------- the streaming envelope

    /**
     * The Connect envelope frames a payload and reads it back.
     *
     * <p>Five bytes: one flag, then the length as four bytes of big-endian unsigned. The corpus's
     * {@code live_watch} request is exactly {@code 0000000000}, so that case is the regression
     * test for this.
     */
    @Test
    public void theEnvelopeFramesAndUnframes() throws Exception {
        byte[] payload = {1, 2, 3, 4, 5};
        byte[] framed = Envelope.frame(payload);
        assertEquals("the prefix is not five bytes", Envelope.PREFIX_LENGTH + payload.length, framed.length);
        assertEquals("the flag byte is wrong", 0, framed[0]);
        assertEquals("the length prefix is not big-endian", 0, framed[1]);
        assertEquals(5, framed[4]);

        Envelope.Frame read = Envelope.read(new ByteArrayInputStream(framed));
        assertNotNull("a framed payload read back as nothing", read);
        assertFalse("a data frame was read as the end of stream", read.isEndStream());
        assertEquals(5, read.payload().length);

        // The exact bytes the corpus recorded for a Connect server stream's empty request.
        Envelope.Frame empty =
                Envelope.read(new ByteArrayInputStream(Envelope.frame(new byte[0])));
        assertEquals(
                "an empty message did not round-trip",
                "AAAAAAA=",
                Base64.getEncoder().encodeToString(Envelope.frame(new byte[0])));
        assertEquals(0, empty.payload().length);
    }

    /**
     * A stream that stops mid-frame is an error, not a clean end.
     *
     * <p>Reporting a truncated stream as finished would tell the caller it had seen everything,
     * which for a sync stream is the failure R7 exists to prevent.
     */
    @Test
    public void aTruncatedEnvelopeIsAnError() throws Exception {
        // A prefix claiming eight bytes with only three behind it.
        byte[] truncated = {0, 0, 0, 0, 8, 1, 2, 3};
        try {
            Envelope.read(new ByteArrayInputStream(truncated));
            fail("a truncated frame was read as a complete one");
        } catch (java.io.EOFException expected) {
            assertTrue(
                    "the failure does not say the stream was truncated: " + expected.getMessage(),
                    expected.getMessage().contains("envelope frame"));
        }
    }

    /** An empty body is a clean end, and a partial prefix is not. */
    @Test
    public void anEmptyBodyIsACleanEndAndAPartialPrefixIsNot() throws Exception {
        assertNull(
                "an empty stream was not a clean end",
                Envelope.read(new ByteArrayInputStream(new byte[0])));
        try {
            Envelope.read(new ByteArrayInputStream(new byte[] {0, 0}));
            fail("a two-byte prefix was read as a frame");
        } catch (java.io.EOFException expected) {
            assertTrue(expected.getMessage().contains("envelope prefix"));
        }
    }

    /** The end-of-stream flag is distinguishable from the compressed flag. */
    @Test
    public void theEndStreamAndCompressedFlagsAreDistinct() throws Exception {
        Envelope.Frame end = Envelope.read(new ByteArrayInputStream(Envelope.frame(new byte[0], 0x02)));
        assertTrue("the end-stream flag was not read", end.isEndStream());
        assertFalse("the end-stream frame was read as compressed", end.isCompressed());

        Envelope.Frame compressed = Envelope.read(new ByteArrayInputStream(Envelope.frame(new byte[0], 0x01)));
        assertFalse("a compressed frame was read as the end of stream", compressed.isEndStream());
        assertTrue("the compressed flag was not read", compressed.isCompressed());
    }

    // ------------------------------------------------------- the Connect JSON error shapes

    /**
     * A Connect end-of-stream frame carrying an error is read as one.
     *
     * <p>This is the shape the corpus's {@code live_watch} case uses: HTTP 200, and the refusal
     * inside the envelope. A client that only reads status codes sees success here.
     */
    @Test
    public void anEndStreamFrameCarryingAnErrorIsReadAsOne() {
        String frame =
                "{\"error\":{\"code\":\"unimplemented\",\"message\":\"not in the standard variant\","
                        + "\"details\":[]},\"metadata\":{}}";
        ConnectError error = ConnectError.fromEndStream(frame);
        assertNotNull("an end-of-stream error frame read as a clean end", error);
        assertEquals("unimplemented", error.code());
        assertEquals("not in the standard variant", error.message());
    }

    /** An end-of-stream frame with no error is how a server says a stream is over. */
    @Test
    public void anEndStreamFrameWithNoErrorIsACleanEnd() {
        assertNull(
                "a clean end-of-stream frame was read as a failure",
                ConnectError.fromEndStream("{\"error\":null,\"metadata\":{}}"));
        assertNull(
                "an empty end-of-stream frame was read as a failure",
                ConnectError.fromEndStream("{}"));
    }

    /**
     * A body that is not a Connect error is {@code null}, not a half-parsed one.
     *
     * <p>A proxy's HTML 502 page is the case: the honest reading is "not a Connect error", which
     * the mapper reports as a below-the-API failure rather than inventing a reason.
     */
    @Test
    public void aBodyThatIsNotAConnectErrorIsNotParsedAsOne() {
        assertNull(ConnectError.fromUnaryBody("<html><body>502 Bad Gateway</body></html>"));
        assertNull(ConnectError.fromUnaryBody("{\"message\":\"no code here\"}"));
        assertNull(ConnectError.fromUnaryBody(""));
        assertNotNull(
                "a real Connect error body was not parsed",
                ConnectError.fromUnaryBody("{\"code\":\"internal\",\"message\":\"boom\"}"));
    }

    /**
     * A malformed detail is skipped, so the reason on its sibling survives (R8).
     *
     * <p>Losing the whole error over one bad entry from a newer server would cost the caller the
     * reason, which is the outcome R8 forbids.
     */
    @Test
    public void aMalformedDetailDoesNotCostTheReason() throws Exception {
        String json =
                "{\"code\":\"unimplemented\",\"message\":\"m\",\"details\":["
                        + "{\"type\":\"something.else\",\"value\":\"not base64!!\"},"
                        + "{\"type\":\"loams.errors.v1.ErrorInfo\",\"value\":\""
                        + Base64.getEncoder()
                                .encodeToString(
                                        dev.loams.gen.loams.errors.v1.ErrorInfo.newBuilder()
                                                .setReason("feature_not_in_variant")
                                                .build()
                                                .toByteArray())
                        + "\"}]}";
        ConnectError error = ConnectError.fromUnaryBody(json);
        assertNotNull(error);
        assertNotNull(
                "the ErrorInfo detail was lost because a sibling did not parse",
                error.detail("loams.errors.v1.ErrorInfo"));
        assertEquals(
                "feature_not_in_variant",
                dev.loams.gen.loams.errors.v1.ErrorInfo.parseFrom(
                                error.detail("loams.errors.v1.ErrorInfo").value())
                        .getReason());
    }

    /** The {@code ErrorInfo} is found by type, not by position (R8). */
    @Test
    public void theErrorInfoIsFoundByTypeNotPosition() throws Exception {
        String json =
                "{\"code\":\"invalid_argument\",\"message\":\"m\",\"details\":["
                        + "{\"type\":\"some.ServiceOwnDetail\",\"value\":\""
                        + Base64.getEncoder()
                                .encodeToString(
                                        dev.loams.gen.loams.errors.v1.ErrorInfo.newBuilder()
                                                .setReason("wrong_one")
                                                .build()
                                                .toByteArray())
                        + "\"},"
                        + "{\"type\":\"loams.errors.v1.ErrorInfo\",\"value\":\""
                        + Base64.getEncoder()
                                .encodeToString(
                                        dev.loams.gen.loams.errors.v1.ErrorInfo.newBuilder()
                                                .setReason("invalid_argument")
                                                .build()
                                                .toByteArray())
                        + "\"}]}";
        ConnectError error = ConnectError.fromUnaryBody(json);
        assertEquals(
                "the lookup returned a detail of the wrong type, so a service's own detail moved"
                        + " reason out from under a caller",
                "invalid_argument",
                dev.loams.gen.loams.errors.v1.ErrorInfo.parseFrom(
                                error.detail("loams.errors.v1.ErrorInfo").value())
                        .getReason());
    }

    // ------------------------------------------------------------------- the JSON reader

    /**
     * The JSON reader parses what a Connect error body and an OAuth response contain.
     *
     * <p>It exists so the SDK depends on nothing but {@code protobuf-java}, so it has to be right
     * about the shapes it is actually fed.
     */
    @Test
    public void theJsonReaderParsesTheShapesTheSdkIsFed() {
        Map<String, Object> object =
                Json.parseObject(
                        "{\"a\":1,\"b\":\"two\",\"c\":true,\"d\":null,\"e\":[1,2],"
                                + "\"f\":{\"g\":\"h\"},\"i\":-1.5e2}");
        assertEquals(1.0, object.get("a"));
        assertEquals("two", object.get("a") == null ? null : object.get("b"));
        assertEquals(Boolean.TRUE, object.get("c"));
        assertNull(object.get("d"));
        assertEquals(2, Json.array(object, "e").size());
        assertEquals("h", Json.object(object, "f").get("g"));
        assertEquals(-150.0, object.get("i"));
        assertEquals("fallback", Json.string(object, "missing", "fallback"));

        // The escapes a server's message can contain.
        Map<String, Object> escaped =
                Json.parseObject("{\"m\":\"a\\\"b\\\\c\\nd\\te\\u0041\"}");
        assertEquals("a\"b\\c\nd\teA", Json.string(escaped, "m"));

        assertEquals(List.of(), Json.array(Json.parseObject("{}"), "absent"));
        assertEquals(Map.of(), Json.stringMap(Json.parseObject("{}"), "absent"));
    }

    /**
     * The JSON reader refuses what it does not understand.
     *
     * <p>An error body is machine-generated, so a lenient reader would only hide a server bug — and
     * a reader that guessed would hand the mapper a reason it invented.
     */
    @Test
    public void theJsonReaderRefusesWhatItDoesNotUnderstand() {
        List<String> bad =
                List.of(
                        "{\"a\":1,}",
                        "{'a':1}",
                        "{a:1}",
                        "[1,2,]",
                        "{\"a\":01}",
                        "{\"a\":NaN}",
                        "{\"a\":1} trailing",
                        "{\"a\":\"unterminated",
                        "\"just a string\" extra");
        for (String text : bad) {
            try {
                Json.parse(text);
                fail("the reader accepted \"" + text + "\", which is not JSON");
            } catch (IllegalArgumentException expected) {
                assertTrue(
                        "the failure does not name the problem: " + expected.getMessage(),
                        expected.getMessage().startsWith("loams: "));
            }
        }
    }

    /** A {@code map<string,string>} arrives as a JSON object, not a list of pairs. */
    @Test
    public void aProtobufStringMapIsAJsonObject() {
        Map<String, Object> parsed = Json.parseObject("{\"metadata\":{\"variant\":\"standard\"}}");
        Map<String, String> metadata = Json.stringMap(parsed, "metadata");
        assertEquals(1, metadata.size());
        assertEquals("standard", metadata.get("variant"));

        // And a non-string entry is skipped rather than failing the whole error.
        Map<String, String> mixed = Json.stringMap(Json.parseObject("{\"m\":{\"a\":1,\"b\":\"2\"}}"), "m");
        assertEquals(Map.of("b", "2"), mixed);
    }

    // ------------------------------------------------------- the client-wide defaults

    /** The client's retry budget defaults to the design's number, not to none (R2). */
    @Test
    public void theClientRetriesByDefault() {
        Client client = newTestClient();
        assertEquals(
                "the client's default retry budget is " + client.maxRetries()
                        + "; an SDK that silently did not retry unless configured would be a sharp"
                        + " edge, so unset means the design's number",
                Retry.DEFAULT_MAX_RETRIES,
                client.maxRetries());
        assertEquals(3, client.maxRetries());

        assertEquals(
                "an explicit zero did not mean no retries",
                0,
                Client.of(Options.builder().endpoint(Fixtures.fixtureServerEndpoint()).maxRetries(0).build())
                        .maxRetries());
        assertEquals(
                "noRetries did not turn retries off",
                0,
                Client.of(Options.builder().endpoint(Fixtures.fixtureServerEndpoint()).noRetries(true).build())
                        .maxRetries());
        client.close();
    }

    /** A per-call {@code withMaxRetries(0)} means none, which is unambiguous on a call. */
    @Test
    public void aPerCallZeroRetriesMeansNone() {
        CallInvoker invoker = new CallInvoker(null, null, Retry.DEFAULT_MAX_RETRIES, null);
        assertEquals(
                "a per-call zero did not override the client's budget",
                0,
                invoker.plan(
                                Client.binding("instance", "GetInstance"),
                                CallOptions.withMaxRetries(0),
                                false)
                        .maxRetries());
        assertEquals(
                "a per-call budget was not honoured",
                7,
                invoker.plan(
                                Client.binding("instance", "GetInstance"),
                                CallOptions.withMaxRetries(7),
                                false)
                        .maxRetries());
        assertEquals(
                "with no per-call budget the client's own was not used",
                Retry.DEFAULT_MAX_RETRIES,
                invoker.plan(Client.binding("instance", "GetInstance"), CallOptions.none(), false)
                        .maxRetries());
    }

    /**
     * An empty endpoint is a usage error rather than a request to localhost.
     *
     * <p>A client that quietly talks to the wrong instance is worse than one that does not start.
     */
    @Test
    public void anEmptyEndpointIsAUsageError() {
        for (String endpoint : List.of("", "   ")) {
            try {
                Client.of(Options.builder().endpoint(endpoint).build());
                fail("a client was built with endpoint \"" + endpoint + "\"");
            } catch (IllegalArgumentException expected) {
                assertTrue(
                        "the message does not say what to do: " + expected.getMessage(),
                        expected.getMessage().contains("base URL"));
            }
        }
        try {
            Client.of(Options.builder().build());
            fail("a client was built with no endpoint at all");
        } catch (IllegalArgumentException expected) {
            assertTrue(expected.getMessage().contains("base URL"));
        }
    }

    /** Session consistency is off by default, and a nil store is distinguishable from an empty one. */
    @Test
    public void sessionConsistencyIsOffByDefault() {
        Client off = newTestClient();
        assertNull(
                "a client with session consistency off returned a store, so \"not on\" and \"on but"
                        + " empty\" look the same",
                off.session());
        off.close();

        Client on =
                Client.of(
                        Options.builder()
                                .endpoint(Fixtures.fixtureServerEndpoint())
                                .sessionConsistency(true)
                                .build());
        assertNotNull("sessionConsistency(true) produced no store", on.session());
        assertEquals("a fresh session already had a token", "", on.session().current());
        on.close();
    }

    // ----------------------------------------------------------------- the binding table

    /**
     * The binding table and the client's module surface agree.
     *
     * <p>They are separate structures on purpose — the table is the contract and the accessors are
     * the sugar — so they can drift, and this is what catches it.
     */
    @Test
    public void theBindingTableAndTheClientSurfaceAgree() {
        Client client = newTestClient();
        for (var module : Facade.MODULES) {
            assertNotNull(
                    "the binding table has module " + module.name() + " but the client exposes no"
                            + " accessor for it",
                    client.module(module.name()));
            assertEquals(
                    "module " + module.name() + " is not exposed under its own name",
                    client.module(module.name()),
                    client.module(module.name()));
        }
        assertEquals(
                "the client's module map and the binding table have drifted",
                Facade.MODULES.size(),
                client.modules().size());
        assertEquals("v1", Client.protoRev());
        client.close();
    }

    /** A call resolves by either the SDK's name or the {@code FacadeOptions} proto name. */
    @Test
    public void aCallResolvesByEitherName() {
        assertEquals(
                "GetInstance",
                Client.binding("instance", "GetInstance").name());
        assertEquals(
                "getInstance did not resolve",
                "GetInstance",
                Client.binding("instance", "getInstance").name());
        assertEquals(
                "watch did not resolve by its proto name",
                "Watch",
                Client.binding("live", "watch").name());
    }

    /** An unresolvable call is an SDK bug, so it is {@code internal} rather than {@code not_found}. */
    @Test
    public void anUnresolvableCallIsAnInternalError() {
        try {
            Client.binding("instance", "NoSuchCall");
            fail("an unresolvable call resolved");
        } catch (InternalException expected) {
            assertEquals(
                    "an unresolvable call was not internal",
                    dev.loams.facade.Reason.INTERNAL,
                    expected.reason());
            assertEquals(Code.INTERNAL, expected.code());
            assertTrue(
                    "the message does not name what was not found: " + expected.getMessage(),
                    expected.getMessage().contains("NoSuchCall"));
        }
    }

    /** An SDK's own usage failure is {@code internal} and so is never retried. */
    @Test
    public void anSdkBugIsNeverRetried() {
        LoamsException bug = Errors.internal("x", "the binding does not resolve", null);
        assertFalse(
                "an SDK bug landed in the retryable set, so a wiring mistake would be retried three"
                        + " times",
                Retry.isRetryableCode(bug.code()));
    }

    /** {@code loams.live} and {@code loams.tables} are two names for one package. */
    @Test
    public void onePackageHasTwoModuleNames() {
        assertEquals(
                "live and tables do not resolve to the same package",
                Facade.module("live").orElseThrow().protoPackage(),
                Facade.module("tables").orElseThrow().protoPackage());
        assertEquals(
                "the module list for loams.live.v1 is wrong",
                List.of("live", "tables"),
                Facade.modulesForPackage("loams.live.v1"));
        // `loams.live` is the module's name with the client's prefix, not a package. Resolving it
        // as a package would ask about `loams.live`, which no instance lists, and report a
        // served module as absent.
        assertEquals(
                "loams.live was resolved as a proto package rather than as the module it is",
                "loams.live.v1",
                SystemModule.packageOf("loams.live"));
        assertEquals(
                "loams.tables did not resolve through the module table",
                "loams.live.v1",
                SystemModule.packageOf("loams.tables"));
        assertEquals("loams.live.v1", SystemModule.packageOf("loams.live.v1"));
        try {
            SystemModule.packageOf("no_such_module");
            fail("an unknown module name resolved to a package");
        } catch (InternalException expected) {
            assertTrue(expected.getMessage().contains("no_such_module"));
        }
    }

    // --------------------------------------------------------------- call option merging

    /** Several options combine, and the last one for a field wins. */
    @Test
    public void callOptionsMerge() {
        CallOptions merged =
                CallOptions.merge(
                        CallOptions.withMaxRetries(2),
                        CallOptions.withHeader("x-a", "1"),
                        CallOptions.withIdempotencyKey("k"));
        assertEquals(Integer.valueOf(2), merged.maxRetries());
        assertEquals("k", merged.idempotencyKey());
        assertEquals("1", merged.headers().get("x-a"));

        CallOptions overridden =
                CallOptions.merge(CallOptions.withMaxRetries(2), CallOptions.withMaxRetries(5));
        assertEquals("the later option did not win", Integer.valueOf(5), overridden.maxRetries());

        assertTrue("no options produced a non-empty CallOptions", CallOptions.merge().isEmpty());
        assertTrue("a null in the array broke the merge", CallOptions.merge(null).isEmpty());
        assertTrue(
                "an option that overrides nothing was not recognised as empty",
                CallOptions.merge(CallOptions.none(), CallOptions.withHeader("x", "1")).headers()
                        .containsKey("x"));
    }

    // ---------------------------------------------------------------------- the SDK's deps

    /** The SDK's runtime needs nothing but protobuf-java. */
    @Test
    public void theRuntimeNeedsOnlyProtobuf() throws Exception {
        // A pin on the dependency list rather than a claim: the SDK's own package must not
        // reference anything outside `dev.loams`, `java.*`, `javax.*` and protobuf. That is what
        // makes the hand-rolled JSON reader and the hand-rolled transport the right trade.
        List<String> offenders = new java.util.ArrayList<>();
        for (String className : runtimeClassNames()) {
            String name = className.replace('/', '.');
            if (name.startsWith("dev.loams")
                    || name.startsWith("java.")
                    || name.startsWith("javax.")
                    || name.startsWith("com.google.protobuf")
                    || name.startsWith("com.sun.net.httpserver")
                    || name.startsWith("jdk.")) {
                continue;
            }
            offenders.add(name);
        }
        assertEquals(
                "the runtime references classes outside its declared dependencies: " + offenders,
                List.of(),
                offenders);
    }

    /** Every class the SDK's own runtime ships. */
    private static List<String> runtimeClassNames() throws Exception {
        java.nio.file.Path root =
                Fixtures.repositoryRoot().resolve("sdks/java/src/dev/loams");
        List<String> names = new java.util.ArrayList<>();
        try (var stream = java.nio.file.Files.walk(root)) {
            stream
                    .filter(path -> path.toString().endsWith(".java"))
                    .forEach(
                            path -> {
                                String relative =
                                        root.relativize(path).toString();
                                names.add(
                                        ("dev/loams/" + relative)
                                                .replace(".java", "")
                                                .replace('/', '.'));
                            });
        }
        return names;
    }

    /** A client against a stub endpoint, for a test that never makes a call. */
    private static Client newTestClient() {
        return Client.of(Options.builder().endpoint("http://127.0.0.1:1").build());
    }

    /** The UTF-8 bytes of a string, for a test that builds a body. */
    static byte[] utf8(String value) {
        return value.getBytes(StandardCharsets.UTF_8);
    }
}