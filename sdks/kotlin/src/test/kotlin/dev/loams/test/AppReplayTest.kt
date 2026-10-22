package dev.loams.test

import com.google.protobuf.DynamicMessage
import dev.loams.CallOptions
import dev.loams.Codec
import dev.loams.CompactJson
import dev.loams.Filter
import dev.loams.Json
import dev.loams.JsonValue
import dev.loams.LoamsClient
import dev.loams.LoamsException
import dev.loams.Messages
import dev.loams.ProducerRecord
import dev.loams.Query
import dev.loams.Schema
import dev.loams.Search
import dev.loams.Sort
import dev.loams.Values
import dev.loams.dbsf
import dev.loams.rrf
import dev.loams.test.Support.FixtureServer
import dev.loams.test.Support.RepositoryRoot
import dev.loams.weighted
import java.io.File

/**
 * The recorded app scenario (`sdks/fixtures/recorded/app/`, `app-scenario.md`), replayed
 * through the public facade against the fixture server (SDKP P0d).
 *
 * Every step names its facade call (`documents.write`) and carries the recorded request and
 * answer. The call is resolved through `facade-index.json` to its RPC and retry class, and the
 * recorded body goes through [LoamsClient.invoke], the path every module method delegates to
 * (namespace, idempotency key, retries, errors), over the Connect JSON codec. `streams.produce`
 * and `streams.fetch` go through the typed [dev.loams.StreamsModule] methods instead, so the
 * fixture server's byte comparison also pins how the facade writes a keyed record and an offset.
 * A step that names a builder case is first checked against this SDK's builder driven from the
 * case's `spec`, so the replay proves the builder too. The answer is compared with the recording
 * with the fixture's `volatile` fields masked (numbers to ~1e-6 relative); a refusal is compared
 * by its code and, when the recording carries one, its reason.
 *
 * `check-languages.mjs --check kotlin --suite app` reads the fixtures these record as run.
 */
object AppReplayTest {
    fun register() {
        // Spelled out: `check-languages.mjs` finds each name by a static scan of the suite.
        Harness.test("kotlin_app_collections") { replay("app_collections") }
        Harness.test("kotlin_app_search") { replay("app_search") }
        Harness.test("kotlin_app_graph") { replay("app_graph") }
        Harness.test("kotlin_app_sql") { replay("app_sql") }
        Harness.test("kotlin_app_streams") { replay("app_streams") }
    }

    private val root: File by lazy { RepositoryRoot.find(File(".").absoluteFile) }
    private val fixtures: File get() = File(root, "sdks/fixtures")

    private fun obj(value: JsonValue?): Json.JsonObject? = value?.asObject()

    private fun replay(name: String) {
        val file = File(fixtures, "recorded/app/$name.json")
        check(file.isFile) { "no recording at ${file.path}" }
        FixtureServer.start(fixtures).use { server ->
            val failures = run(server.endpoint, Json.parseOrThrow(file.readText()).asObject()!!, name)
            check(failures.isEmpty()) { failures.joinToString("\n        ") }
        }
        record(name)
    }

    private class IndexEntry(val rpc: String, val retry: String)

    private val index: Map<String, IndexEntry> by lazy {
        val calls = Json.parseOrThrow(File(fixtures, "facade-index.json").readText()).asObject()!!["calls"]!!.asArray()!!
        calls.associate { call ->
            val c = call.asObject()!!
            normal(c.member("module")!!, c.member("call")!!) to IndexEntry(c.member("rpc")!!, c.member("retry")!!)
        }
    }

    // The recordings name calls in snake_case and the index in each module's own casing.
    private fun normal(module: String, call: String) = "$module.${call.replace("_", "").lowercase()}"

    private fun run(endpoint: String, fixture: Json.JsonObject, name: String): List<String> {
        val volatile = fixture["volatile"]?.asArray()?.mapNotNull { it.asString() }?.toSet() ?: emptySet()
        val client = LoamsClient(endpoint = endpoint, codec = Codec.JSON, maxRetries = 0)
        val failures = mutableListOf<String>()
        fixture["steps"]!!.asArray()!!.forEachIndexed { index, value ->
            val step = value.asObject()!!
            val call = step.member("call")!!
            val label = "$name step $index ($call)"
            val body = obj(step["request"])?.member("body")
            val request = if (body.isNullOrEmpty()) Json.JsonObject(emptyMap()) else Json.parseOrThrow(body).asObject()!!
            step.member("builder")?.let { case -> checkBuilder(case, request)?.let { failures.add("$label: $it") } }
            val expected = obj(step["response"])!!
            val status = expected["status"]?.asLong()?.toInt()
            val headers = mapOf("loams-fixture-name" to name, "loams-fixture-step" to index.toString())
            try {
                val answer = invoke(client, call, request, headers)
                if (status != 200) {
                    failures.add("$label: answered, but the recording is a ${obj(expected["body"])?.member("code")}")
                } else {
                    val got = Json.parseOrThrow(CompactJson.format(answer))
                    compare(mask(expected["body"]!!, volatile), mask(got, volatile), "$")?.let { failures.add("$label: $it") }
                }
            } catch (thrown: LoamsException) {
                val want = obj(expected["body"])
                if (status == 200) {
                    failures.add("$label: refused (${thrown.code.wireName}) where the recording answered")
                } else if (thrown.code.wireName != want?.member("code")) {
                    failures.add("$label: refused with ${thrown.code.wireName}, the recording with ${want?.member("code")}")
                } else {
                    val reason = recordedReason(want)
                    val gotReason = thrown.detail?.reason ?: thrown.unknownReason
                    if (reason != null && reason != gotReason) {
                        failures.add("$label: refused with reason $gotReason, the recording with $reason")
                    }
                }
            }
        }
        return failures
    }

    /** One step's call, through the facade. */
    private fun invoke(client: LoamsClient, call: String, request: Json.JsonObject, headers: Map<String, String>): DynamicMessage {
        val module = call.substringBeforeLast('.')
        val entry = requireNotNull(index[normal(module, call.substringAfterLast('.'))]) {
            "facade-index.json has no call $call"
        }
        val options = CallOptions(headers = headers, retrySafe = entry.retry == "safe")
        return when (call) {
            // The typed Part C calls: what the facade writes is what the recording sent.
            "streams.produce" -> client.streams.produce(
                stream = request.member("stream")!!,
                partition = request["partition"]?.asLong()?.toInt() ?: 0,
                records = request["records"]!!.asArray()!!.map { r ->
                    val record = r.asObject()!!
                    ProducerRecord(record.member("key")?.let(::bytes), bytes(record.member("value")!!))
                },
                producerId = request.member("producerId"),
                producerSeq = request["producerSeq"]?.let { it.asString()?.toLong() ?: it.asLong() },
                namespace = request.member("namespace"),
                options = options,
            )
            "streams.fetch" -> client.streams.fetch(
                stream = request.member("stream")!!,
                partition = request["partition"]?.asLong()?.toInt() ?: 0,
                offset = request["offset"]?.let { it.asString()?.toLong() ?: it.asLong() } ?: 0L,
                maxRecords = request["maxRecords"]?.asLong()?.toInt(),
                namespace = request.member("namespace"),
                options = options,
            )
            else -> {
                val binding = client.binding(entry.rpc)
                client.invoke(entry.rpc, Messages.of(binding.request.fullName, request), options)
            }
        }
    }

    private fun bytes(base64: String): ByteArray = java.util.Base64.getDecoder().decode(base64)

    /** The `reason` of the recorded refusal's `ErrorInfo` detail, or null when it has none. */
    private fun recordedReason(body: Json.JsonObject?): String? {
        val detail = body?.get("details")?.asArray()?.firstOrNull()?.asObject() ?: return null
        if (detail.member("type") != "loams.errors.v1.ErrorInfo") return null
        val raw = detail.member("value") ?: return null
        val padded = raw.replace('-', '+').replace('_', '/').let { it + "=".repeat((4 - it.length % 4) % 4) }
        val descriptor = dev.loams.Descriptors.message("loams.errors.v1.ErrorInfo") ?: return null
        val message = DynamicMessage.parseFrom(descriptor, java.util.Base64.getDecoder().decode(padded))
        return (message.getField(descriptor.findFieldByName("reason")) as String).ifEmpty { null }
    }

    // ---- builders -------------------------------------------------------------------------

    /** The builder case's output against the part of the request it builds, or null when they match. */
    private fun checkBuilder(case: String, request: Json.JsonObject): String? {
        val file = File(fixtures, "builders/$case.json")
        if (!file.isFile) return "no builder case $case"
        val c = Json.parseOrThrow(file.readText()).asObject()!!
        val message = c.member("message")!!
        val built: JsonValue
        val part: JsonValue?
        when (c.member("builder")) {
            "schema" -> {
                built = schema(c["spec"]!!.asArray()!!)
                part = request["schema"]
            }
            "filter" -> {
                built = filter(c["spec"]!!).json
                part = request["where"] ?: request["filter"]
            }
            "search" -> {
                built = search(c["spec"]!!.asObject()!!).json()
                part = Json.JsonObject(request.entries.filterKeys { it != "namespace" })
            }
            else -> return "builder case $case has an unknown builder ${c.member("builder")}"
        }
        if (part == null) return "the recording has no part for the $case builder"
        val same = if (message == "google.protobuf.Struct") {
            compare(part, built, "$") == null
        } else {
            Messages.of(message, part) == Messages.of(message, built)
        }
        return if (same) null else "the $case builder builds ${Json.writeCompact(built)}, the recording sent ${Json.writeCompact(part)}"
    }

    private class Call(val op: String, val args: List<JsonValue>) {
        val options: Json.JsonObject? get() = args.lastOrNull()?.asObject()?.takeIf { it["op"] == null }
        fun str(i: Int) = args[i].asString()!!
        fun num(i: Int) = (args[i] as Json.JsonNumber).raw.toDouble()
    }

    private fun call(value: JsonValue): Call {
        val o = value.asObject()!!
        return Call(o.member("op")!!, o["args"]?.asArray() ?: emptyList())
    }

    private fun isCall(value: JsonValue) = value.asObject()?.get("op") != null

    private fun schema(spec: List<JsonValue>): JsonValue {
        val s = Schema()
        for (v in spec) {
            val c = call(v)
            when (c.op) {
                "text" -> s.text(c.str(0))
                "keyword" -> s.keyword(c.str(0))
                "i64" -> s.i64(c.str(0))
                "f64" -> s.f64(c.str(0))
                "bool" -> s.bool(c.str(0))
                "timestamp" -> s.timestamp(c.str(0))
                "uuid" -> s.uuid(c.str(0))
                "json" -> s.json(c.str(0))
                "vector" -> s.vector(c.str(0), c.num(1).toInt(), c.options?.member("distance") ?: "cosine")
                else -> error("schema op ${c.op} is not driven here")
            }
        }
        return s.build()
    }

    private fun plain(value: JsonValue): Any? = when (value) {
        is Json.JsonString -> value.value
        is Json.JsonNumber -> value.raw.toLongOrNull() ?: value.raw.toDouble()
        is Json.JsonBoolean -> value.value
        else -> null
    }

    private fun filter(spec: JsonValue): Query {
        val c = call(spec)
        return when (c.op) {
            "eq" -> Filter.eq(c.str(0), plain(c.args[1]))
            "range" -> c.options!!.let { o ->
                Filter.range(c.str(0), gt = o["gt"]?.let(::plain), gte = o["gte"]?.let(::plain),
                    lt = o["lt"]?.let(::plain), lte = o["lte"]?.let(::plain))
            }
            "exists" -> Filter.exists(c.str(0))
            "and" -> Filter.and(*c.args.map(::filter).toTypedArray())
            "or" -> Filter.or(*c.args.map(::filter).toTypedArray())
            "not" -> Filter.not(*c.args.map(::filter).toTypedArray())
            else -> error("filter op ${c.op} is not driven here")
        }
    }

    private fun search(spec: Json.JsonObject): Search {
        val s = Search.on(spec.member("on")!!)
        for (v in spec["calls"]!!.asArray()!!) {
            val c = call(v)
            when (c.op) {
                "text" -> {
                    val fields = c.options?.get("fields")?.asArray()?.map { it.asString()!! } ?: emptyList()
                    s.text(c.str(0), *fields.toTypedArray())
                }
                "vector" -> {
                    val values = c.args[1].asArray()!!.map { (it as Json.JsonNumber).raw.toFloat() }.toFloatArray()
                    val o = c.options
                    val params = o?.entries?.filterKeys { it != "k" && it != "filter" }
                        ?.mapKeys { (k, _) -> camel(k) }
                        ?.takeIf { it.isNotEmpty() }?.let { Json.JsonObject(it) }
                    s.vector(c.str(0), values, k = o?.get("k")?.asLong()?.toInt(), params = params)
                }
                "filter" -> s.filter(filter(c.args[0]))
                "highlight" -> s.highlight(c.str(0))
                "select" -> s.select(include = c.options?.get("include")?.asArray()?.map { it.asString()!! },
                    exclude = c.options?.get("exclude")?.asArray()?.map { it.asString()!! })
                "limit" -> s.limit(c.num(0).toInt())
                "sort" -> s.sort(*c.args.map { key ->
                    val k = call(key)
                    when (k.op) {
                        "score" -> Sort.score()
                        "pk" -> Sort.pk()
                        "field" -> Sort.field(k.str(0))
                        else -> error("sort key ${k.op} is not driven here")
                    }
                }.toTypedArray())
                "after" -> s.after(c.args[0].asArray()!!)
                "fuse" -> {
                    val f = call(c.args[0])
                    s.fuse(when (f.op) {
                        "rrf" -> if (f.args.isEmpty()) rrf() else rrf(f.num(0).toInt())
                        "dbsf" -> dbsf()
                        "weighted" -> weighted(*f.args.map { (it as Json.JsonNumber).raw.toDouble() }.toDoubleArray())
                        else -> error("fusion ${f.op} is not driven here")
                    })
                }
                "expand" -> s.expand(c.str(0), c.options!!.member("from_field")!!,
                    hops = c.options?.get("hops")?.asLong()?.toInt() ?: 1)
                else -> error("search op ${c.op} is not driven here")
            }
        }
        return s
    }

    private fun camel(snake: String): String =
        snake.split('_').mapIndexed { i, p -> if (i == 0) p else p.replaceFirstChar { it.uppercase() } }.joinToString("")

    // ---- comparison -----------------------------------------------------------------------

    private fun mask(value: JsonValue, volatile: Set<String>): JsonValue = when (value) {
        is Json.JsonObject -> Json.JsonObject(value.entries.filterKeys { it !in volatile }.mapValues { mask(it.value, volatile) })
        is Json.JsonArray -> Json.JsonArray(value.items.map { mask(it, volatile) })
        else -> value
    }

    /** Null when equal; numbers compare to float32 precision. */
    private fun compare(want: JsonValue?, got: JsonValue?, at: String): String? = when (want) {
        is Json.JsonObject -> {
            val g = got as? Json.JsonObject
            if (g == null) "$at: $got is not an object" else
                (want.entries.keys + g.entries.keys).firstNotNullOfOrNull { k -> compare(want[k], g[k], "$at.$k") }
        }
        is Json.JsonArray -> {
            val g = got as? Json.JsonArray
            if (g == null || g.items.size != want.items.size) "$at: $got is not ${want.items.size} items" else
                want.items.indices.firstNotNullOfOrNull { i -> compare(want.items[i], g.items[i], "$at[$i]") }
        }
        is Json.JsonNumber -> {
            val w = want.raw.toDouble()
            val g = (got as? Json.JsonNumber)?.raw?.toDouble()
            if (g != null && Math.abs(w - g) <= 1e-6 * maxOf(1.0, Math.abs(w))) null else "$at: $got, recorded $want"
        }
        else -> if (want == got) null else "$at: $got, recorded $want"
    }

    /** Adds [name] to `sdks/fixtures/results/kotlin.json`'s `ran`, keeping what the report already says. */
    private fun record(name: String) {
        val path = File(root, "sdks/fixtures/results/kotlin.json")
        path.parentFile.mkdirs()
        val existing = path.takeIf { it.isFile }?.let { Json.parse(it.readText())?.asObject() }
        val entries = LinkedHashMap<String, JsonValue>(
            existing?.entries ?: mapOf("language" to Values.str("kotlin"), "transport" to Values.str("connect"))
        )
        val ran = ((entries["ran"]?.asArray()?.mapNotNull { it.asString() } ?: emptyList()) + name).toSortedSet()
        entries["ran"] = Json.JsonArray(ran.map { Values.str(it) })
        path.writeText(Json.writePretty(Json.JsonObject(entries)) + "\n")
    }
}
