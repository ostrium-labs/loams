package dev.loams

import com.google.protobuf.DynamicMessage
import dev.loams.Json.JsonArray
import dev.loams.Json.JsonBoolean
import dev.loams.Json.JsonNull
import dev.loams.Json.JsonNumber
import dev.loams.Json.JsonObject
import dev.loams.Json.JsonString

// The Schema, Filter and Search builders (SDKP §2.4, §2.6): the same verbs in every SDK, each a
// pure function onto the request's proto3 JSON, which [Messages.of] turns into the request message
// through the descriptor set.
//
//     val request = search("notes") {
//         text("refund", "body")
//         vector("embedding", floatArrayOf(0.1f, 0.2f, 0.3f, 0.4f))
//         fuse(rrf(60))
//         filter(Filter.eq("tags", "policy"))
//         limit(10)
//     }

/** Plain Kotlin values as proto3 JSON (`google.protobuf.Value` and friends). */
object Values {
    /** null, Boolean, a number, a String, a [JsonValue], a Map with string keys, an Iterable or an array. */
    fun of(value: Any?): JsonValue = when (value) {
        null -> JsonNull
        is JsonValue -> value
        is String -> JsonString(value)
        is Boolean -> JsonBoolean(value)
        is Int, is Long, is Short, is Byte -> JsonNumber(value.toString())
        is ULong -> if (value <= (1uL shl 53)) JsonNumber(value.toString()) else JsonString(value.toString())
        is UInt -> JsonNumber(value.toString())
        is Float -> number(value.toDouble())
        is Double -> number(value)
        is Number -> number(value.toDouble())
        is Map<*, *> -> JsonObject(value.entries.associate { (k, v) -> k.toString() to of(v) })
        is FloatArray -> JsonArray(value.map { number(it.toDouble()) })
        is DoubleArray -> JsonArray(value.map { number(it) })
        is Array<*> -> JsonArray(value.map { of(it) })
        is Iterable<*> -> JsonArray(value.map { of(it) })
        else -> throw IllegalArgumentException("loams: cannot send a ${value::class.simpleName} as a protobuf Value")
    }

    /** A number in its shortest form: `4`, not `4.0`. */
    fun number(value: Double): JsonValue {
        require(value.isFinite()) { "loams: a NaN or infinite number has no JSON form" }
        return if (value == Math.rint(value) && Math.abs(value) < 1e15) {
            JsonNumber(value.toLong().toString())
        } else {
            JsonNumber(value.toString())
        }
    }

    /** A JSON object from pairs; null values are left out. */
    fun obj(vararg pairs: Pair<String, JsonValue?>): JsonObject =
        JsonObject(linkedMapOf(*pairs.filter { it.second != null }.map { it.first to it.second!! }.toTypedArray()))

    fun str(value: String): JsonValue = JsonString(value)

    fun arr(values: List<JsonValue>): JsonValue = JsonArray(values)
}

/** A document id: an unsigned 64-bit integer, a string or a UUID. */
sealed class DocumentId {
    data class Uint(val value: ULong) : DocumentId()
    data class Str(val value: String) : DocumentId()
    data class Uuid(val value: String) : DocumentId()

    /** The proto3 JSON of `loams.collection.v1.DocumentId` (a `uint64` is a string). */
    val json: JsonValue
        get() = when (this) {
            is Uint -> Values.obj("uint" to Values.str(value.toString()))
            is Str -> Values.obj("string" to Values.str(value))
            is Uuid -> Values.obj("uuid" to Values.str(value))
        }

    /** The bare form `Query.ids` carries. */
    internal val bare: JsonValue
        get() = when (this) {
            is Uint -> if (value <= (1uL shl 53)) JsonNumber(value.toString()) else json
            is Str -> Values.str(value)
            is Uuid -> json
        }

    /** The id as text. */
    val text: String
        get() = when (this) {
            is Uint -> value.toString()
            is Str -> value
            is Uuid -> value
        }

    companion object {
        /** An id from a Long, Int, ULong, String or DocumentId. */
        fun of(id: Any): DocumentId = when (id) {
            is DocumentId -> id
            is ULong -> Uint(id)
            is Long -> { require(id >= 0) { "loams: a numeric document id is unsigned" }; Uint(id.toULong()) }
            is Int -> { require(id >= 0) { "loams: a numeric document id is unsigned" }; Uint(id.toULong()) }
            is String -> Str(id)
            is java.util.UUID -> Uuid(id.toString())
            else -> throw IllegalArgumentException("loams: a document id is a number, a string or a UUID")
        }

        /** An id from a response message's `DocumentId`. */
        fun of(message: DynamicMessage): DocumentId? {
            for (field in message.allFields.keys) {
                val value = message.getField(field)
                return when (field.name) {
                    "uint" -> Uint((value as Long).toULong())
                    "string" -> Str(value as String)
                    "uuid" -> Uuid(value as String)
                    else -> null
                }
            }
            return null
        }
    }
}

// ---------------------------------------------------------------- Schema

/** Builds a collection schema, the `Struct` `CreateCollectionRequest.schema` takes. */
class Schema {
    private val fields = ArrayList<JsonValue>()
    private val vectors = ArrayList<JsonValue>()
    private val sparse = ArrayList<JsonValue>()
    private val annotations = LinkedHashMap<String, JsonValue>()
    private var dynamicMode = "ignore"
    private var maxFieldCount = 1000

    private fun field(name: String, kind: JsonValue, indexed: Boolean, fast: Boolean, sourcePath: String?) = apply {
        fields.add(
            Values.obj(
                "name" to Values.str(name),
                "source_path" to Values.str(sourcePath ?: name),
                "kind" to kind,
                "indexed" to JsonBoolean(indexed),
                "fast" to JsonBoolean(fast),
            )
        )
    }

    fun text(name: String, analyzer: String = "standard", positions: Boolean = true, indexed: Boolean = true,
             fast: Boolean = false, sourcePath: String? = null) =
        field(name, Values.obj("text" to Values.obj("analyzer" to Values.str(analyzer), "positions" to JsonBoolean(positions))),
            indexed, fast, sourcePath)

    fun keyword(name: String, indexed: Boolean = true, fast: Boolean = true, sourcePath: String? = null) =
        field(name, Values.str("keyword"), indexed, fast, sourcePath)

    fun i64(name: String, indexed: Boolean = true, fast: Boolean = true, sourcePath: String? = null) =
        field(name, Values.str("i64"), indexed, fast, sourcePath)

    fun f64(name: String, indexed: Boolean = true, fast: Boolean = true, sourcePath: String? = null) =
        field(name, Values.str("f64"), indexed, fast, sourcePath)

    fun bool(name: String, indexed: Boolean = true, fast: Boolean = true, sourcePath: String? = null) =
        field(name, Values.str("bool"), indexed, fast, sourcePath)

    /** A timestamp field (the engine's `date` kind). */
    fun timestamp(name: String, indexed: Boolean = true, fast: Boolean = true, sourcePath: String? = null) =
        field(name, Values.str("date"), indexed, fast, sourcePath)

    fun uuid(name: String, indexed: Boolean = true, fast: Boolean = true, sourcePath: String? = null) =
        field(name, Values.str("uuid"), indexed, fast, sourcePath)

    fun json(name: String, indexed: Boolean = true, fast: Boolean = false, sourcePath: String? = null) =
        field(name, Values.str("json"), indexed, fast, sourcePath)

    fun vector(name: String, dim: Int, distance: String = "cosine") = apply {
        vectors.add(Values.obj("name" to Values.str(name), "dim" to JsonNumber(dim.toString()), "distance" to Values.str(distance)))
    }

    fun sparseVector(name: String, modifier: String? = null) = apply {
        sparse.add(Values.obj("name" to Values.str(name), "modifier" to modifier?.let { Values.str(it) }))
    }

    /** What happens to a source key no field names: `ignore`, `strict` or `map`. */
    fun dynamic(mode: String) = apply { dynamicMode = mode }

    fun maxFields(n: Int) = apply { maxFieldCount = n }

    fun annotation(key: String, value: String) = apply { annotations[key] = Values.str(value) }

    /** The schema as the request's `Struct`, in JSON. */
    fun build(): JsonObject = Values.obj(
        "fields" to JsonArray(fields.toList()),
        "vectors" to JsonArray(vectors.toList()),
        "sparse_vectors" to JsonArray(sparse.toList()),
        "dynamic" to Values.str(dynamicMode),
        "max_fields" to JsonNumber(maxFieldCount.toString()),
        "annotations" to if (annotations.isEmpty()) null else JsonObject(annotations.toMap()),
    )
}

// ---------------------------------------------------------------- Filter

/** A `loams.collection.v1.Query`, as proto3 JSON. `and`, `or` and `!` are optional sugar. */
data class Query(val json: JsonValue) {
    infix fun and(other: Query): Query = Filter.and(this, other)
    infix fun or(other: Query): Query = Filter.or(this, other)
    operator fun not(): Query = Filter.not(this)
}

/** The filter builder: the same verbs in every SDK. */
object Filter {
    private fun arm(name: String, vararg body: Pair<String, JsonValue?>) = Query(Values.obj(name to Values.obj(*body)))
    private fun bool(name: String, queries: List<Query>, extra: Pair<String, JsonValue?>? = null) =
        arm("bool", name to JsonArray(queries.map { it.json }), extra ?: ("" to null))

    fun all() = Query(Values.obj("matchAll" to Values.obj()))
    fun none() = Query(Values.obj("matchNone" to Values.obj()))

    /** The field equals the value exactly (`term`). */
    fun eq(field: String, value: Any?) = arm("term", "field" to Values.str(field), "value" to Values.of(value))

    /** The field equals one of the values (`terms`). */
    fun `in`(field: String, vararg values: Any?) =
        arm("terms", "field" to Values.str(field), "values" to JsonArray(values.map { Values.of(it) }))

    /** The field is in a range; each bound left null is open. */
    fun range(field: String, gt: Any? = null, gte: Any? = null, lt: Any? = null, lte: Any? = null) = arm(
        "range", "field" to Values.str(field),
        "gt" to gt?.let { Values.of(it) }, "gte" to gte?.let { Values.of(it) },
        "lt" to lt?.let { Values.of(it) }, "lte" to lte?.let { Values.of(it) },
    )

    fun exists(field: String) = arm("exists", "field" to Values.str(field))
    fun isNull(field: String) = arm("isNull", "field" to Values.str(field))
    fun isEmpty(field: String) = arm("isEmpty", "field" to Values.str(field))
    fun prefix(field: String, value: String) = arm("prefix", "field" to Values.str(field), "value" to Values.str(value))
    fun wildcard(field: String, pattern: String) = arm("wildcard", "field" to Values.str(field), "pattern" to Values.str(pattern))

    /** Within [edits] edits of the value; null means `auto`. */
    fun fuzzy(field: String, value: String, edits: Int? = null) = arm(
        "fuzzy", "field" to Values.str(field), "value" to Values.str(value),
        "fuzziness" to if (edits == null) Values.obj("auto" to Values.obj()) else Values.obj("edits" to JsonNumber(edits.toString())),
    )

    /** A full-text match on one field (scored). */
    fun match(field: String, text: String, all: Boolean = false) = arm(
        "match", "field" to Values.str(field), "text" to Values.str(text), "operator" to if (all) Values.str("and") else null,
    )

    fun phrase(field: String, text: String, slop: Int = 0) = arm(
        "matchPhrase", "field" to Values.str(field), "text" to Values.str(text),
        "slop" to if (slop > 0) JsonNumber(slop.toString()) else null,
    )

    fun multiMatch(text: String, vararg fields: String) = arm(
        "multiMatch", "text" to Values.str(text), "fields" to JsonArray(fields.map { Values.obj("field" to Values.str(it)) }),
    )

    fun queryString(query: String, vararg defaultFields: String) = arm(
        "queryString", "query" to Values.str(query),
        "defaultFields" to if (defaultFields.isEmpty()) null else JsonArray(defaultFields.map { Values.str(it) }),
    )

    /** The documents with these ids (Longs, Ints, Strings, UUIDs or DocumentIds). */
    fun ids(vararg ids: Any) = Query(Values.obj("ids" to JsonArray(ids.map { DocumentId.of(it).bare })))

    fun and(vararg queries: Query) = bool("must", queries.toList())
    fun or(vararg queries: Query) = bool("should", queries.toList())
    fun orAtLeast(minimumShouldMatch: String, vararg queries: Query) =
        bool("should", queries.toList(), "minimumShouldMatch" to Values.str(minimumShouldMatch))
    fun not(vararg queries: Query) = bool("mustNot", queries.toList())

    /** All of the queries, without scoring (`bool.filter`; the plan's `filter` verb). */
    fun nonScoring(vararg queries: Query) = bool("filter", queries.toList())

    fun boost(query: Query, boost: Double) = Query(Values.obj("boost" to Values.obj("query" to query.json, "boost" to Values.number(boost))))
    fun constantScore(query: Query, score: Double) =
        Query(Values.obj("constantScore" to Values.obj("query" to query.json, "score" to Values.number(score))))

    /**
     * A filter in the engine's native JSON IR, as `ScrollDocuments`, `CountDocuments`,
     * `DeleteByFilter` and `PatchByFilter` take it in their `filter` field today: snake_case keys,
     * unit arms as bare strings, ids as bare keys (`connect_query_filters.rs`'s mapping). Retired
     * when those requests gain a typed `where` (SDKP P0a).
     */
    fun ir(query: Query): JsonValue = irOf(query.json)

    private fun snake(key: String): String = buildString {
        for (c in key) if (c.isUpperCase()) append('_').append(c.lowercaseChar()) else append(c)
    }

    private fun irOf(value: JsonValue): JsonValue {
        val fields = (value as? JsonObject)?.entries ?: return value
        if (fields.size != 1) return value
        val (key, body) = fields.entries.first()
        val name = snake(key)
        return when (name) {
            "match_all", "match_none" -> Values.str(name)
            "ids" -> Values.obj("ids" to JsonArray((body as JsonArray).items.map { id ->
                ((id as? JsonObject)?.entries?.get("uint") as? JsonString)?.let { JsonNumber(it.value) } ?: id
            }))
            "bool" -> Values.obj("bool" to JsonObject((body as JsonObject).entries.entries.associate { (arm, q) ->
                snake(arm) to if (q is JsonArray) JsonArray(q.items.map { irOf(it) }) else q
            }))
            "boost", "constant_score" -> Values.obj(name to JsonObject((body as JsonObject).entries.mapValues { (k, v) ->
                if (k == "query") irOf(v) else v
            }))
            else -> Values.obj(name to JsonObject(((body as? JsonObject)?.entries ?: emptyMap()).entries.associate { (k, v) ->
                if (k == "fuzziness" && v is JsonObject) k to (v.entries["edits"] ?: Values.str("auto")) else snake(k) to v
            }))
        }
    }
}

// ---------------------------------------------------------------- Search

/** Reciprocal rank fusion with constant [k]. */
fun rrf(k: Int = 60): JsonValue = Values.obj("rrf" to Values.obj("k" to JsonNumber(k.toString())))

/** Distribution-based score fusion. */
fun dbsf(): JsonValue = Values.obj("dbsf" to Values.obj())

/** A weighted sum of the retrievers' normalised scores, one weight per retriever. */
fun weighted(vararg weights: Double): JsonValue =
    Values.obj("weightedSum" to Values.obj("weights" to JsonArray(weights.map { Values.number(it) })))

/** Sort keys. */
object Sort {
    /**
     * By score; null leaves the order unset, which is the IR's default for a score (descending)
     * and what the builder corpus's `sort` cases send (`{"score":{}}`).
     */
    fun score(ascending: Boolean? = null): JsonValue =
        Values.obj("score" to Values.obj("order" to ascending?.let { Values.str(if (it) "SORT_ORDER_ASC" else "SORT_ORDER_DESC") }))

    /** By primary key; null leaves the order unset, the IR's default for a key (ascending). */
    fun pk(descending: Boolean? = null): JsonValue =
        Values.obj("pk" to Values.obj("order" to descending?.let { Values.str(if (it) "SORT_ORDER_DESC" else "SORT_ORDER_ASC") }))

    fun field(name: String, descending: Boolean = false, missingFirst: Boolean? = null): JsonValue = Values.obj(
        "field" to Values.obj(
            "field" to Values.str(name),
            "order" to Values.str(if (descending) "SORT_ORDER_DESC" else "SORT_ORDER_ASC"),
            "missing" to missingFirst?.let { Values.str(if (it) "MISSING_ORDER_FIRST" else "MISSING_ORDER_LAST") },
        )
    )
}

/** Consistency levels for a collection read. */
object Consistent {
    fun strong(): JsonValue = Values.obj("freshness" to Values.str("FRESHNESS_STRONG"))
    fun eventual(): JsonValue = Values.obj("freshness" to Values.str("FRESHNESS_EVENTUAL"))
    fun atLeast(token: String): JsonValue = Values.obj("atLeast" to Values.str(token))
}

/** Builds a `SearchRequest`; the DSL form is [search]. */
class Search(val collection: String) {
    private var namespace: String? = null
    private val retrievers = ArrayList<JsonObject>()
    private val fields = LinkedHashMap<String, JsonValue>()
    private val highlights = ArrayList<JsonValue>()
    internal val sortKeys = ArrayList<JsonValue>()
    internal var limitValue: Int? = null

    fun namespace(ns: String) = apply { namespace = ns }

    /** A text retriever: a `match` on one field, a `multiMatch` on several, a query string on none. */
    fun text(text: String, vararg fields: String) = text(
        when (fields.size) {
            0 -> Filter.queryString(text)
            1 -> Filter.match(fields[0], text)
            else -> Filter.multiMatch(text, *fields)
        }
    )

    /** A text retriever over any scoring query; `k` null takes the default. */
    fun text(query: Query, k: Int? = null) = apply {
        retrievers.add(Values.obj("text" to Values.obj("query" to query.json, "k" to k?.let { JsonNumber(it.toString()) })))
    }

    /** A dense vector retriever (kNN). */
    fun vector(field: String, values: FloatArray, k: Int? = null, params: JsonValue? = null, filter: Query? = null) = apply {
        retrievers.add(
            Values.obj(
                "vector" to Values.obj(
                    "field" to Values.str(field),
                    "query" to Values.of(values),
                    "k" to k?.let { JsonNumber(it.toString()) },
                    "params" to params,
                    "filter" to filter?.json,
                )
            )
        )
    }

    fun sparse(field: String, terms: Map<String, Double>, k: Int? = null) = apply {
        retrievers.add(
            Values.obj(
                "sparse" to Values.obj(
                    "field" to Values.str(field),
                    "query" to JsonObject(terms.mapValues { Values.number(it.value) }),
                    "k" to k?.let { JsonNumber(it.toString()) },
                )
            )
        )
    }

    fun fuse(fusion: JsonValue) = apply { fields["fusion"] = fusion }
    fun filter(query: Query) = apply { fields["filter"] = query.json }
    fun sort(vararg keys: JsonValue) = apply { sortKeys.clear(); sortKeys.addAll(keys) }
    fun limit(n: Int) = apply { limitValue = n }
    fun offset(n: Int) = apply { fields["offset"] = JsonNumber(n.toString()) }
    fun after(sortValues: List<JsonValue>) = apply { fields["searchAfter"] = JsonArray(sortValues) }
    fun minScore(score: Double) = apply { fields["scoreThreshold"] = Values.number(score) }

    fun select(include: List<String>? = null, exclude: List<String>? = null, vectors: List<String>? = null,
               typed: List<String>? = null) = apply {
        fields["select"] = Values.obj(
            "source" to if (include == null && exclude == null) null else Values.obj(
                // `select` is a Struct, so an empty list is sent rather than omitted: leave out
                // the side the caller did not give, as the builder corpus does.
                "include" to include?.let { v -> JsonArray(v.map { Values.str(it) }) },
                "exclude" to exclude?.let { v -> JsonArray(v.map { Values.str(it) }) },
            ),
            "vectors" to vectors?.let { v -> JsonArray(v.map { Values.str(it) }) },
            "fields" to typed?.let { v -> JsonArray(v.map { Values.str(it) }) },
        )
    }

    fun highlight(field: String, pre: String? = null, post: String? = null, fragmentSize: Int? = null, fragments: Int? = null) =
        apply {
            highlights.add(
                Values.obj(
                    "field" to Values.str(field),
                    "preTag" to pre?.let { Values.str(it) },
                    "postTag" to post?.let { Values.str(it) },
                    "fragmentSize" to fragmentSize?.let { JsonNumber(it.toString()) },
                    "numberOfFragments" to fragments?.let { JsonNumber(it.toString()) },
                )
            )
        }

    fun groupBy(field: String, size: Int? = null, limit: Int? = null) = apply {
        fields["groupBy"] = Values.obj(
            "field" to Values.str(field),
            "groupSize" to size?.let { JsonNumber(it.toString()) },
            "limit" to limit?.let { JsonNumber(it.toString()) },
        )
    }

    /** Expands each hit through a graph (`expandSpec`). */
    fun expand(graph: String, fromField: String, hops: Int = 1, direction: String? = null, edgeTypes: List<String> = emptyList(),
               limitPerSeed: Int? = null, limit: Int? = null) = apply {
        fields["expandSpec"] = Values.obj(
            "graph" to Values.str(graph),
            "fromField" to Values.str(fromField),
            "hops" to JsonNumber(hops.toString()),
            "direction" to direction?.let { Values.str(it) },
            "edgeTypes" to if (edgeTypes.isEmpty()) null else JsonArray(edgeTypes.map { Values.str(it) }),
            "limitPerSeed" to limitPerSeed?.let { JsonNumber(it.toString()) },
            "limit" to limit?.let { JsonNumber(it.toString()) },
        )
    }

    fun consistency(consistency: JsonValue) = apply { fields["consistency"] = consistency }

    /**
     * The request's JSON. A retriever's `k` is sent only when the caller set it; left unset, the
     * engine fills in `max(10, offset + limit)`.
     */
    fun json(): JsonObject {
        val out = LinkedHashMap<String, JsonValue>()
        out["collection"] = Values.str(collection)
        namespace?.let { out["namespace"] = Values.str(it) }
        out["retrievers"] = JsonArray(retrievers.toList())
        out.putAll(fields)
        if (sortKeys.isNotEmpty()) out["sort"] = JsonArray(sortKeys.toList())
        limitValue?.let { out["limit"] = JsonNumber(it.toString()) }
        if (highlights.isNotEmpty()) out["highlight"] = Values.obj("fields" to JsonArray(highlights.toList()))
        return JsonObject(out)
    }

    /** The request message. */
    fun build(): DynamicMessage = Messages.of("loams.collection.v1.SearchRequest", json())

    companion object {
        /** The page size `queryAll` uses when the search names no limit. */
        const val DEFAULT_K: Int = 10

        /** A search over one collection. */
        fun on(collection: String): Search = Search(collection)
    }
}

/** The search DSL: `search("notes") { text("refund", "body"); fuse(rrf(60)) }`. */
fun search(collection: String, block: Search.() -> Unit): Search = Search(collection).apply(block)

/** Request messages from proto3 JSON, through the committed descriptor set. */
object Messages {
    /** The message `fullName` the JSON describes. */
    fun of(fullName: String, json: JsonValue): DynamicMessage {
        val descriptor = requireNotNull(Descriptors.message(fullName)) {
            "the committed descriptor set carries no $fullName; regenerate it with './build.sh descriptors'"
        }
        return MessageCodec.parseJson(descriptor, Json.writeCompact(json))
    }
}
