package dev.loams

import com.google.protobuf.DynamicMessage
import dev.loams.Json.JsonArray
import dev.loams.Json.JsonBoolean
import dev.loams.Json.JsonNumber
import dev.loams.Json.JsonObject

// The app-building modules (SDKP §2.2–§2.5): typed wrappers over the descriptor-driven calls of
// `loams.collection.v1` and `loams.graph.v1`. The RPCs come from the committed descriptor set; the
// wrappers add the shapes a caller writes. Until SDKP P0a annotates these packages, the retry
// classes the plan gives as `retry_safe` are applied here, per call, and are checked against
// `sdks/fixtures/facade-index.json` by `kotlin_facade_matches_index` once P0a lands.

private const val NS = "loams.collection.v1.NamespaceService"
private const val COL = "loams.collection.v1.CollectionService"
private const val DOC = "loams.collection.v1.DocumentService"
private const val QUERY = "loams.collection.v1.QueryService"
private const val GRAPH_ADMIN = "loams.graph.v1.GraphAdminService"
private const val GRAPH = "loams.graph.v1.GraphService"

private val RETRY_SAFE = CallOptions(retrySafe = true)

private fun merged(options: CallOptions?, base: CallOptions): CallOptions {
    val o = options ?: return base
    return o.copy(
        retrySafe = o.retrySafe ?: base.retrySafe,
        mintIdempotencyKey = o.mintIdempotencyKey ?: base.mintIdempotencyKey,
    )
}

/** A field of a response message, by its proto name. */
fun DynamicMessage.field(name: String): Any? {
    val field = descriptorForType.findFieldByName(name) ?: return null
    return getField(field)
}

/** A string field of a response message. */
fun DynamicMessage.text(name: String): String = field(name) as? String ?: ""

/** A repeated message field of a response message. */
@Suppress("UNCHECKED_CAST")
fun DynamicMessage.messages(name: String): List<DynamicMessage> = field(name) as? List<DynamicMessage> ?: emptyList()

/** One document to upsert: an id, its source and its dense vectors. */
data class Doc(val id: Any, val source: Map<String, Any?>, val vectors: Map<String, FloatArray> = emptyMap()) {
    /** The write op that upserts this document. */
    val op: JsonValue
        get() = Values.obj(
            "upsert" to Values.obj(
                "id" to DocumentId.of(id).json,
                "source" to Values.of(source),
                "vectors" to if (vectors.isEmpty()) null else JsonObject(vectors.mapValues { Values.of(it.value) }),
            )
        )
}

private class Caller(private val client: LoamsClient) {
    fun call(rpc: String, type: String, json: JsonValue, options: CallOptions?): DynamicMessage =
        client.invoke(rpc, Messages.of(type, json), options)
}

/** `loams.namespaces`: the container every collection and stream lives in. */
class NamespacesModule internal constructor(client: LoamsClient) {
    private val caller = Caller(client)

    /** Creates a namespace. A taken name is `already_exists`. Retry-safe. */
    fun create(name: String, options: CallOptions? = null): DynamicMessage =
        caller.call("$NS/CreateNamespace", "loams.collection.v1.CreateNamespaceRequest",
            Values.obj("namespace" to Values.str(name)), merged(options, RETRY_SAFE))

    /** One page of namespaces. */
    fun list(pageSize: Int = 0, pageToken: String = "", options: CallOptions? = null): DynamicMessage =
        caller.call("$NS/ListNamespaces", "loams.collection.v1.ListNamespacesRequest",
            Values.obj("pageSize" to JsonNumber(pageSize.toString()), "pageToken" to Values.str(pageToken)), options)

    /** Every namespace, following the page tokens. */
    fun listAll(pageSize: Int = 0, options: CallOptions? = null): Sequence<DynamicMessage> =
        pages("namespaces") { token -> list(pageSize, token, options) }
}

/** Follows `next_page_token` to the end, yielding the items of [field]. */
internal fun pages(field: String, fetch: (String) -> DynamicMessage): Sequence<DynamicMessage> = sequence {
    var token = ""
    repeat(100_000) {
        val page = fetch(token)
        yieldAll(page.messages(field))
        token = page.text("next_page_token")
        if (token.isEmpty()) return@sequence
    }
}

/** `loams.collections`: collections and their schemas. */
class CollectionsModule internal constructor(client: LoamsClient) {
    private val caller = Caller(client)

    private fun ref(collection: String, namespace: String?) =
        Values.obj("collection" to Values.str(collection), "namespace" to namespace?.let { Values.str(it) })

    /** Creates a collection with a schema from [Schema]. Idempotent by name. */
    fun create(name: String, schema: Schema, partitions: Int? = null, namespace: String? = null,
               options: CallOptions? = null): DynamicMessage =
        caller.call("$COL/CreateCollection", "loams.collection.v1.CreateCollectionRequest", Values.obj(
            "name" to Values.str(name),
            "schema" to schema.build(),
            "partitions" to partitions?.let { JsonNumber(it.toString()) },
            "namespace" to namespace?.let { Values.str(it) },
        ), merged(options, RETRY_SAFE))

    /** One page of the namespace's collections. */
    fun list(pageSize: Int = 0, pageToken: String = "", namespace: String? = null, options: CallOptions? = null): DynamicMessage =
        caller.call("$COL/ListCollections", "loams.collection.v1.ListCollectionsRequest", Values.obj(
            "pageSize" to JsonNumber(pageSize.toString()),
            "pageToken" to Values.str(pageToken),
            "namespace" to namespace?.let { Values.str(it) },
        ), options)

    /** Every collection in the namespace, following the page tokens. */
    fun listAll(pageSize: Int = 0, namespace: String? = null, options: CallOptions? = null): Sequence<DynamicMessage> =
        pages("collections") { token -> list(pageSize, token, namespace, options) }

    /** One collection, by name or alias. */
    fun get(collection: String, namespace: String? = null, options: CallOptions? = null): DynamicMessage =
        caller.call("$COL/GetCollection", "loams.collection.v1.GetCollectionRequest", ref(collection, namespace), options)

    /** Drops a collection. */
    fun drop(collection: String, namespace: String? = null, options: CallOptions? = null): DynamicMessage =
        caller.call("$COL/DropCollection", "loams.collection.v1.DropCollectionRequest", ref(collection, namespace),
            merged(options, RETRY_SAFE))

    /** Any other collection call by method (`AddFields`, `ListVersions`, `UpdateAliases`, `Scan`, `SetHot`, `WarmCollection`). */
    fun call(method: String, request: JsonValue, options: CallOptions? = null): DynamicMessage {
        val safe = method in setOf("AddFields", "UpdateAliases", "SetHot", "WarmCollection")
        return caller.call("$COL/$method", "loams.collection.v1.${method}Request", request,
            if (safe) merged(options, RETRY_SAFE) else options)
    }
}

/** A finished batch: the merged token and how many chunks and ops it wrote. */
data class BatchResult(val token: String, val chunks: Int, val ops: Int)

/**
 * A batch that stopped at a failed chunk (reason `batch_partially_applied`, client-side). The
 * failure is the cause.
 */
class BatchException(val appliedChunks: Int, val appliedOps: Int, val token: String, cause: LoamsException) :
    LoamsException(
        code = cause.code,
        rpc = cause.rpc,
        reason = cause.reason,
        unknownReason = BATCH_PARTIALLY_APPLIED,
        metadata = mapOf("applied_chunks" to appliedChunks.toString(), "applied_ops" to appliedOps.toString()),
        message = "write_batch stopped after $appliedChunks applied chunk(s) ($appliedOps ops): ${cause.message}",
    ) {
    companion object {
        const val BATCH_PARTIALLY_APPLIED: String = "batch_partially_applied"
    }
}

/** `documents.writeBatch` (runtime contract R13): chunks sent one at a time, one key per chunk. */
object WriteBatch {
    const val DEFAULT_CHUNK_ROWS: Int = 10_000
    const val DEFAULT_CHUNK_BYTES: Int = 4 * 1024 * 1024

    /** The chunks: at most [rows] ops, and a new chunk once the next op would pass [bytes]. */
    fun chunks(ops: List<JsonValue>, rows: Int, bytes: Int): List<List<JsonValue>> {
        val out = ArrayList<List<JsonValue>>()
        var chunk = ArrayList<JsonValue>()
        var size = 0
        for (op in ops) {
            val opSize = Json.writeCompact(op).length
            if (chunk.isNotEmpty() && (chunk.size >= rows || size + opSize > bytes)) {
                out.add(chunk)
                chunk = ArrayList()
                size = 0
            }
            chunk.add(op)
            size += opSize
        }
        if (chunk.isNotEmpty()) out.add(chunk)
        return out
    }

    /** Sends each chunk with its own key and merges the tokens. */
    fun run(ops: List<JsonValue>, rows: Int, bytes: Int, send: (List<JsonValue>, String) -> DynamicMessage): BatchResult {
        require(rows > 0 && bytes > 0) { "loams: writeBatch chunks need at least one row and one byte" }
        var merged: ConsistencyToken? = null
        var chunks = 0
        var applied = 0
        for (chunk in chunks(ops, rows, bytes)) {
            val answer = try {
                send(chunk, UuidV7.new())
            } catch (error: LoamsException) {
                throw BatchException(chunks, applied, merged?.toString() ?: "", error)
            }
            ConsistencyToken.tryParse(answer.text("token"))?.let { merged = merged?.merge(it) ?: it }
            chunks++
            applied += chunk.size
        }
        return BatchResult(merged?.toString() ?: "", chunks, applied)
    }
}

/** `loams.documents`: write, read, scroll and count documents. */
class DocumentsModule internal constructor(client: LoamsClient) {
    private val caller = Caller(client)

    private fun write(collection: String, ops: List<JsonValue>, namespace: String?, options: CallOptions?) =
        caller.call("$DOC/WriteDocuments", "loams.collection.v1.WriteDocumentsRequest", Values.obj(
            "collection" to Values.str(collection),
            "ops" to JsonArray(ops),
            "namespace" to namespace?.let { Values.str(it) },
        ), options)

    /** Upserts documents, as one write: keyed (R3), the key reused on every retry. */
    fun upsert(collection: String, docs: List<Doc>, namespace: String? = null, options: CallOptions? = null): DynamicMessage =
        write(collection, docs.map { it.op }, namespace, options)

    /** Deletes documents by id, as one write. */
    fun delete(collection: String, ids: List<Any>, namespace: String? = null, options: CallOptions? = null): DynamicMessage =
        write(collection, ids.map { Values.obj("delete" to Values.obj("id" to DocumentId.of(it).json)) }, namespace, options)

    /** Changes one document: [set] merges into its source and [unset] removes keys. */
    fun patch(collection: String, id: Any, set: Map<String, Any?>? = null, unset: List<String> = emptyList(),
              mode: String? = null, namespace: String? = null, options: CallOptions? = null): DynamicMessage =
        write(collection, listOf(Values.obj("patch" to Values.obj(
            "id" to DocumentId.of(id).json,
            "source" to set?.let { Values.of(it) },
            "deleteKeys" to if (unset.isEmpty()) null else JsonArray(unset.map { Values.str(it) }),
            "mode" to mode?.let { Values.str(it) },
        ))), namespace, options)

    /** Writes any number of documents in chunks, one chunk in flight at a time (R13). */
    fun writeBatch(collection: String, docs: List<Doc>, chunkRows: Int = WriteBatch.DEFAULT_CHUNK_ROWS,
                   chunkBytes: Int = WriteBatch.DEFAULT_CHUNK_BYTES, namespace: String? = null,
                   options: CallOptions? = null): BatchResult =
        WriteBatch.run(docs.map { it.op }, chunkRows, chunkBytes) { chunk, key ->
            write(collection, chunk, namespace, (options ?: CallOptions.NONE).copy(idempotencyKey = key))
        }

    /** Documents by id. */
    fun get(collection: String, ids: List<Any>, consistency: JsonValue? = null, namespace: String? = null,
            options: CallOptions? = null): DynamicMessage =
        caller.call("$DOC/GetDocuments", "loams.collection.v1.GetDocumentsRequest", Values.obj(
            "collection" to Values.str(collection),
            "ids" to JsonArray(ids.map { DocumentId.of(it).json }),
            "consistency" to consistency,
            "namespace" to namespace?.let { Values.str(it) },
        ), options)

    /** Every document matching [where] (all when null), in id order, following `next`. */
    fun scrollAll(collection: String, where: Query? = null, limit: Int? = null, namespace: String? = null,
                  options: CallOptions? = null): Sequence<DynamicMessage> = sequence {
        var next: JsonValue? = null
        repeat(100_000) {
            val page = caller.call("$DOC/ScrollDocuments", "loams.collection.v1.ScrollDocumentsRequest", Values.obj(
                "collection" to Values.str(collection),
                "filter" to where?.let { Filter.ir(it) },
                "limit" to limit?.let { JsonNumber(it.toString()) },
                "namespace" to namespace?.let { Values.str(it) },
                "pageToken" to next,
            ), options)
            yieldAll(page.messages("documents"))
            val after = page.field("next") as? DynamicMessage
            if (after == null || !page.hasField(page.descriptorForType.findFieldByName("next"))) return@sequence
            next = DocumentId.of(after)?.json ?: return@sequence
        }
    }

    /** How many documents match [where] (all when null). */
    fun count(collection: String, where: Query? = null, namespace: String? = null, options: CallOptions? = null): Long {
        val answer = caller.call("$DOC/CountDocuments", "loams.collection.v1.CountDocumentsRequest", Values.obj(
            "collection" to Values.str(collection),
            "filter" to where?.let { Filter.ir(it) },
            "namespace" to namespace?.let { Values.str(it) },
        ), options)
        return answer.field("count") as? Long ?: 0
    }

    /** `DeleteByFilter` or `PatchByFilter`, with the request's JSON. Never retried. */
    fun call(method: String, request: JsonValue, options: CallOptions? = null): DynamicMessage =
        caller.call("$DOC/$method", "loams.collection.v1.${method}Request", request, options)
}

/** `loams.search`: text, vector and hybrid search over one collection. */
class SearchModule internal constructor(client: LoamsClient) {
    private val caller = Caller(client)

    /** One search, from the builder. */
    fun query(search: Search, options: CallOptions? = null): DynamicMessage =
        caller.call("$QUERY/Search", "loams.collection.v1.SearchRequest", search.json(), options)

    /** A hybrid search: text on [textField] plus a vector on [vectorField], fused (RRF k=60 by default). */
    fun hybrid(collection: String, text: String, textField: String, vectorField: String, vector: FloatArray,
               fusion: JsonValue = rrf(), limit: Int? = null, options: CallOptions? = null): DynamicMessage =
        query(Search.on(collection).text(text, textField).vector(vectorField, vector).fuse(fusion)
            .also { s -> limit?.let { s.limit(it) } }, options)

    /**
     * Every hit, page after page, following `searchAfter` from the last hit's sort values until a
     * page comes back short. The search must sort with a primary-key tiebreaker ([Sort.pk]);
     * without one this throws `invalid_argument` before any call.
     */
    fun queryAll(search: Search, options: CallOptions? = null): Sequence<DynamicMessage> {
        if (search.sortKeys.none { (it as? JsonObject)?.entries?.containsKey("pk") == true }) {
            throw InvalidArgumentException(
                rpc = "$QUERY/Search",
                unknownReason = "search_after_needs_tiebreak",
                metadata = mapOf("field" to "sort"),
                message = "queryAll pages with searchAfter, which needs a sort with a primary-key tiebreaker: " +
                    "add Sort.pk() as the last sort key",
            )
        }
        val limit = search.limitValue ?: Search.DEFAULT_K
        search.limit(limit)
        return sequence {
            repeat(100_000) {
                val hits = query(search, options).messages("hits")
                yieldAll(hits)
                if (hits.size < limit || hits.isEmpty()) return@sequence
                @Suppress("UNCHECKED_CAST")
                val values = hits.last().field("sort_values") as List<DynamicMessage>
                search.after(values.map { Json.parseOrThrow(String(MessageCodec.serialize(it, Codec.JSON))) })
            }
        }
    }
}

/** `loams.vector`: the search RPC with a vector's shape (Q613). */
class VectorModule internal constructor(client: LoamsClient) {
    private val caller = Caller(client)

    /** The nearest neighbours of [vector] in [field]. */
    fun search(collection: String, vector: FloatArray, field: String, limit: Int? = null, filter: Query? = null,
               namespace: String? = null, options: CallOptions? = null): DynamicMessage {
        val search = Search.on(collection).vector(field, vector)
        limit?.let { search.limit(it) }
        filter?.let { search.filter(it) }
        namespace?.let { search.namespace(it) }
        return caller.call("$QUERY/Search", "loams.collection.v1.SearchRequest", search.json(), options)
    }
}

/** `loams.graph`: stored property graphs, queried with GQL. **Experimental** (GR1e). */
class GraphModule internal constructor(client: LoamsClient) {
    private val caller = Caller(client)

    private fun unkeyed(options: CallOptions?): CallOptions =
        if (!options?.idempotencyKey.isNullOrEmpty()) options!! else (options ?: CallOptions.NONE).copy(mintIdempotencyKey = false)

    /** Creates an owned graph. Keyed (R3). */
    fun create(name: String, namespace: String? = null, options: CallOptions? = null): DynamicMessage =
        caller.call("$GRAPH_ADMIN/CreateGraph", "loams.graph.v1.CreateGraphRequest",
            Values.obj("name" to Values.str(name), "namespace" to namespace?.let { Values.str(it) }), options)

    fun get(name: String, options: CallOptions? = null): DynamicMessage =
        caller.call("$GRAPH_ADMIN/GetGraph", "loams.graph.v1.GetGraphRequest", Values.obj("name" to Values.str(name)), options)

    /** Deletes a graph; answers its operation. Never retried unless the caller supplies a key. */
    fun delete(name: String, options: CallOptions? = null): DynamicMessage =
        caller.call("$GRAPH_ADMIN/DeleteGraph", "loams.graph.v1.DeleteGraphRequest", Values.obj("name" to Values.str(name)),
            unkeyed(options))

    fun schema(name: String, options: CallOptions? = null): DynamicMessage =
        caller.call("$GRAPH_ADMIN/GetSchema", "loams.graph.v1.GetSchemaRequest", Values.obj("name" to Values.str(name)), options)

    /** Runs one GQL statement. Read-only is retried freely and not keyed; a write is keyed (R3). */
    fun query(graph: String, statement: String, parameters: Map<String, JsonValue> = emptyMap(), readOnly: Boolean = false,
              namespace: String? = null, options: CallOptions? = null): DynamicMessage {
        val settings = if (readOnly) merged(options, CallOptions(retrySafe = true, mintIdempotencyKey = false)) else options
        return caller.call("$GRAPH/Execute", "loams.graph.v1.ExecuteRequest", Values.obj(
            "graph" to Values.str(graph),
            "statement" to Values.str(statement),
            "language" to Values.str("QUERY_LANGUAGE_GQL"),
            "readOnly" to if (readOnly) JsonBoolean(true) else null,
            "parameters" to if (parameters.isEmpty()) null else JsonObject(parameters),
            "namespace" to namespace?.let { Values.str(it) },
        ), settings)
    }

    /** Any other graph call by method (`UpdateGraph`, `ExecuteBatch`, `Explain`), with the request's JSON. */
    fun call(method: String, request: JsonValue, options: CallOptions? = null): DynamicMessage {
        val service = if (method in setOf("ExecuteBatch", "Explain")) GRAPH else GRAPH_ADMIN
        val settings = when (method) {
            "Explain" -> merged(options, RETRY_SAFE)
            "UpdateGraph" -> unkeyed(options)
            else -> options
        }
        return caller.call("$service/$method", "loams.graph.v1.${method}Request", request, settings)
    }

    /** GQL parameter values (`loams.graph.v1.Value`). */
    object Param {
        fun int(n: Long): JsonValue = Values.obj("int64" to Values.str(n.toString()))
        fun string(s: String): JsonValue = Values.obj("string" to Values.str(s))
        fun bool(b: Boolean): JsonValue = Values.obj("boolean" to JsonBoolean(b))
        fun double(d: Double): JsonValue = Values.obj("float64" to Values.number(d))
    }
}

/** The collection handle (SDKP §2.8): calls with the collection (and namespace) filled in. */
class CollectionHandle internal constructor(private val client: LoamsClient, val name: String, val namespace: String?) {
    fun upsert(docs: List<Doc>, options: CallOptions? = null) = client.documents.upsert(name, docs, namespace, options)
    fun get(ids: List<Any>, consistency: JsonValue? = null, options: CallOptions? = null) =
        client.documents.get(name, ids, consistency, namespace, options)
    fun delete(ids: List<Any>, options: CallOptions? = null) = client.documents.delete(name, ids, namespace, options)
    fun patch(id: Any, set: Map<String, Any?>? = null, unset: List<String> = emptyList(), options: CallOptions? = null) =
        client.documents.patch(name, id, set, unset, null, namespace, options)
    fun writeBatch(docs: List<Doc>, options: CallOptions? = null) =
        client.documents.writeBatch(name, docs, namespace = namespace, options = options)
    fun count(where: Query? = null, options: CallOptions? = null) = client.documents.count(name, where, namespace, options)
    fun scrollAll(where: Query? = null, limit: Int? = null, options: CallOptions? = null) =
        client.documents.scrollAll(name, where, limit, namespace, options)
    /** A search over this collection, starting from an empty builder. */
    fun search(options: CallOptions? = null, block: Search.() -> Unit): DynamicMessage {
        val search = Search(name).apply(block)
        namespace?.let { search.namespace(it) }
        return client.search.query(search, options)
    }
    fun vectorSearch(vector: FloatArray, field: String, limit: Int? = null, options: CallOptions? = null) =
        client.vector.search(name, vector, field, limit, null, namespace, options)
    fun drop(options: CallOptions? = null) = client.collections.drop(name, namespace, options)
    /** The collection's info (`collections.get`). */
    fun info(options: CallOptions? = null) = client.collections.get(name, namespace, options)
}
