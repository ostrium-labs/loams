package dev.loams;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotEquals;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;
import static org.junit.Assert.fail;

import com.google.protobuf.Message;
import com.google.protobuf.Value;
import dev.loams.facade.CallBinding;
import dev.loams.facade.Facade;
import dev.loams.facade.RetryClass;
import dev.loams.facade.Streaming;
import dev.loams.gen.loams.collection.v1.AnnParams;
import dev.loams.gen.loams.collection.v1.CollectionInfo;
import dev.loams.gen.loams.collection.v1.CountDocumentsRequest;
import dev.loams.gen.loams.collection.v1.CountDocumentsResponse;
import dev.loams.gen.loams.collection.v1.Freshness;
import dev.loams.gen.loams.collection.v1.GetCollectionRequest;
import dev.loams.gen.loams.collection.v1.GetDocumentsRequest;
import dev.loams.gen.loams.collection.v1.GetDocumentsResponse;
import dev.loams.gen.loams.collection.v1.Hit;
import dev.loams.gen.loams.collection.v1.SearchRequest;
import dev.loams.gen.loams.collection.v1.SearchResponse;
import dev.loams.gen.loams.collection.v1.WriteDocumentsRequest;
import dev.loams.gen.loams.collection.v1.WriteDocumentsResponse;
import dev.loams.gen.loams.graph.v1.ExecuteRequest;
import dev.loams.gen.loams.graph.v1.ExecuteResponse;
import dev.loams.gen.loams.graph.v1.RowSet;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.time.Duration;
import java.util.ArrayDeque;
import java.util.ArrayList;
import java.util.Deque;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import org.junit.Test;

/**
 * SDKP (docs/plans/2026-10-19-sdk-parity.md §5, L8): the runtime additions R4, R11–R14, the
 * Schema, Filter and Search builders, the collection handle and the app modules, against a stub
 * Connect server.
 */
public class AppTest {

    private static final String[] MALFORMED = {
        "v2:s1/p0@1", "v1:s1/p0", "v1:s01/p0@1", "v1:s1/p0@1,", "", "v1", "s1/p0@1", "v1:,s1/p0@1",
        "v1:s1/p00@1", "v1:s1/p0@01", "v1:s1/p4294967296@1", "v1:s18446744073709551616/p0@1", "v1:s+1/p0@1",
        "v1:s1/p0@-1", "v1:s1p0@1", "v1:S1/P0@1", "v1: s1/p0@1", "v1:s1/p0@1@2", "v1:s/p0@1",
    };

    // ---------------------------------------------------------------- R4

    @Test
    public void java_consistency_token_merge() {
        ConsistencyToken one = ConsistencyToken.parse("v1:s7/p3@918274");
        assertEquals(List.of(new ConsistencyToken.Position(7, 3, 918274)), one.positions());
        assertEquals("v1:s7/p3@918274", one.toString());
        String max = "v1:s18446744073709551615/p4294967295@18446744073709551615";
        assertEquals(max, ConsistencyToken.parse(max).toString());
        assertEquals("v1:", ConsistencyToken.parse("v1:").toString());

        ConsistencyToken merged = ConsistencyToken.parse("v1:s7/p3@10,s1/p0@5")
                .merge(ConsistencyToken.parse("v1:s7/p3@4,s7/p1@2,s1/p0@9"));
        assertEquals("v1:s1/p0@9,s7/p1@2,s7/p3@10", merged.toString());
        assertEquals(Long.valueOf(10), merged.offset(7, 3).orElseThrow());
        assertTrue(merged.offset(1, 1).isEmpty());
        assertEquals("v1:s1/p5@3,s2/p0@8", ConsistencyToken.of(List.of(
                new ConsistencyToken.Position(2, 0, 1), new ConsistencyToken.Position(1, 5, 3),
                new ConsistencyToken.Position(2, 0, 8), new ConsistencyToken.Position(1, 5, 2))).toString());

        for (String bad : MALFORMED) {
            assertTrue(bad, ConsistencyToken.tryParse(bad).isEmpty());
            try {
                ConsistencyToken.parse(bad);
                fail(bad + " parsed");
            } catch (InvalidArgumentException expected) {
                assertEquals(Code.INVALID_ARGUMENT, expected.code());
            }
        }
    }

    @Test
    public void java_session_read_your_writes() throws Exception {
        try (AppStub stub = new AppStub(
                AppStub.ok(WriteDocumentsResponse.newBuilder().setToken("v1:s4/p0@7").build()),
                AppStub.ok(WriteDocumentsResponse.newBuilder().setToken("v1:s4/p1@2,s4/p0@3").build()),
                AppStub.ok(GetDocumentsResponse.newBuilder().setReadToken("v1:s4/p0@9").build()),
                AppStub.ok(GetDocumentsResponse.getDefaultInstance()))) {
            Client client = stub.client("app", true);
            client.documents().upsert("notes", List.of(new Doc(1, Map.of("title", "a"))));
            client.documents().delete("notes", List.of(2));
            assertEquals("v1:s4/p0@7,s4/p1@2", client.session().current());

            client.documents().get("notes", List.of(1));
            GetDocumentsRequest get = GetDocumentsRequest.parseFrom(stub.requests.get(2).body());
            assertEquals("v1:s4/p0@7,s4/p1@2", get.getConsistency().getAtLeast());
            assertEquals("v1:s4/p0@9,s4/p1@2", client.session().current());

            client.documents().get("notes", List.of(1), Consistent.strong());
            GetDocumentsRequest strong = GetDocumentsRequest.parseFrom(stub.requests.get(3).body());
            assertEquals(Freshness.FRESHNESS_STRONG, strong.getConsistency().getFreshness());
            assertEquals("", strong.getConsistency().getAtLeast());
        }
        try (AppStub stub = new AppStub(AppStub.ok(GetDocumentsResponse.getDefaultInstance()))) {
            Client off = stub.client("app", false);
            off.documents().get("notes", List.of(1));
            assertNull(off.session());
            assertFalse(GetDocumentsRequest.parseFrom(stub.requests.get(0).body()).hasConsistency());
        }
    }

    // ---------------------------------------------------------------- R11

    @Test
    public void java_default_namespace() throws Exception {
        try (AppStub stub = new AppStub(AppStub.ok(CollectionInfo.newBuilder().setName("notes").build()))) {
            Client client = stub.client("app", false);
            client.collections().get("notes");
            client.collections().get(GetCollectionRequest.newBuilder().setCollection("notes").setNamespace("other").build());
            client.collections().get("notes", CallOptions.withNamespace("per-call"));
            List<String> seen = new ArrayList<>();
            for (AppStub.Request r : stub.requests) {
                seen.add(GetCollectionRequest.parseFrom(r.body()).getNamespace());
            }
            assertEquals(List.of("app", "other", "per-call"), seen);

            Client bare = stub.client(null, false);
            try {
                bare.collections().get("notes");
                fail("no namespace went out");
            } catch (InvalidArgumentException expected) {
                assertEquals("namespace", expected.metadata().get("field"));
            }
            assertEquals(3, stub.requests.size());
        }
    }

    // ---------------------------------------------------------------- R12

    @Test
    public void java_timeout_deadline() throws Exception {
        try (AppStub stub = new AppStub(AppStub.hang(2_000))) {
            Client client = Client.of(Options.builder().endpoint(stub.endpoint()).namespace("app")
                    .timeout(Duration.ofMillis(200)).build());
            long started = System.nanoTime();
            try {
                client.collections().get("notes");
                fail("the call outlived its deadline");
            } catch (DeadlineExceededException expected) {
                assertEquals(Deadline.CLIENT_DEADLINE_EXCEEDED, expected.unknownReason());
                assertEquals(Code.DEADLINE_EXCEEDED, expected.code());
            }
            assertTrue("the deadline did not cut the call short",
                    Duration.ofNanos(System.nanoTime() - started).compareTo(Duration.ofMillis(1_500)) < 0);
            assertEquals("a spent budget was retried", 1, stub.requests.size());
            long sent = Long.parseLong(stub.requests.get(0).header("connect-timeout-ms"));
            assertTrue(sent > 0 && sent <= 200);

            try {
                client.collections().get("notes", CallOptions.withTimeout(Duration.ofMillis(50)));
                fail("the per-call deadline was ignored");
            } catch (DeadlineExceededException expected) {
                assertEquals("50", expected.metadata().get("timeout_ms"));
            }
            assertEquals(Duration.ofSeconds(30), CallInvoker.DEFAULT_TIMEOUT);
        }
    }

    // ---------------------------------------------------------------- R13

    @Test
    public void java_write_batch_chunks() throws Exception {
        List<Doc> docs = new ArrayList<>();
        for (int i = 0; i < 25; i++) {
            docs.add(new Doc((long) i, Map.of("n", i)));
        }
        try (AppStub stub = new AppStub(
                AppStub.ok(WriteDocumentsResponse.newBuilder().setToken("v1:s1/p0@10").build()),
                AppStub.refusal(503, "unavailable", "restarting"),
                AppStub.ok(WriteDocumentsResponse.newBuilder().setToken("v1:s1/p1@4").build()),
                AppStub.ok(WriteDocumentsResponse.newBuilder().setToken("v1:s1/p0@25").build()))) {
            Client client = stub.client("app", false);
            List<dev.loams.gen.loams.collection.v1.WriteOp> ops = new ArrayList<>();
            for (Doc d : docs) {
                ops.add(d.toOp());
            }
            WriteBatch.Result result = client.documents().writeBatch("notes", ops, 10, WriteBatch.DEFAULT_CHUNK_BYTES);
            assertEquals(3, result.chunks());
            assertEquals(25, result.ops());
            assertEquals("v1:s1/p0@25,s1/p1@4", result.token());
            List<WriteDocumentsRequest> bodies = new ArrayList<>();
            for (AppStub.Request r : stub.requests) {
                bodies.add(WriteDocumentsRequest.parseFrom(r.body()));
            }
            assertEquals(List.of(10, 10, 10, 5), bodies.stream().map(WriteDocumentsRequest::getOpsCount).toList());
            assertEquals("the retried chunk changed its key", bodies.get(1).getIdempotencyKey(), bodies.get(2).getIdempotencyKey());
            assertEquals(3, bodies.stream().map(WriteDocumentsRequest::getIdempotencyKey).distinct().count());
        }
        int chunks = 0;
        for (var ignored : WriteBatch.chunks(docs.stream().map(Doc::toOp).toList(), 10_000, 1)) {
            chunks++;
        }
        assertEquals("bytes did not cut a chunk", 25, chunks);

        try (AppStub stub = new AppStub(
                AppStub.ok(WriteDocumentsResponse.newBuilder().setToken("v1:s1/p0@10").build()),
                AppStub.refusal(400, "invalid_argument", "bad doc"))) {
            Client client = stub.client("app", false);
            List<dev.loams.gen.loams.collection.v1.WriteOp> ops = docs.stream().map(Doc::toOp).toList();
            try {
                client.documents().writeBatch("notes", ops, 10, WriteBatch.DEFAULT_CHUNK_BYTES);
                fail("a failed chunk did not stop the batch");
            } catch (WriteBatch.BatchException expected) {
                assertEquals(1, expected.appliedChunks());
                assertEquals(10, expected.appliedOps());
                assertEquals(WriteBatch.BATCH_PARTIALLY_APPLIED, expected.unknownReason());
                assertEquals("v1:s1/p0@10", expected.token());
            }
            assertEquals(2, stub.requests.size());
        }
    }

    // ---------------------------------------------------------------- R14

    @Test
    public void java_streams_subscribe_resumes() {
        Deque<List<Subscription.Record>> pages = new ArrayDeque<>(List.of(
                List.of(record(0), record(1)), List.of(), List.of(), List.of(record(2), record(3))));
        List<Long> asked = new ArrayList<>();
        List<Duration> waits = new ArrayList<>();
        Subscription subscription = new Subscription(offset -> {
            asked.add(offset);
            return pages.isEmpty() ? List.of() : pages.poll();
        }, 0, waits::add);
        List<Long> seen = new ArrayList<>();
        while (subscription.hasNext()) {
            seen.add(subscription.next().offset());
            if (seen.size() == 3) {
                subscription.close();
            }
        }
        assertEquals(List.of(0L, 1L, 2L), seen);
        assertEquals(3, subscription.offset());
        assertEquals(List.of(0L, 2L, 2L, 2L), asked);
        assertEquals(List.of(Duration.ofMillis(100), Duration.ofMillis(200)), waits);

        Subscription resumed = new Subscription(offset -> offset == 3 ? List.of(record(3)) : List.of(),
                subscription.offset(), wait -> {});
        assertEquals(3, resumed.next().offset());
        assertEquals(4, resumed.offset());
        resumed.close();
        assertFalse(resumed.hasNext());
    }

    private static Subscription.Record record(long offset) {
        return new Subscription.Record(0, offset, ("{\"n\":" + offset + "}").getBytes(StandardCharsets.UTF_8), 0);
    }

    // ---------------------------------------------------------------- builders

    private static final float[] EMBEDDING = {0.1f, 0.2f, 0.3f, 0.4f};
    private static final float[] UNIT = {1f, 0f, 0f, 0f};

    private static final String TEXT_REFUND = "{\"text\":{\"query\":{\"match\":{\"field\":\"body\",\"text\":\"refund\"}}}}";

    private static String hybrid(String fusion) {
        return "{\"collection\":\"notes\",\"retrievers\":[" + TEXT_REFUND + ","
                + "{\"vector\":{\"field\":\"embedding\",\"query\":[1,0,0,0],\"k\":5}}],\"fusion\":" + fusion
                + ",\"limit\":3}";
    }

    private static Search hybridSearch() {
        return Search.on("notes").text("refund", "body").vector("embedding", UNIT, 5, null, null).limit(3);
    }

    /** The builder cases, by the corpus's names (P0d's {@code sdks/fixtures/builders}). */
    static Map<String, Map.Entry<Message, String>> cases() {
        Map<String, Map.Entry<Message, String>> cases = new LinkedHashMap<>();
        cases.put("schema_notes", Map.entry(
                new Schema().text("title").text("body").keyword("tags").i64("n").vector("embedding", 4).build(),
                "{\"fields\":["
                        + "{\"name\":\"title\",\"source_path\":\"title\",\"kind\":{\"text\":{\"analyzer\":\"standard\",\"positions\":true}},\"indexed\":true,\"fast\":false},"
                        + "{\"name\":\"body\",\"source_path\":\"body\",\"kind\":{\"text\":{\"analyzer\":\"standard\",\"positions\":true}},\"indexed\":true,\"fast\":false},"
                        + "{\"name\":\"tags\",\"source_path\":\"tags\",\"kind\":\"keyword\",\"indexed\":true,\"fast\":true},"
                        + "{\"name\":\"n\",\"source_path\":\"n\",\"kind\":\"i64\",\"indexed\":true,\"fast\":true}],"
                        + "\"vectors\":[{\"name\":\"embedding\",\"dim\":4,\"distance\":\"cosine\"}],"
                        + "\"sparse_vectors\":[],\"dynamic\":\"ignore\",\"max_fields\":1000}"));
        cases.put("filter_term", Map.entry(Filter.eq("tags", "policy"), "{\"term\":{\"field\":\"tags\",\"value\":\"policy\"}}"));
        cases.put("filter_bool_nested", Map.entry(
                Filter.and(Filter.eq("tags", "policy"), Filter.or(Filter.gte("n", 2), Filter.not(Filter.exists("archived")))),
                "{\"bool\":{\"must\":[{\"term\":{\"field\":\"tags\",\"value\":\"policy\"}},"
                        + "{\"bool\":{\"should\":[{\"range\":{\"field\":\"n\",\"gte\":2}},{\"bool\":{\"mustNot\":[{\"exists\":{\"field\":\"archived\"}}]}}]}}]}}"));
        cases.put("filter_range_open", Map.entry(Filter.gt("n", 1), "{\"range\":{\"field\":\"n\",\"gt\":1}}"));
        cases.put("filter_ids", Map.entry(Filter.ids(1, "k-str", java.util.UUID.fromString("0190f5c4-6c1e-7b3a-9d2e-4f5a6b7c8d9e")),
                "{\"ids\":[{\"uint\":\"1\"},\"k-str\",{\"uuid\":\"0190f5c4-6c1e-7b3a-9d2e-4f5a6b7c8d9e\"}]}"));
        cases.put("search_text_filter_highlight_select", Map.entry(
                Search.on("notes").text("refund", "body").filter(Filter.eq("tags", "policy")).highlight("body")
                        .select(List.of("title"), null).sort(Sort.score(), Sort.pk()).limit(2).build(),
                "{\"collection\":\"notes\",\"retrievers\":[" + TEXT_REFUND + "],"
                        + "\"filter\":{\"term\":{\"field\":\"tags\",\"value\":\"policy\"}},"
                        + "\"sort\":[{\"score\":{}},{\"pk\":{}}],"
                        + "\"limit\":2,\"select\":{\"source\":{\"include\":[\"title\"]}},"
                        + "\"highlight\":{\"fields\":[{\"field\":\"body\"}]}}"));
        cases.put("search_vector_ann_params", Map.entry(
                Search.on("notes").vector("embedding", UNIT, 3, AnnParams.newBuilder().setNprobes(8).setRefineFactor(2).build(), null).limit(3).build(),
                "{\"collection\":\"notes\",\"retrievers\":[{\"vector\":{\"field\":\"embedding\",\"query\":[1,0,0,0],\"k\":3,\"params\":{\"nprobes\":8,\"refineFactor\":2}}}],\"limit\":3}"));
        cases.put("search_hybrid_rrf", Map.entry(hybridSearch().fuse(Fusion.rrf(60)).build(), hybrid("{\"rrf\":{\"k\":60}}")));
        cases.put("search_hybrid_dbsf", Map.entry(hybridSearch().fuse(Fusion.dbsf()).build(), hybrid("{\"dbsf\":{}}")));
        cases.put("search_hybrid_weighted", Map.entry(hybridSearch().fuse(Fusion.weighted(0.7f, 0.3f)).build(),
                hybrid("{\"weightedSum\":{\"weights\":[0.7,0.3]}}")));
        cases.put("search_after_page", Map.entry(
                Search.on("notes").text("refund", "body").filter(Filter.eq("tags", "policy")).highlight("body")
                        .select(List.of("title"), null).limit(2).sort(Sort.score(), Sort.pk())
                        .after(List.of(Value.newBuilder().setNumberValue(0.3438858389854431).build(), Value.newBuilder().setNumberValue(3).build()))
                        .build(),
                "{\"collection\":\"notes\",\"retrievers\":[" + TEXT_REFUND + "],"
                        + "\"filter\":{\"term\":{\"field\":\"tags\",\"value\":\"policy\"}},"
                        + "\"highlight\":{\"fields\":[{\"field\":\"body\"}]},"
                        + "\"select\":{\"source\":{\"include\":[\"title\"]}},\"limit\":2,"
                        + "\"sort\":[{\"score\":{}},{\"pk\":{}}],"
                        + "\"searchAfter\":[0.3438858389854431,3]}"));
        cases.put("search_expand", Map.entry(
                Search.on("notes").text("refund", "body").expand("notes_graph", "id").build(),
                "{\"collection\":\"notes\",\"retrievers\":[" + TEXT_REFUND + "],"
                        + "\"expandSpec\":{\"graph\":\"notes_graph\",\"fromField\":\"id\",\"hops\":1}}"));
        return cases;
    }

    private static Message parse(Message like, String json) {
        return ProtoJson.parse(like, json);
    }

    @Test
    public void java_builders_match_corpus() throws Exception {
        for (var entry : cases().entrySet()) {
            Message built = entry.getValue().getKey();
            assertEquals(entry.getKey(), parse(built, entry.getValue().getValue()), built);
        }
        Path dir = Fixtures.repositoryRoot().resolve("sdks/fixtures/builders");
        if (!Files.isDirectory(dir)) {
            return;
        }
        Map<String, Map.Entry<Message, String>> cases = cases();
        try (var files = Files.list(dir)) {
            for (Path file : files.filter(p -> p.toString().endsWith(".json")).toList()) {
                Map<String, Object> root = dev.loams.internal.Json.parseObject(Files.readString(file));
                String name = root.get("name") instanceof String n ? n : file.getFileName().toString().replace(".json", "");
                var mine = cases.get(name);
                if (mine != null) {
                    assertEquals("corpus case " + name, ProtoJson.fromParsed(mine.getKey(), root.get("expect")), mine.getKey());
                }
            }
        }
    }

    @Test
    public void java_filter_ir_matches_the_engine_mapping() throws Exception {
        Value ir = Filter.toIr(Filter.and(Filter.all(), Filter.not(Filter.isNull("x")), Filter.ids(1, "a")));
        assertEquals(
                parse(Value.getDefaultInstance(),
                        "{\"bool\":{\"must\":[\"match_all\",{\"bool\":{\"must_not\":[{\"is_null\":{\"field\":\"x\"}}]}},{\"ids\":[1,\"a\"]}]}}"),
                ir);
    }

    // ---------------------------------------------------------------- the handle and the modules

    @Test
    public void java_collection_handle_delegates() throws Exception {
        try (AppStub stub = new AppStub(AppStub.ok(WriteDocumentsResponse.getDefaultInstance()))) {
            Client client = stub.client("app", false);
            CollectionHandle notes = client.collection("notes", "other");
            notes.upsert(List.of(new Doc(1, Map.of("title", "a"))));
            notes.get(List.of(1));
            notes.delete(List.of(1));
            notes.patch(1, Map.of("title", "b"), List.of("body"));
            notes.count(Filter.eq("tags", "x"));
            notes.search(Search.on("ignored").text("refund", "body"));
            notes.vectorSearch(EMBEDDING, "embedding", 3, null);
            notes.info();
            notes.drop();
            String c = "/loams.collection.v1.";
            assertEquals(List.of(
                    c + "DocumentService/WriteDocuments", c + "DocumentService/GetDocuments",
                    c + "DocumentService/WriteDocuments", c + "DocumentService/WriteDocuments",
                    c + "DocumentService/CountDocuments", c + "QueryService/Search", c + "QueryService/Search",
                    c + "CollectionService/GetCollection", c + "CollectionService/DropCollection"),
                    stub.requests.stream().map(AppStub.Request::path).toList());
            CountDocumentsRequest count = CountDocumentsRequest.parseFrom(stub.requests.get(4).body());
            assertEquals("other", count.getNamespace());
            assertEquals("notes", count.getCollection());
            assertEquals("x", count.getFilter().getStructValue().getFieldsOrThrow("term").getStructValue()
                    .getFieldsOrThrow("value").getStringValue());
            SearchRequest search = SearchRequest.parseFrom(stub.requests.get(5).body());
            assertEquals("notes", search.getCollection());
            assertEquals("other", search.getNamespace());
            var patch = WriteDocumentsRequest.parseFrom(stub.requests.get(3).body()).getOps(0).getPatch();
            assertEquals("b", patch.getSource().getFieldsOrThrow("title").getStringValue());
            assertEquals(List.of("body"), patch.getDeleteKeysList());

            client.collection("notes").info();
            assertEquals("app", GetCollectionRequest.parseFrom(stub.requests.get(stub.requests.size() - 1).body()).getNamespace());
        }
    }

    @Test
    public void java_search_query_all_requires_tiebreak() throws Exception {
        try (AppStub stub = new AppStub(AppStub.ok(SearchResponse.getDefaultInstance()))) {
            Client client = stub.client("app", false);
            try {
                client.search().queryAll(Search.on("notes").text("refund", "body").sort(Sort.score()));
                fail("query_all ran without a tiebreak");
            } catch (InvalidArgumentException expected) {
                assertEquals("sort", expected.metadata().get("field"));
            }
            assertEquals(0, stub.requests.size());
        }
        Hit one = Hit.newBuilder().setPk(Values.id(1)).addSortValues(num(2)).addSortValues(num(1)).build();
        Hit two = Hit.newBuilder().setPk(Values.id(2)).addSortValues(num(1)).addSortValues(num(2)).build();
        Hit three = Hit.newBuilder().setPk(Values.id(3)).addSortValues(num(0.5)).addSortValues(num(3)).build();
        try (AppStub stub = new AppStub(
                AppStub.ok(SearchResponse.newBuilder().addHits(one).addHits(two).build()),
                AppStub.ok(SearchResponse.newBuilder().addHits(three).build()))) {
            Client client = stub.client("app", false);
            List<Long> ids = new ArrayList<>();
            for (Hit hit : client.search().queryAll(Search.on("notes").text("refund", "body").sort(Sort.score(), Sort.pk()).limit(2))) {
                ids.add(hit.getPk().getUint());
            }
            assertEquals(List.of(1L, 2L, 3L), ids);
            assertEquals(2, stub.requests.size());
            assertEquals(List.of(num(1), num(2)), SearchRequest.parseFrom(stub.requests.get(1).body()).getSearchAfterList());
        }
    }

    private static Value num(double d) {
        return Value.newBuilder().setNumberValue(d).build();
    }

    @Test
    public void java_app_bindings() {
        Map<String, RetryClass> want = new LinkedHashMap<>();
        for (String safe : List.of("namespaces.Create", "namespaces.List", "collections.Create", "collections.List",
                "collections.Get", "collections.Drop", "collections.AddFields", "collections.ListVersions",
                "collections.UpdateAliases", "collections.SetHot", "collections.Warm", "documents.Get",
                "documents.Scroll", "documents.Count", "search.Query", "vector.Search", "graph.Get", "graph.List",
                "graph.Schema", "graph.Explain")) {
            want.put(safe, RetryClass.SAFE);
        }
        for (String manual : List.of("collections.Scan", "documents.Write", "documents.DeleteByFilter",
                "documents.PatchByFilter", "graph.Create", "graph.Update", "graph.Delete", "graph.Query",
                "graph.QueryBatch", "graph.QueryStream")) {
            want.put(manual, RetryClass.MANUAL);
        }
        want.forEach((key, retry) -> {
            String[] parts = key.split("\\.");
            assertEquals(key, retry, Facade.binding(parts[0], parts[1]).orElseThrow().retry());
        });
        CallBinding write = Facade.binding("documents", "Write").orElseThrow();
        assertEquals("loams.collection.v1.DocumentService/WriteDocuments", write.rpc());
        assertTrue(write.takesIdempotencyKey());
        assertEquals(Facade.binding("search", "Query").orElseThrow().rpc(), Facade.binding("vector", "Search").orElseThrow().rpc());
        assertEquals("namespaces", Facade.binding("namespaces", "List").orElseThrow().pagination().itemsField());
        assertEquals(Streaming.SERVER, Facade.binding("graph", "QueryStream").orElseThrow().streaming());
        assertTrue(Facade.module("graph").orElseThrow().unstable());
        Client client = Client.of(Options.builder().endpoint("http://127.0.0.1:1").build());
        for (String module : List.of("namespaces", "collections", "documents", "search", "vector", "graph")) {
            assertTrue(module, client.modules().containsKey(module));
        }
    }

    @Test
    public void java_graph_read_only_query_is_unkeyed_and_retried() throws Exception {
        ExecuteResponse rows = ExecuteResponse.newBuilder().setRows(RowSet.newBuilder().addColumns("n")).build();
        try (AppStub stub = new AppStub(AppStub.refusal(503, "unavailable", "restarting"), AppStub.ok(rows))) {
            Client client = stub.client("app", false);
            ExecuteResponse answer = client.graph().query("g", "MATCH (n) RETURN n", null, true);
            assertEquals(List.of("n"), answer.getRows().getColumnsList());
            assertEquals(2, stub.requests.size());
            ExecuteRequest sent = ExecuteRequest.parseFrom(stub.requests.get(0).body());
            assertEquals("", sent.getIdempotencyKey());
            assertEquals("app", sent.getNamespace());
        }
        try (AppStub stub = new AppStub(AppStub.ok(ExecuteResponse.getDefaultInstance()))) {
            Client client = stub.client("app", false);
            client.graph().query("g", "INSERT (:Note {id: $id})", Map.of("id", 1L), false);
            ExecuteRequest sent = ExecuteRequest.parseFrom(stub.requests.get(0).body());
            assertNotEquals("", sent.getIdempotencyKey());
            assertEquals(1L, sent.getParametersOrThrow("id").getInt64());
        }
    }
}
