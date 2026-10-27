package dev.loams;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.fail;

import com.google.protobuf.Message;
import com.google.protobuf.Struct;
import dev.loams.gen.loams.collection.v1.CountDocumentsRequest;
import dev.loams.gen.loams.collection.v1.CreateCollectionRequest;
import dev.loams.gen.loams.collection.v1.CreateNamespaceRequest;
import dev.loams.gen.loams.collection.v1.DropCollectionRequest;
import dev.loams.gen.loams.collection.v1.GetCollectionRequest;
import dev.loams.gen.loams.collection.v1.GetDocumentsRequest;
import dev.loams.gen.loams.collection.v1.ListCollectionsRequest;
import dev.loams.gen.loams.collection.v1.Query;
import dev.loams.gen.loams.collection.v1.ScrollDocumentsRequest;
import dev.loams.gen.loams.collection.v1.SearchRequest;
import dev.loams.gen.loams.collection.v1.WriteDocumentsRequest;
import dev.loams.gen.loams.graph.v1.CreateGraphRequest;
import dev.loams.gen.loams.graph.v1.DeleteGraphRequest;
import dev.loams.gen.loams.graph.v1.ExecuteRequest;
import dev.loams.gen.loams.instance.v1.GetInstanceRequest;
import dev.loams.gen.loams.sql.v1.QueryArrowRequest;
import dev.loams.gen.loams.sql.v1.QueryRequest;
import dev.loams.gen.loams.stream.v1.CreateStreamRequest;
import dev.loams.gen.loams.stream.v1.DescribeStreamRequest;
import dev.loams.gen.loams.stream.v1.FetchCloudEventsRequest;
import dev.loams.gen.loams.stream.v1.FetchRequest;
import dev.loams.gen.loams.stream.v1.ListStreamsRequest;
import dev.loams.gen.loams.stream.v1.ProduceCloudEventsRequest;
import dev.loams.gen.loams.stream.v1.ProduceRequest;
import dev.loams.gen.loams.stream.v1.Record;
import dev.loams.internal.Json;
import dev.loams.internal.ProtoJsonCodec;
import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import java.util.TreeSet;
import org.junit.AfterClass;
import org.junit.BeforeClass;
import org.junit.Test;

/**
 * The app scenario (SDKP P0d), replayed through the public facade.
 *
 * <p>{@code sdks/fixtures/recorded/app/*.json} is a notes app recorded once from {@code loams dev};
 * {@code sdks/fixtures/app-scenario.md} is the prose. Each step's request body is decoded into the
 * call's request message (or built with the builders, where the step names a {@code builder}
 * case), sent through the facade with {@code loams-fixture-name} and {@code loams-fixture-step},
 * and the answer compared with the recording, or for a refusal the typed error's code and reason.
 * The recordings are JSON, so the client is built with {@code Options.json(true)}: the fixture
 * server compares the body it receives with the recorded one as JSON.
 *
 * <p>The streams steps go through the convenience methods ({@code create(name, partitions)},
 * {@code produce} with {@link ProducerRecord}s and a producer key, {@code fetch}, {@code get}),
 * so the replay proves that what those build is what the server was sent.
 *
 * <p>A fixture that replays cleanly is added to {@code sdks/fixtures/results/java.json}'s
 * {@code ran}, which {@code check-languages.mjs --check java --suite app} reads.
 */
public class AppScenarioTest {

    private static Fixtures fixtures;

    @BeforeClass
    public static void startFixtureServer() {
        fixtures = Fixtures.startShared();
    }

    @AfterClass
    public static void stopFixtureServer() {
        if (fixtures != null) {
            fixtures.close();
        }
    }

    @Test
    public void java_app_collections() throws Exception {
        replay("app_collections");
    }

    @Test
    public void java_app_search() throws Exception {
        replay("app_search");
    }

    @Test
    public void java_app_graph() throws Exception {
        replay("app_graph");
    }

    @Test
    public void java_app_sql() throws Exception {
        replay("app_sql");
    }

    @Test
    public void java_app_streams() throws Exception {
        replay("app_streams");
    }

    // ---------------------------------------------------------------- the replay

    private static void replay(String name) throws IOException {
        Path file = Fixtures.repositoryRoot().resolve("sdks/fixtures/recorded/app").resolve(name + ".json");
        Map<String, Object> fixture = Json.parseObject(Files.readString(file));
        List<Object> steps = Json.array(fixture, "steps");
        try (Client client = Client.of(Options.builder()
                .endpoint(fixtures.endpoint())
                .namespace("app")
                .noRetries(true)
                .json(true)
                .httpClient(java.net.http.HttpClient.newBuilder()
                        .connectTimeout(java.time.Duration.ofSeconds(10))
                        .build())
                .build())) {
            for (int at = 0; at < steps.size(); at++) {
                @SuppressWarnings("unchecked")
                Map<String, Object> step = (Map<String, Object>) steps.get(at);
                String call = Json.string(step, "call");
                String place = name + " step " + at + " (" + call + ")";
                CallOptions options = CallOptions.builder()
                        .header("loams-fixture-name", name)
                        .header("loams-fixture-step", Integer.toString(at))
                        .build();
                Map<String, Object> expect = Json.object(step, "expect");
                int status = (int) (double) (Double) expect.get("status");
                Message answer = null;
                LoamsException refused = null;
                try {
                    answer = invoke(client, step, options);
                } catch (LoamsException e) {
                    refused = e;
                }
                Map<String, Object> response = Json.object(step, "response");
                if (status == 200) {
                    if (refused != null) {
                        throw new AssertionError(place + ": refused: " + refused, refused);
                    }
                    assertNotNull(place, answer);
                    Message recorded = ProtoJsonCodec.mergeParsed(response.get("body"),
                            answer.getDefaultInstanceForType().newBuilderForType()).build();
                    assertEquals(place, recorded, answer);
                    continue;
                }
                if (refused == null) {
                    fail(place + ": expected a refusal, got " + answer);
                }
                String reason = refused.reason() != null ? refused.reason().wire() : refused.unknownReason();
                assertEquals(place + ": reason", Json.string(expect, "reason"), reason);
                @SuppressWarnings("unchecked")
                Map<String, Object> body = response.get("body") instanceof Map<?, ?> m ? (Map<String, Object>) m : Map.of();
                String code = Json.string(body, "code");
                if (code != null) {
                    assertEquals(place + ": code", code, refused.code().wire());
                }
            }
        }
        record(name);
    }

    /** The request a step sends: its recorded body, decoded into {@code like}'s type. */
    @SuppressWarnings("unchecked")
    private static <M extends Message> M request(Map<String, Object> step, M like) {
        String body = Json.string(Json.object(step, "request"), "body", "{}");
        return (M) ProtoJsonCodec.merge(body, like.newBuilderForType()).build();
    }

    /** The builder corpus case a step names, built by this SDK's builders. */
    private static Message built(Map<String, Object> step) {
        String builder = Json.string(step, "builder");
        if (builder == null || builder.isEmpty()) {
            return null;
        }
        var entry = AppTest.cases().get(builder);
        if (entry == null) {
            throw new AssertionError("no builder case " + builder + " in AppTest.cases()");
        }
        return entry.getKey();
    }

    private static Message invoke(Client client, Map<String, Object> step, CallOptions options) {
        String call = Json.string(step, "call");
        Message built = built(step);
        switch (call) {
            case "instance.get_instance":
                return client.instance().getInstance(request(step, GetInstanceRequest.getDefaultInstance()), options);
            case "namespaces.create":
                return client.namespaces().create(request(step, CreateNamespaceRequest.getDefaultInstance()), options);
            case "collections.create": {
                CreateCollectionRequest request = request(step, CreateCollectionRequest.getDefaultInstance());
                if (built != null) {
                    request = request.toBuilder().setSchema((Struct) built).build();
                }
                return client.collections().create(request, options);
            }
            case "collections.get":
                return client.collections().get(request(step, GetCollectionRequest.getDefaultInstance()), options);
            case "collections.list":
                return client.collections().list(request(step, ListCollectionsRequest.getDefaultInstance()), options);
            case "collections.drop":
                return client.collections().drop(request(step, DropCollectionRequest.getDefaultInstance()), options);
            case "documents.write":
                return client.documents().write(request(step, WriteDocumentsRequest.getDefaultInstance()), options);
            case "documents.get":
                return client.documents().get(request(step, GetDocumentsRequest.getDefaultInstance()), options);
            case "documents.count": {
                CountDocumentsRequest request = request(step, CountDocumentsRequest.getDefaultInstance());
                if (built != null) {
                    // `where` is the Query message's own JSON, carried as a Struct.
                    Struct where = ProtoJsonCodec.merge(ProtoJsonCodec.print((Query) built), Struct.newBuilder()).build();
                    request = request.toBuilder().setWhere(where).build();
                }
                return client.documents().count(request, options);
            }
            case "documents.scroll":
                return client.documents().scroll(request(step, ScrollDocumentsRequest.getDefaultInstance()), options);
            case "search.query":
                // A builder step is the builder's request as it stands; the client's default
                // namespace fills `namespace`, as it does for any caller.
                return client.search().query(
                        built != null ? (SearchRequest) built : request(step, SearchRequest.getDefaultInstance()), options);
            case "vector.search":
                return client.vector().search(
                        built != null ? (SearchRequest) built : request(step, SearchRequest.getDefaultInstance()), options);
            case "graph.create":
                return client.graph().create(request(step, CreateGraphRequest.getDefaultInstance()), options);
            case "graph.query":
                return client.graph().query(request(step, ExecuteRequest.getDefaultInstance()), options);
            case "graph.delete": {
                DeleteGraphRequest request = request(step, DeleteGraphRequest.getDefaultInstance());
                return client.graph().delete(request.getName(), CallOptions.merge(options,
                        CallOptions.withIdempotencyKey(request.getIdempotencyKey())));
            }
            case "sql.query":
                return client.sql().query(request(step, QueryRequest.getDefaultInstance()), options);
            case "sql.query_arrow":
                return client.sql().queryArrow(request(step, QueryArrowRequest.getDefaultInstance()), options);
            case "streams.create": {
                CreateStreamRequest request = request(step, CreateStreamRequest.getDefaultInstance());
                return client.streams().create(request.getName(), request.getPartitions(), options);
            }
            case "streams.get":
                return client.streams().get(request(step, DescribeStreamRequest.getDefaultInstance()).getStream(), options);
            case "streams.list":
                return client.streams().list(request(step, ListStreamsRequest.getDefaultInstance()), options);
            case "streams.produce": {
                ProduceRequest request = request(step, ProduceRequest.getDefaultInstance());
                List<Object> records = new ArrayList<>();
                for (Record record : request.getRecordsList()) {
                    records.add(new ProducerRecord(
                            record.hasKey() ? record.getKey().toByteArray() : null, record.getValue().toByteArray()));
                }
                return client.streams().produce(request.getStream(), request.getPartition(), records,
                        request.getProducerId().isEmpty() ? null : request.getProducerId(),
                        request.getProducerId().isEmpty() ? null : request.getProducerSeq(), options);
            }
            case "streams.fetch": {
                FetchRequest request = request(step, FetchRequest.getDefaultInstance());
                return client.streams().fetch(request.getStream(), request.getPartition(), request.getOffset(),
                        request.hasMaxRecords() ? request.getMaxRecords() : null,
                        request.hasMaxWaitMs() ? request.getMaxWaitMs() : null, options);
            }
            case "streams.produce_events":
                return client.streams().produceEvents(request(step, ProduceCloudEventsRequest.getDefaultInstance()), options);
            case "streams.fetch_events":
                return client.streams().fetchEvents(request(step, FetchCloudEventsRequest.getDefaultInstance()), options);
            default:
                throw new AssertionError("no replay for " + call);
        }
    }

    /** Adds {@code name} to {@code sdks/fixtures/results/java.json}'s {@code ran}. */
    private static synchronized void record(String name) throws IOException {
        Path path = Fixtures.repositoryRoot().resolve("sdks/fixtures/results/java.json");
        Files.createDirectories(path.getParent());
        TreeSet<String> ran = new TreeSet<>();
        if (Files.isRegularFile(path)) {
            List<Object> before = Json.array(Json.parseObject(Files.readString(path)), "ran");
            if (before != null) {
                before.forEach(item -> ran.add((String) item));
            }
        }
        ran.add(name);
        StringBuilder out = new StringBuilder("{\n  \"language\": \"java\",\n  \"transport\": \"connect\",\n  \"ran\": [");
        boolean first = true;
        for (String item : ran) {
            out.append(first ? "\n    \"" : ",\n    \"").append(item).append('"');
            first = false;
        }
        out.append(ran.isEmpty() ? "]\n}\n" : "\n  ]\n}\n");
        Files.writeString(path, out.toString());
    }
}
