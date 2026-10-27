package dev.loams

import com.google.protobuf.ByteString
import com.google.protobuf.DynamicMessage
import dev.loams.Json.JsonArray
import dev.loams.Json.JsonNumber

// The `sql` and `streams` modules (SDKP §2.5, Part C): typed wrappers over the descriptor-driven
// calls of `loams.sql.v1` and `loams.stream.v1` (SDKP P0b and P0c).

private const val SQL = "loams.sql.v1.SqlService"
private const val STREAM = "loams.stream.v1.StreamService"

/** `loams.sql`: one read-only statement over a namespace's collections, with `$n` parameters. */
class SqlModule internal constructor(private val client: LoamsClient) {
    /**
     * Runs [sql] with [params] bound to `$1…$n` (a count mismatch is `invalid_argument`, reason
     * `sql_param_count`) and answers its columns and rows.
     */
    fun query(sql: String, params: List<Any?> = emptyList(), maxRows: Int? = null, consistency: JsonValue? = null,
              namespace: String? = null, options: CallOptions? = null): DynamicMessage =
        client.invoke("$SQL/Query", Messages.of("loams.sql.v1.QueryRequest", Values.obj(
            "sql" to Values.str(sql),
            "params" to if (params.isEmpty()) null else JsonArray(params.map { Values.of(it) }),
            "maxRows" to maxRows?.let { JsonNumber(it.toString()) },
            "consistency" to consistency,
            "namespace" to namespace?.let { Values.str(it) },
        )), options)

    /**
     * Runs one statement and answers its result as one Arrow IPC stream (`arrow_ipc`); this SDK
     * carries no Arrow reader, so the bytes are the caller's to read.
     */
    fun queryArrow(sql: String, params: List<Any?> = emptyList(), options: CallOptions? = null): ByteArray {
        val answer = client.invoke("$SQL/QueryArrow", Messages.of("loams.sql.v1.QueryArrowRequest", Values.obj(
            "sql" to Values.str(sql),
            "params" to if (params.isEmpty()) null else JsonArray(params.map { Values.of(it) }),
        )), options)
        return (answer.field("arrow_ipc") as ByteString).toByteArray()
    }

    companion object {
        /** Each row of an answer as a map of column name to plain value. */
        fun records(response: DynamicMessage): List<Map<String, Any?>> {
            val names = response.messages("columns").map { it.text("name") }
            return response.messages("rows").map { row ->
                val values = row.messages("values")
                names.mapIndexed { i, name -> name to values.getOrNull(i)?.let { plain(it) } }.toMap()
            }
        }

        /** A `google.protobuf.Value` as a plain value: null, Boolean, Double, String, List or Map. */
        fun plain(value: DynamicMessage): Any? {
            val (field, v) = value.allFields.entries.firstOrNull() ?: return null
            return when (field.name) {
                "bool_value" -> v as Boolean
                "number_value" -> v as Double
                "string_value" -> v as String
                "list_value" -> (v as DynamicMessage).messages("values").map { plain(it) }
                "struct_value" -> (v as DynamicMessage).messages("fields").associate { entry ->
                    entry.text("key") to plain(entry.field("value") as DynamicMessage)
                }
                else -> null
            }
        }
    }
}

/** `loams.streams`: a namespace's logs: create, list, produce, fetch and subscribe. */
class StreamsModule internal constructor(private val client: LoamsClient) {
    private fun call(method: String, type: String, json: JsonValue, options: CallOptions?): DynamicMessage =
        client.invoke("$STREAM/$method", Messages.of("loams.stream.v1.$type", json), options)

    /** Creates a stream. Idempotent by name and partition count, so retried. */
    fun create(name: String, partitions: Int = 1, namespace: String? = null, options: CallOptions? = null): DynamicMessage =
        call("CreateStream", "CreateStreamRequest", Values.obj(
            "name" to Values.str(name),
            "partitions" to JsonNumber(partitions.toString()),
            "namespace" to namespace?.let { Values.str(it) },
        ), (options ?: CallOptions.NONE).let { it.copy(retrySafe = it.retrySafe ?: true) })

    /** One stream, with each partition's offsets. */
    fun get(stream: String, namespace: String? = null, options: CallOptions? = null): DynamicMessage =
        call("DescribeStream", "DescribeStreamRequest",
            Values.obj("stream" to Values.str(stream), "namespace" to namespace?.let { Values.str(it) }), options)

    /** Every stream in the namespace, following the page tokens. */
    fun listAll(pageSize: Int = 0, options: CallOptions? = null): Sequence<DynamicMessage> = pages("streams") { token ->
        call("ListStreams", "ListStreamsRequest",
            Values.obj("pageSize" to JsonNumber(pageSize.toString()), "pageToken" to Values.str(token)), options)
    }

    /**
     * Appends records to one partition. Each record is bytes, a string (UTF-8) or a value (as JSON),
     * or a [ProducerRecord] to give it a key and headers.
     * With [producerId] and [producerSeq] a repeat answers the first append with `duplicate` set, so
     * the SDK retries it; without them a produce is a plain append and is never retried.
     */
    fun produce(stream: String, partition: Int, records: List<Any>, producerId: String? = null, producerSeq: Long? = null,
                namespace: String? = null, options: CallOptions? = null): DynamicMessage {
        val keyed = producerId != null && producerSeq != null
        return call("Produce", "ProduceRequest", Values.obj(
            "stream" to Values.str(stream),
            "partition" to JsonNumber(partition.toString()),
            "records" to JsonArray(records.map(::recordJson)),
            "producerId" to producerId?.let { Values.str(it) },
            "producerSeq" to producerSeq?.let { Values.str(it.toString()) },
            "namespace" to namespace?.let { Values.str(it) },
        ), if (keyed) (options ?: CallOptions.NONE).let { it.copy(retrySafe = it.retrySafe ?: true) } else options)
    }

    /**
     * Reads one partition from [offset]; [maxWaitMs] holds an empty fetch until a record arrives (a
     * long poll, at most 30 000).
     */
    fun fetch(stream: String, partition: Int, offset: Long, maxRecords: Int? = null, maxWaitMs: Int? = null,
              namespace: String? = null, options: CallOptions? = null): DynamicMessage {
        var settings = options
        if (maxWaitMs != null && options?.timeout == null) {
            settings = (options ?: CallOptions.NONE).copy(timeout = CallInvoker.DEFAULT_TIMEOUT.plusMillis(maxWaitMs.toLong()))
        }
        return call("Fetch", "FetchRequest", Values.obj(
            "stream" to Values.str(stream),
            "partition" to JsonNumber(partition.toString()),
            "offset" to Values.str(offset.toString()),
            "maxRecords" to maxRecords?.let { JsonNumber(it.toString()) },
            "maxWaitMs" to maxWaitMs?.let { JsonNumber(it.toString()) },
            "namespace" to namespace?.let { Values.str(it) },
        ), settings)
    }

    /** A resumable subscription to one partition (R14); each empty fetch long-polls for [maxWaitMs]. */
    fun subscribe(stream: String, partition: Int, fromOffset: Long = 0, maxWaitMs: Int = 1000, namespace: String? = null,
                  options: CallOptions? = null): Subscription = Subscription(fromOffset) { offset ->
        fetch(stream, partition, offset, maxWaitMs = maxWaitMs, namespace = namespace, options = options)
            .messages("records").map { r ->
                val key = r.descriptorForType.findFieldByName("key")
                Subscription.Record(partition, r.field("offset") as Long,
                    (r.field("value") as ByteString).toByteArray(), r.field("timestamp_ms") as Long,
                    if (r.hasField(key)) (r.getField(key) as ByteString).toByteArray() else null)
            }
    }

    /** Appends CloudEvents (a `ProduceCloudEventsRequest` as JSON). */
    fun produceEvents(request: JsonValue, options: CallOptions? = null): DynamicMessage =
        call("ProduceCloudEvents", "ProduceCloudEventsRequest", request, options)

    /** Reads one partition's records as CloudEvents (a `FetchCloudEventsRequest` as JSON). */
    fun fetchEvents(request: JsonValue, options: CallOptions? = null): DynamicMessage =
        call("FetchCloudEvents", "FetchCloudEventsRequest", request, options)

    companion object {
        /**
         * A record's bytes: as given, a string's UTF-8, or a value's JSON with object keys sorted, so the
         * same value is the same bytes however its map was built.
         */
        fun encode(record: Any): ByteArray = when (record) {
            is ByteArray -> record
            is ByteString -> record.toByteArray()
            is String -> record.toByteArray(Charsets.UTF_8)
            else -> Json.writeCompact(sorted(Values.of(record))).toByteArray(Charsets.UTF_8)
        }

        private fun sorted(value: JsonValue): JsonValue = when (value) {
            is Json.JsonObject -> Json.JsonObject(value.entries.toSortedMap().mapValues { sorted(it.value) })
            is JsonArray -> JsonArray(value.items.map(::sorted))
            else -> value
        }

        private fun base64(bytes: ByteArray): JsonValue = Values.str(java.util.Base64.getEncoder().encodeToString(bytes))

        /** One `loams.stream.v1.Record` as proto3 JSON: a [ProducerRecord] with its key and headers, or a bare value. */
        internal fun recordJson(record: Any): JsonValue {
            if (record !is ProducerRecord) return Values.obj("value" to base64(encode(record)))
            return Values.obj(
                "key" to record.key?.let { base64(encode(it)) },
                "value" to base64(encode(record.value)),
                "headers" to if (record.headers.isEmpty()) null else JsonArray(record.headers.map { (name, value) ->
                    Values.obj("key" to Values.str(name), "value" to base64(encode(value)))
                }),
            )
        }
    }
}

/**
 * A record to produce with a key and headers (SDKP-25). The key (partitioning, compaction), the value
 * and each header value are bytes, a string (UTF-8) or a value (as JSON), as [StreamsModule.encode]
 * encodes them; a null key produces an unkeyed record.
 */
data class ProducerRecord(val key: Any?, val value: Any, val headers: List<Pair<String, Any>> = emptyList())
