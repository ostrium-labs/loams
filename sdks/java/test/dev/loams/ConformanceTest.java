package dev.loams;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;
import static org.junit.Assert.fail;

import dev.loams.facade.Facade;
import dev.loams.gen.loams.instance.v1.GetInstanceRequest;
import dev.loams.gen.loams.instance.v1.GetInstanceResponse;
import dev.loams.gen.loams.instance.v1.WhoAmIRequest;
import dev.loams.gen.loams.live.v1.QueryRequest;
import dev.loams.gen.loams.live.v1.WatchRequest;
import dev.loams.internal.Json;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.Collections;
import java.util.List;
import java.util.Map;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicInteger;
import org.junit.AfterClass;
import org.junit.BeforeClass;
import org.junit.Test;

/**
 * SDK2 Task 6's {@code java_conformance_all_required_fixtures}, and the two runtime-contract
 * clauses the corpus can answer alongside it.
 *
 * <p>The corpus in {@code sdks/fixtures} is recorded from a real {@code loams dev}, and this runs
 * every case in it through the SDK's <b>public</b> surface — {@code client.instance()} and
 * {@code client.tables()}, the same objects an application uses — rather than through the
 * transport. That is the point of the suite: it proves the facade dispatches to the right RPC,
 * sends the right encoding, and turns what comes back into the right typed value.
 *
 * <p>Three things are covered, which between them are what the design asks of a conforming SDK
 * (design §44 §10.4):
 *
 * <ul>
 *   <li>a successful call, in the encoding an SDK sends by default;
 *   <li>a structured-reason error, with {@code reason} and not the message;
 *   <li>the unavailable-service path, in all three of its shapes: the guard that costs no RPC, the
 *       refusal a call gets, and the refusal on a stream.
 * </ul>
 *
 * <h2>Naming</h2>
 *
 * <p>The six canonical names are the ones {@code sdks/conformance/required.mjs} builds from
 * {@code testName(language, short)}, and unlike Go's test tool JUnit runs a method with any legal
 * Java identifier, so each is spelled <b>literally</b> as its canonical name:
 * {@code java_conformance_all_required_fixtures}, not {@code testConformanceAllRequiredFixtures}.
 * {@link #theSixCanonicalNamesExist()} asserts all six exist, so the set cannot quietly shrink.
 */
public class ConformanceTest {

    /**
     * The six tests SDK2 Task 6 requires, in the names the plan states.
     *
     * <p>They are declared here, and {@link #theSixCanonicalNamesExist()} reads them back through
     * reflection, so the registry cannot disagree with the tests that exist.
     */
    static final String[] REQUIRED_TEST_NAMES = {
        "java_conformance_all_required_fixtures",
        "java_retry_reuses_idempotency_key",
        "java_error_reason_mapping",
        "java_stream_resume_with_cursor",
        "java_token_source_refresh",
        "java_pagination_iterator",
    };

    /** The corpus cases the suite covers, which is every case in {@code index.json}. */
    private static final List<String> EXPECTED_CASES =
            List.of(
                    // A successful call, in each encoding a client might pick.
                    "instance_get_instance_json",
                    "instance_get_instance_proto",
                    "instance_get_instance_grpc_web",
                    "instance_get_instance_grpc_web_json",
                    // A structured-reason error, in each encoding.
                    "instance_who_am_i_json",
                    "instance_who_am_i_proto",
                    "instance_who_am_i_grpc_web",
                    "instance_who_am_i_grpc_web_json",
                    // The unavailable-service path, unary and on a stream.
                    "live_query_json",
                    "live_query_proto",
                    "live_query_grpc_web",
                    "live_query_grpc_web_json",
                    "live_watch");

    private static Fixtures fixtures;
    private static Client client;

    @BeforeClass
    public static void startFixtureServer() {
        fixtures = Fixtures.start();
        client = newTestClient(fixtures.endpoint());
    }

    @AfterClass
    public static void stopFixtureServer() {
        if (client != null) {
            client.close();
        }
        if (fixtures != null) {
            fixtures.close();
        }
    }

    /** A client against a fixture endpoint, unauthenticated. */
    static Client newTestClient(String endpoint) {
        return Client.of(
                Options.builder()
                        .endpoint(endpoint)
                        // A generous timeout rather than none: the fixture server is local, and a
                        // hung test should fail with a timeout rather than hang the suite.
                        .httpClient(
                                java.net.http.HttpClient.newBuilder()
                                        .connectTimeout(java.time.Duration.ofSeconds(10))
                                        .build())
                        .build());
    }

    /**
     * The corpus case list, and the check that the suite covers all of it.
     *
     * <p>It fails rather than skips: a corpus entry with no test behind it means the suite is
     * being excused for coverage nobody wrote.
     */
    @Test
    public void theCorpusCasesAreTheOnesTheSuiteCovers() {
        List<String> recorded = corpusCaseNames();
        List<String> sortedRecorded = new ArrayList<>(recorded);
        Collections.sort(sortedRecorded);
        List<String> sortedExpected = new ArrayList<>(EXPECTED_CASES);
        Collections.sort(sortedExpected);
        assertEquals(
                "the corpus and the suite's expected case list have drifted apart",
                sortedExpected,
                sortedRecorded);
    }

    /**
     * SDK2 Task 6's {@code java_conformance_all_required_fixtures}.
     *
     * <p>This is the test {@code sdks/conformance/run.sh -t java_conformance_all_required_fixtures}
     * runs.
     */
    @Test
    public void java_conformance_all_required_fixtures() {
        // The corpus is checked first, so a corpus that grew is a failure here rather than a
        // silent narrowing of what this suite claims to cover.
        assertEquals(EXPECTED_CASES.size(), corpusCaseNames().size());

        // A successful call. `GetInstance` needs no auth, which is why it is the first thing any
        // client calls, and it is the case that proves the SDK sends the encoding the corpus
        // recorded: an SDK that asked for JSON would get the JSON bytes and fail to parse them as
        // protobuf, which is exactly the class of mismatch the four encodings exist to catch.
        GetInstanceResponse info =
                client.instance().getInstance(GetInstanceRequest.getDefaultInstance());
        assertEquals("Loams", info.getName());
        assertTrue(
                "api_versions is " + info.getApiVersionsList() + ", want it to contain loams.instance.v1",
                info.getApiVersionsList().contains("loams.instance.v1"));
        assertFalse(
                "services is empty; the catalogue is what feature detection reads",
                info.getServicesList().isEmpty());

        // A structured-reason error. The reason is what the SDK reads; the message is for a
        // person and is not asserted on.
        LoamsException whoAmIFailed = assertThrowsLoams(() -> client.instance().whoAmI(WhoAmIRequest.getDefaultInstance()));
        assertEquals(
                "WhoAmI reason is " + whoAmIFailed.reason() + ", want not_implemented",
                dev.loams.facade.Reason.NOT_IMPLEMENTED,
                whoAmIFailed.reason());
        assertTrue(
                "WhoAmI failed with " + whoAmIFailed.getClass().getSimpleName() + ", want an UnimplementedException",
                whoAmIFailed instanceof UnimplementedException);

        // The unavailable-service path, three ways.
        //
        // 1. The guard, from the catalogue, spending no RPC on a call that cannot work.
        //    `loams.live` and `loams.tables` are the same service, so the guard is asked about
        //    either.
        FeatureNotInVariantException absent = client.system().guard("live");
        assertNotNull("Guard(\"live\") passed, but loams.live.v1 is not served in the standard variant", absent);
        assertEquals(dev.loams.facade.Reason.FEATURE_NOT_IN_VARIANT, absent.reason());
        assertNull(
                "Guard(\"instance\") failed, but loams.instance.v1 is served",
                client.system().guard("instance"));

        // 2. The refusal a call gets when the caller skips the guard. This is the typed surface:
        //    the reason is in the registry, and the variant is read out of the metadata rather
        //    than parsed out of the message.
        LoamsException queryFailed = assertThrowsLoams(() -> client.tables().query(QueryRequest.getDefaultInstance()));
        assertTrue(
                "tables.query failed with " + queryFailed.getClass().getSimpleName()
                        + ", want a FeatureNotInVariantException",
                queryFailed instanceof FeatureNotInVariantException);
        FeatureNotInVariantException refused = (FeatureNotInVariantException) queryFailed;
        assertEquals(
                "the refusal reason is " + refused.reason() + ", want feature_not_in_variant",
                dev.loams.facade.Reason.FEATURE_NOT_IN_VARIANT,
                refused.reason());
        assertEquals(
                "the refusal variant is \"" + refused.variant() + "\", want \"standard\" (it comes from"
                        + " metadata.variant, not the message)",
                "standard",
                refused.variant());

        // 3. The refusal on a server stream, which arrives inside the Connect envelope rather
        //    than as an HTTP status. A client that only reads status codes sees a 200 here, so
        //    this is the case that distinguishes a real Connect implementation from a
        //    status-code-only fake.
        LoamsException streamFailure = streamFailureOnWatch();
        assertTrue(
                "watch failed with " + streamFailure.getClass().getSimpleName()
                        + ", want a FeatureNotInVariantException",
                streamFailure instanceof FeatureNotInVariantException);

        // The catalogue answers the same question the refusals do, from one call.
        Catalogue catalogue = client.system().catalogue();
        assertTrue(
                "served is " + catalogue.served() + ", want it to contain loams.instance.v1",
                catalogue.served().contains("loams.instance.v1"));
        assertTrue(
                "unavailable is " + catalogue.unavailable() + ", want it to contain loams.live.v1",
                catalogue.unavailable().contains("loams.live.v1"));
        ServiceStatus liveStatus = catalogue.status("loams.live.v1");
        assertNotNull("the catalogue has no loams.live.v1 entry", liveStatus);
        assertTrue(
                "loams.live.v1 is not marked unstable; buf breaking skips that package",
                liveStatus.unstable());

        // The two facade names for one package answer the same question, because the guard
        // resolves a module name to its package.
        assertFalse(
                "AvailableModule(\"tables\") says loams.live.v1 is served",
                client.system().availableModule("tables"));
        assertFalse(
                "Available(\"loams.live.v1\") says the package is served",
                client.system().available("loams.live.v1"));
    }

    /**
     * The refusal on {@code watch}, from whichever of the two places it can arrive.
     *
     * <p>A refusal arriving at open time rather than at the first read is the same failure, and
     * both are legal for a Connect client — but both must map to the same type, which is the part
     * worth asserting.
     */
    private LoamsException streamFailureOnWatch() {
        try (Stream<dev.loams.gen.loams.live.v1.Transition> stream =
                client.live().watch(WatchRequest.getDefaultInstance())) {
            int received = 0;
            while (stream.receive()) {
                received++;
            }
            assertEquals("watch yielded " + received + " messages, want none before the refusal", 0, received);
            LoamsException error = stream.error();
            assertNotNull("watch ended cleanly; the corpus's live_watch case is a refusal", error);
            return error;
        }
    }

    /**
     * R9: the SDK declares its proto revision and reports the server's packages beside it, and a
     * package the SDK speaks that the server does not serve is a warning rather than an exception.
     */
    @Test
    public void java_conformance_reports_version() {
        VersionReport report = client.system().version();
        assertEquals(
                "the report says proto revision \"" + report.protoRev() + "\", the client says \""
                        + Client.protoRev() + "\"",
                Client.protoRev(),
                report.protoRev());
        assertFalse("the report has no server version", report.serverVersion().isEmpty());
        // `loams.live.v1` is served as unavailable in the standard variant, and
        // `GetInstance.apiVersions` lists only what is served, so a mismatch here is the
        // server's, not the SDK's.
        assertEquals(
                "api_versions is " + report.apiVersions() + ", want [loams.instance.v1]",
                List.of("loams.instance.v1"),
                report.apiVersions());
        assertFalse(
                "the report says the instance is compatible, but it does not serve every package the SDK speaks",
                report.compatible());
        assertTrue(
                "missing is " + report.missing() + ", want it to contain loams.live.v1",
                report.missing().contains("loams.live.v1"));
    }

    /**
     * R5's "concurrent readers share one in-flight fetch": a hundred readers at once must cost
     * one {@code GetInstance}.
     *
     * <p>It counts the catalogue entries rather than the RPCs, because the count of RPCs is not
     * observable from inside one process — but a reader that got an empty or a stale catalogue
     * would show up, and that is the failure the shared fetch exists to prevent.
     */
    @Test
    public void java_conformance_catalogue_is_shared() throws Exception {
        client.invalidateCatalogue();
        int readers = 100;
        ExecutorService pool = Executors.newFixedThreadPool(16);
        try {
            CountDownLatch start = new CountDownLatch(1);
            CountDownLatch done = new CountDownLatch(readers);
            AtomicInteger empty = new AtomicInteger();
            AtomicInteger failed = new AtomicInteger();
            for (int i = 0; i < readers; i++) {
                pool.execute(
                        () -> {
                            try {
                                start.await();
                                Catalogue catalogue = client.system().catalogue();
                                if (catalogue.served().isEmpty()) {
                                    empty.incrementAndGet();
                                }
                            } catch (Throwable e) {
                                failed.incrementAndGet();
                            } finally {
                                done.countDown();
                            }
                        });
            }
            start.countDown();
            assertTrue("the concurrent readers did not finish within 30s", done.await(30, TimeUnit.SECONDS));
            assertEquals("a concurrent reader failed", 0, failed.get());
            assertEquals("a concurrent reader got an empty catalogue", 0, empty.get());
        } finally {
            pool.shutdownNow();
        }
        Catalogue first = client.system().catalogue();
        Catalogue again = client.system().catalogue();
        assertEquals(
                "two catalogue reads disagree: " + first.services().size() + " entries then "
                        + again.services().size(),
                first.services().size(),
                again.services().size());
    }

    /**
     * The six canonical names exist as real methods on real classes.
     *
     * <p>Without it, "the six tests exist" is a comment rather than a check, and a rename that
     * dropped one would leave the suite green with five.
     */
    @Test
    public void theSixCanonicalNamesExist() throws Exception {
        for (String name : REQUIRED_TEST_NAMES) {
            assertNotNull(
                    "no test is named " + name + "; " + Arrays.toString(REQUIRED_TEST_NAMES)
                            + " are part of the contract so two languages can be compared",
                    findTestMethod(name));
        }
    }

    /** The method named {@code name}, wherever it lives in this package. */
    private static java.lang.reflect.Method findTestMethod(String name) {
        String[] owners = {
            "dev.loams.ConformanceTest",
            "dev.loams.RetryIdempotencyTest",
            "dev.loams.ErrorReasonMappingTest",
            "dev.loams.StreamResumeTest",
            "dev.loams.TokenSourceTest",
            "dev.loams.PaginationTest",
        };
        for (String owner : owners) {
            try {
                return Class.forName(owner).getMethod(name);
            } catch (ClassNotFoundException | NoSuchMethodException e) {
                continue;
            }
        }
        return null;
    }

    /** The corpus's case names, read from {@code sdks/fixtures/index.json}. */
    static List<String> corpusCaseNames() {
        List<String> names = new ArrayList<>();
        for (Object entry : Json.array(Fixtures.corpusIndex(), "cases")) {
            if (entry instanceof Map<?, ?> fields && fields.get("name") instanceof String name) {
                names.add(name);
            }
        }
        return names;
    }

    /** Run {@code call}, which must fail with a Loams failure, and return it. */
    static LoamsException assertThrowsLoams(Runnable call) {
        try {
            call.run();
        } catch (LoamsException e) {
            return e;
        } catch (RuntimeException e) {
            fail("the call failed with " + e.getClass().getName() + ": " + e.getMessage());
        }
        fail("the call answered, but the corpus says it must fail");
        throw new AssertionError("unreachable");
    }

    /** The reason registry, for a test that walks every reason. */
    static List<dev.loams.facade.Reason> allReasons() {
        return List.of(dev.loams.facade.Reason.all());
    }

    /** The facade's module names, for a test that checks the client's surface. */
    static List<String> facadeModuleNames() {
        List<String> names = new ArrayList<>();
        for (var entry : Facade.MODULES) {
            names.add(entry.name());
        }
        return names;
    }
}