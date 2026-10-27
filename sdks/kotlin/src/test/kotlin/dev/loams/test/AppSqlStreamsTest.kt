package dev.loams.test

import com.google.protobuf.DynamicMessage
import dev.loams.Descriptors
import dev.loams.Json
import dev.loams.LoamsClient
import dev.loams.LoamsException
import dev.loams.Messages
import dev.loams.ProducerRecord
import dev.loams.SqlModule
import dev.loams.field
import dev.loams.messages
import dev.loams.test.Support.StubTransport

/** SDKP Part C (L9): the `sql` and `streams` modules, against the stub transport. */
object AppSqlStreamsTest {
    private fun eq(expected: Any?, actual: Any?, what: String = "") =
        check(expected == actual) { "$what: expected <$expected> but was <$actual>" }

    private fun ok(stub: StubTransport, type: String, body: String = "{}") =
        stub.answerMessage(Descriptors.message(type)!!, Messages.of(type, Json.parseOrThrow(body)))

    private fun sent(stub: StubTransport, index: Int, type: String): DynamicMessage =
        DynamicMessage.parseFrom(Descriptors.message(type)!!, stub.requests[index].body)

    private fun client(stub: StubTransport) =
        LoamsClient(endpoint = "stub://in-process", transport = stub, maxRetries = 3, namespace = "app")

    fun register() {
        Harness.test("kotlin_sql_params_bind_and_rows_read_as_records") { sqlParams() }
        Harness.test("kotlin_streams_produce_is_retried_only_with_a_producer_key") { produceRetries() }
        Harness.test("kotlin_streams_produce_keyed_records") { produceKeyed() }
    }

    private fun sqlParams() {
        val stub = StubTransport()
        ok(stub, "loams.sql.v1.QueryResponse",
            """{"columns":[{"name":"tags","type":"Utf8"},{"name":"c","type":"Int64"}],"rows":[["a",2]]}""")
        val answer = client(stub).sql.query("SELECT tags, count(*) AS c FROM notes WHERE n > \$1 GROUP BY tags", listOf(1))
        val request = sent(stub, 0, "loams.sql.v1.QueryRequest")
        eq("app", request.field("namespace"))
        eq(1, request.messages("params").size)
        eq(listOf(mapOf("tags" to "a", "c" to 2.0)), SqlModule.records(answer))
    }

    /** Keyed records (SDKP-25) carry the recorded app_streams produce body's keys and values. */
    private fun produceKeyed() {
        val stub = StubTransport()
        ok(stub, "loams.stream.v1.ProduceResponse", """{"lastOffset":"1"}""")
        client(stub).streams.produce("note-events", 0, listOf(
            ProducerRecord("1", mapOf("op" to "created", "note" to 1)),
            ProducerRecord("5", mapOf("op" to "deleted", "note" to 5), listOf("source" to "notes")),
            "plain",
        ), "app-producer-1", 1)
        val records = sent(stub, 0, "loams.stream.v1.ProduceRequest").messages("records")
        val b64 = { b: Any? -> java.util.Base64.getEncoder().encodeToString((b as com.google.protobuf.ByteString).toByteArray()) }
        eq("MQ==", b64(records[0].field("key")))
        eq("eyJub3RlIjoxLCJvcCI6ImNyZWF0ZWQifQ==", b64(records[0].field("value")))
        eq("eyJub3RlIjo1LCJvcCI6ImRlbGV0ZWQifQ==", b64(records[1].field("value")))
        eq("source", records[1].messages("headers")[0].field("key"))
        eq(false, records[2].hasField(records[2].descriptorForType.findFieldByName("key")))
    }

    private fun produceRetries() {
        val unkeyed = StubTransport()
        unkeyed.answerConnectError(503, "unavailable", "restarting")
        try {
            client(unkeyed).streams.produce("events", 0, listOf("a"))
            throw AssertionError("an unkeyed produce succeeded")
        } catch (_: LoamsException) {
        }
        eq(1, unkeyed.requests.size)

        val keyed = StubTransport()
        keyed.answerConnectError(503, "unavailable", "restarting")
        ok(keyed, "loams.stream.v1.ProduceResponse", """{"lastOffset":"2"}""")
        val answer = client(keyed).streams.produce("events", 0, listOf("text", mapOf("n" to 1)), "app-1", 7)
        eq(2L, answer.field("last_offset"))
        eq(2, keyed.requests.size)
        val request = sent(keyed, 0, "loams.stream.v1.ProduceRequest")
        eq("app-1", request.field("producer_id"))
        eq(listOf("text", "{\"n\":1}"), request.messages("records").map {
            (it.field("value") as com.google.protobuf.ByteString).toStringUtf8()
        })
    }
}
