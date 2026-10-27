package dev.loams.test

import com.google.protobuf.DynamicMessage
import dev.loams.CallOptions
import dev.loams.ConsistencySession
import dev.loams.ConsistencyToken
import dev.loams.Consistent
import dev.loams.Deadline
import dev.loams.DeadlineExceededException
import dev.loams.Descriptors
import dev.loams.Doc
import dev.loams.Filter
import dev.loams.InvalidArgumentException
import dev.loams.Json
import dev.loams.JsonValue
import dev.loams.LoamsClient
import dev.loams.Messages
import dev.loams.Schema
import dev.loams.Search
import dev.loams.Sort
import dev.loams.Subscription
import dev.loams.Transport
import dev.loams.TransportRequest
import dev.loams.TransportResponse
import dev.loams.WriteBatch
import dev.loams.BatchException
import dev.loams.dbsf
import dev.loams.field
import dev.loams.messages
import dev.loams.rrf
import dev.loams.search
import dev.loams.text
import dev.loams.weighted
import dev.loams.test.Support.StubTransport
import java.time.Duration

/**
 * SDKP (docs/plans/2026-10-19-sdk-parity.md §5, L9): the runtime additions R4, R11–R14, the
 * Schema, Filter and Search builders, the collection handle and the app modules, against the stub
 * transport.
 */
object AppTest {
    private val MALFORMED = listOf(
        "v2:s1/p0@1", "v1:s1/p0", "v1:s01/p0@1", "v1:s1/p0@1,", "", "v1", "s1/p0@1", "v1:,s1/p0@1",
        "v1:s1/p00@1", "v1:s1/p0@01", "v1:s1/p4294967296@1", "v1:s18446744073709551616/p0@1", "v1:s+1/p0@1",
        "v1:s1/p0@-1", "v1:s1p0@1", "v1:S1/P0@1", "v1: s1/p0@1", "v1:s1/p0@1@2", "v1:s/p0@1",
    )

    private fun eq(expected: Any?, actual: Any?, what: String = "") =
        check(expected == actual) { "$what: expected <$expected> but was <$actual>" }

    private fun json(text: String): JsonValue = Json.parseOrThrow(text)

    private fun ok(stub: StubTransport, type: String, body: String = "{}"): StubTransport =
        stub.answerMessage(Descriptors.message(type)!!, Messages.of(type, json(body)))

    private fun sent(stub: StubTransport, index: Int, type: String): DynamicMessage =
        DynamicMessage.parseFrom(Descriptors.message(type)!!, stub.requests[index].body)

    private fun client(stub: Transport, namespace: String? = "app", session: ConsistencySession? = null,
                       timeout: Duration = Duration.ofSeconds(30)) =
        LoamsClient(endpoint = "stub://in-process", transport = stub, maxRetries = 3, session = session,
            namespace = namespace, timeout = timeout)

    private inline fun <reified T : Throwable> raises(block: () -> Unit): T {
        try {
            block()
        } catch (thrown: Throwable) {
            if (thrown is T) return thrown
            throw AssertionError("expected ${T::class.simpleName}, got $thrown", thrown)
        }
        throw AssertionError("expected ${T::class.simpleName}, nothing was thrown")
    }

    fun register() {
        Harness.test("kotlin_consistency_token_merge") { tokenMerge() }
        Harness.test("kotlin_session_read_your_writes") { sessionReadYourWrites() }
        Harness.test("kotlin_default_namespace") { defaultNamespace() }
        Harness.test("kotlin_timeout_deadline") { timeoutDeadline() }
        Harness.test("kotlin_write_batch_chunks") { writeBatchChunks() }
        Harness.test("kotlin_streams_subscribe_resumes") { subscribeResumes() }
        Harness.test("kotlin_builders_match_corpus") { buildersMatchCorpus() }
        Harness.test("kotlin_filter_ir_matches_the_engine_mapping") { filterIr() }
        Harness.test("kotlin_collection_handle_delegates") { handleDelegates() }
        Harness.test("kotlin_search_query_all_requires_tiebreak") { queryAllRequiresTiebreak() }
        Harness.test("kotlin_graph_read_only_query_is_unkeyed_and_retried") { graphReadOnly() }
    }

    private fun tokenMerge() {
        val one = ConsistencyToken.parse("v1:s7/p3@918274")
        eq(listOf(ConsistencyToken.Position(7uL, 3u, 918274uL)), one.positions)
        eq("v1:s7/p3@918274", one.toString())
        val max = "v1:s18446744073709551615/p4294967295@18446744073709551615"
        eq(max, ConsistencyToken.parse(max).toString())
        eq(ConsistencyToken.EMPTY, ConsistencyToken.parse("v1:"))
        val merged = ConsistencyToken.parse("v1:s7/p3@10,s1/p0@5").merge(ConsistencyToken.parse("v1:s7/p3@4,s7/p1@2,s1/p0@9"))
        eq("v1:s1/p0@9,s7/p1@2,s7/p3@10", merged.toString())
        eq(10uL, merged.offset(7uL, 3u))
        eq(null, merged.offset(1uL, 1u))
        for (bad in MALFORMED) {
            eq(null, ConsistencyToken.tryParse(bad), bad)
            raises<InvalidArgumentException> { ConsistencyToken.parse(bad) }
        }
    }

    private fun sessionReadYourWrites() {
        val stub = StubTransport()
        ok(stub, "loams.collection.v1.WriteDocumentsResponse", """{"token":"v1:s4/p0@7"}""")
        ok(stub, "loams.collection.v1.WriteDocumentsResponse", """{"token":"v1:s4/p1@2,s4/p0@3"}""")
        ok(stub, "loams.collection.v1.GetDocumentsResponse", """{"readToken":"v1:s4/p0@9"}""")
        ok(stub, "loams.collection.v1.GetDocumentsResponse")
        val session = ConsistencySession()
        val loams = client(stub, session = session)
        loams.documents.upsert("notes", listOf(Doc(1, mapOf("title" to "a"))))
        loams.documents.delete("notes", listOf(2))
        eq("v1:s4/p0@7,s4/p1@2", session.token())
        loams.documents.get("notes", listOf(1))
        val get = sent(stub, 2, "loams.collection.v1.GetDocumentsRequest")
        eq("v1:s4/p0@7,s4/p1@2", (get.field("consistency") as DynamicMessage).field("at_least"))
        eq("v1:s4/p0@9,s4/p1@2", session.token())
        loams.documents.get("notes", listOf(1), Consistent.strong())
        val strong = sent(stub, 3, "loams.collection.v1.GetDocumentsRequest")
        eq("", (strong.field("consistency") as DynamicMessage).field("at_least"))
    }

    private fun defaultNamespace() {
        val stub = StubTransport()
        repeat(3) { ok(stub, "loams.collection.v1.CollectionInfo", """{"name":"notes"}""") }
        val loams = client(stub)
        loams.collections.get("notes")
        loams.collections.get("notes", "other")
        loams.collections.get("notes", options = CallOptions(namespace = "per-call"))
        eq(listOf("app", "other", "per-call"), (0..2).map { sent(stub, it, "loams.collection.v1.GetCollectionRequest").field("namespace") })
        val bare = client(StubTransport(), namespace = null)
        val error = raises<InvalidArgumentException> { bare.collections.get("notes") }
        eq("namespace", error.metadata["field"])
    }

    private class Hanging : Transport {
        val seen = mutableListOf<TransportRequest>()
        override fun send(request: TransportRequest): TransportResponse {
            seen.add(request)
            val wait = request.timeout ?: Duration.ofSeconds(30)
            Thread.sleep(wait.toMillis() + 20)
            throw java.net.http.HttpTimeoutException("request timed out")
        }
    }

    private fun timeoutDeadline() {
        val hanging = Hanging()
        val loams = client(hanging, timeout = Duration.ofMillis(200))
        val started = System.nanoTime()
        val error = raises<DeadlineExceededException> { loams.collections.get("notes") }
        eq(Deadline.CLIENT_DEADLINE_EXCEEDED, error.unknownReason)
        check(Duration.ofNanos(System.nanoTime() - started) < Duration.ofSeconds(5)) { "the deadline did not cut the call short" }
        eq(1, hanging.seen.size, "a spent budget is not retried")
        val header = hanging.seen[0].headers["connect-timeout-ms"]!!.toLong()
        check(header in 1..200) { "connect-timeout-ms was $header" }
        val quick = raises<DeadlineExceededException> {
            loams.collections.get("notes", options = CallOptions(timeout = Duration.ofMillis(50)))
        }
        eq("50", quick.metadata["timeout_ms"])
    }

    private fun writeBatchChunks() {
        val docs = (0 until 25).map { Doc(it, mapOf("n" to it)) }
        val stub = StubTransport()
        ok(stub, "loams.collection.v1.WriteDocumentsResponse", """{"token":"v1:s1/p0@10"}""")
        stub.answerConnectError(503, "unavailable", "restarting")
        ok(stub, "loams.collection.v1.WriteDocumentsResponse", """{"token":"v1:s1/p1@4"}""")
        ok(stub, "loams.collection.v1.WriteDocumentsResponse", """{"token":"v1:s1/p0@25"}""")
        val result = client(stub).documents.writeBatch("notes", docs, chunkRows = 10)
        eq(3, result.chunks)
        eq(25, result.ops)
        eq("v1:s1/p0@25,s1/p1@4", result.token)
        val bodies = (0..3).map { sent(stub, it, "loams.collection.v1.WriteDocumentsRequest") }
        eq(listOf(10, 10, 10, 5), bodies.map { it.messages("ops").size })
        val keys = bodies.map { it.field("idempotency_key") as String }
        eq(keys[1], keys[2], "the retried chunk reuses its key")
        eq(3, keys.toSet().size)
        eq(25, WriteBatch.chunks(docs.map { it.op }, 10_000, 1).size, "bytes cut a chunk")

        val failing = StubTransport()
        ok(failing, "loams.collection.v1.WriteDocumentsResponse", """{"token":"v1:s1/p0@10"}""")
        failing.answerConnectError(400, "invalid_argument", "bad doc")
        val error = raises<BatchException> { client(failing).documents.writeBatch("notes", docs, chunkRows = 10) }
        eq(1, error.appliedChunks)
        eq(10, error.appliedOps)
        eq("v1:s1/p0@10", error.token)
        eq(BatchException.BATCH_PARTIALLY_APPLIED, error.unknownReason)
    }

    private fun subscribeResumes() {
        fun record(offset: Long) = Subscription.Record(0, offset, "{\"n\":$offset}".toByteArray())
        val pages = ArrayDeque(listOf(listOf(record(0), record(1)), emptyList(), emptyList(), listOf(record(2), record(3))))
        val asked = mutableListOf<Long>()
        val waits = mutableListOf<Long>()
        val subscription = Subscription(0, sleep = { waits.add(it) }) { offset ->
            asked.add(offset)
            pages.removeFirstOrNull() ?: emptyList()
        }
        val seen = mutableListOf<Long>()
        while (subscription.hasNext()) {
            seen.add(subscription.next().offset)
            if (seen.size == 3) subscription.close()
        }
        eq(listOf(0L, 1L, 2L), seen)
        eq(3L, subscription.offset)
        eq(listOf(0L, 2L, 2L, 2L), asked)
        eq(listOf(100L, 200L), waits)
        val resumed = Subscription(subscription.offset, sleep = {}) { offset -> if (offset == 3L) listOf(record(3)) else emptyList() }
        eq(3L, resumed.next().offset)
        eq(4L, resumed.offset)
    }

    private val embedding = floatArrayOf(0.5f, 0.25f, 0.125f, 1f)
    private const val TEXT_REFUND = """{"text":{"query":{"match":{"field":"body","text":"refund"}}}}"""

    private fun same(type: String, built: JsonValue, expected: String, what: String) =
        eq(Messages.of(type, json(expected)), Messages.of(type, built), what)

    private fun buildersMatchCorpus() {
        same("google.protobuf.Struct", Schema().text("title").text("body").keyword("tags").i64("n").vector("embedding", 4).build(),
            """{"fields":[
              {"name":"title","source_path":"title","kind":{"text":{"analyzer":"standard","positions":true}},"indexed":true,"fast":false},
              {"name":"body","source_path":"body","kind":{"text":{"analyzer":"standard","positions":true}},"indexed":true,"fast":false},
              {"name":"tags","source_path":"tags","kind":"keyword","indexed":true,"fast":true},
              {"name":"n","source_path":"n","kind":"i64","indexed":true,"fast":true}],
             "vectors":[{"name":"embedding","dim":4,"distance":"cosine"}],"sparse_vectors":[],"dynamic":"ignore","max_fields":1000}""",
            "schema_notes")
        val q = "loams.collection.v1.Query"
        same(q, Filter.eq("tags", "policy").json, """{"term":{"field":"tags","value":"policy"}}""", "filter_term")
        same(q, (Filter.eq("tags", "policy") and (Filter.range("n", gte = 2) or !Filter.exists("archived"))).json,
            """{"bool":{"must":[{"term":{"field":"tags","value":"policy"}},
              {"bool":{"should":[{"range":{"field":"n","gte":2}},{"bool":{"mustNot":[{"exists":{"field":"archived"}}]}}]}}]}}""",
            "filter_bool_nested")
        same(q, Filter.range("n", gt = 1).json, """{"range":{"field":"n","gt":1}}""", "filter_range_open")
        same(q, Filter.ids(1, "k-str").json, """{"ids":[1,"k-str"]}""", "filter_ids")
        val s = "loams.collection.v1.SearchRequest"
        same(s, search("notes") {
            text("refund", "body"); filter(Filter.eq("tags", "policy")); highlight("body"); select(include = listOf("title"))
            sort(Sort.score(), Sort.pk()); limit(2)
        }.json(), """{"collection":"notes","retrievers":[$TEXT_REFUND],"filter":{"term":{"field":"tags","value":"policy"}},
            "sort":[{"score":{}},{"pk":{}}],"limit":2,
            "select":{"source":{"include":["title"]}},"highlight":{"fields":[{"field":"body"}]}}""",
            "search_text_filter_highlight_select")
        same(s, search("notes") { vector("embedding", embedding, k = 3, params = json("""{"ef":64}""")); limit(3) }.json(),
            """{"collection":"notes","retrievers":[{"vector":{"field":"embedding","query":[0.5,0.25,0.125,1],"k":3,"params":{"ef":64}}}],"limit":3}""",
            "search_vector_ann_params")
        fun hybrid(fusion: JsonValue) = search("notes") { text("refund", "body"); vector("embedding", embedding); limit(3); fuse(fusion) }.json()
        fun hybridJson(fusion: String) = """{"collection":"notes","retrievers":[$TEXT_REFUND,
            {"vector":{"field":"embedding","query":[0.5,0.25,0.125,1]}}],"fusion":$fusion,"limit":3}"""
        same(s, hybrid(rrf(60)), hybridJson("""{"rrf":{"k":60}}"""), "search_hybrid_rrf")
        same(s, hybrid(dbsf()), hybridJson("""{"dbsf":{}}"""), "search_hybrid_dbsf")
        same(s, hybrid(weighted(0.7, 0.3)), hybridJson("""{"weightedSum":{"weights":[0.7,0.3]}}"""), "search_hybrid_weighted")
        same(s, search("notes") { text("refund", "body"); sort(Sort.score(), Sort.pk()); after(listOf(json("1.5"), json("3"))); limit(2) }.json(),
            """{"collection":"notes","retrievers":[$TEXT_REFUND],"sort":[{"score":{}},{"pk":{}}],
            "searchAfter":[1.5,3],"limit":2}""", "search_after_page")
        same(s, search("notes") { text("refund", "body"); expand("notes_graph", "id") }.json(),
            """{"collection":"notes","retrievers":[$TEXT_REFUND],"expandSpec":{"graph":"notes_graph","fromField":"id","hops":1}}""",
            "search_expand")
    }

    private fun filterIr() {
        val ir = Filter.ir(Filter.and(Filter.all(), Filter.not(Filter.isNull("x")), Filter.ids(1, "a")))
        eq(json("""{"bool":{"must":["match_all",{"bool":{"must_not":[{"is_null":{"field":"x"}}]}},{"ids":[1,"a"]}]}}"""), ir)
    }

    private fun handleDelegates() {
        val stub = StubTransport()
        val c = "loams.collection.v1."
        ok(stub, c + "WriteDocumentsResponse")
        ok(stub, c + "GetDocumentsResponse")
        ok(stub, c + "WriteDocumentsResponse")
        ok(stub, c + "WriteDocumentsResponse")
        ok(stub, c + "CountDocumentsResponse", """{"count":"3"}""")
        ok(stub, c + "SearchResponse")
        ok(stub, c + "SearchResponse")
        ok(stub, c + "CollectionInfo")
        ok(stub, c + "DropCollectionResponse")
        ok(stub, c + "CollectionInfo")
        val loams = client(stub)
        val notes = loams.collection("notes", "other")
        notes.upsert(listOf(Doc(1, mapOf("title" to "a"))))
        notes.get(listOf(1))
        notes.delete(listOf(1))
        notes.patch(1, mapOf("title" to "b"), listOf("body"))
        eq(3L, notes.count(Filter.eq("tags", "x")))
        notes.search { text("refund", "body") }
        notes.vectorSearch(embedding, "embedding")
        notes.info()
        notes.drop()
        eq(listOf(
            "DocumentService/WriteDocuments", "DocumentService/GetDocuments", "DocumentService/WriteDocuments",
            "DocumentService/WriteDocuments", "DocumentService/CountDocuments", "QueryService/Search", "QueryService/Search",
            "CollectionService/GetCollection", "CollectionService/DropCollection",
        ).map { "loams.collection.v1.$it" }, stub.requests.map { it.rpc })
        val count = sent(stub, 4, c + "CountDocumentsRequest")
        eq("other", count.field("namespace"))
        eq("notes", count.field("collection"))
        val search = sent(stub, 5, c + "SearchRequest")
        eq("other", search.field("namespace"))
        loams.collection("notes").info()
        eq("app", sent(stub, 9, c + "GetCollectionRequest").field("namespace"))
    }

    private fun queryAllRequiresTiebreak() {
        val stub = StubTransport()
        val loams = client(stub)
        val error = raises<InvalidArgumentException> { loams.search.queryAll(search("notes") { text("refund"); sort(Sort.score()) }) }
        eq("sort", error.metadata["field"])
        eq(0, stub.requests.size)
        val r = "loams.collection.v1.SearchResponse"
        ok(stub, r, """{"hits":[{"pk":{"uint":"1"},"sortValues":[2,1]},{"pk":{"uint":"2"},"sortValues":[1,2]}]}""")
        ok(stub, r, """{"hits":[{"pk":{"uint":"3"},"sortValues":[0.5,3]}]}""")
        val ids = loams.search.queryAll(search("notes") { text("refund"); sort(Sort.score(), Sort.pk()); limit(2) })
            .map { dev.loams.DocumentId.of(it.field("pk") as DynamicMessage)!!.text }.toList()
        eq(listOf("1", "2", "3"), ids)
        eq(2, stub.requests.size)
        val second = sent(stub, 1, "loams.collection.v1.SearchRequest")
        eq(2, (second.field("search_after") as List<*>).size)
    }

    private fun graphReadOnly() {
        val stub = StubTransport()
        stub.answerConnectError(503, "unavailable", "restarting")
        ok(stub, "loams.graph.v1.ExecuteResponse", """{"rows":{"columns":["n"]}}""")
        val loams = client(stub)
        loams.graph.query("g", "MATCH (n) RETURN n", readOnly = true)
        eq(2, stub.requests.size)
        val first = sent(stub, 0, "loams.graph.v1.ExecuteRequest")
        eq("", first.field("idempotency_key"))
        eq("app", first.field("namespace"))

        val write = StubTransport()
        ok(write, "loams.graph.v1.ExecuteResponse")
        client(write).graph.query("g", "INSERT (:Note {id: \$id})", mapOf("id" to dev.loams.GraphModule.Param.int(1)))
        check((sent(write, 0, "loams.graph.v1.ExecuteRequest").field("idempotency_key") as String).isNotEmpty()) { "a write was not keyed" }
    }
}
