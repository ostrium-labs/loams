package dev.loams.test.Support

import dev.loams.Json
import dev.loams.JsonValue

/**
 * One required fixture, as `sdks/fixtures/manifest.json` states it.
 *
 * The manifest is the **authority** (D617) and this is the only thing that reads
 * it on the SDK's behalf. Nothing in this file names a fixture: the list of
 * fixtures a run is judged on is the manifest's, so a fixture the manifest newly
 * requires is in the next run without anybody editing the suite, and a fixture
 * that stops being reachable is *named in the failure* rather than quietly
 * dropped from a hand-written list.
 */
data class RequiredFixture(
    /** The fixture's name, which is what the report's `ran` carries. */
    val name: String,
    /** The recording, relative to the corpus root. */
    val file: String,
    /** Why the corpus records it, or empty. */
    val reason: String,
    /** The runtime-contract clauses it pins, from `pinnedBy`. */
    val clauses: List<String>,
)

/** One recorded step's request. */
data class RecordedRequest(
    val method: String,
    val path: String,
    val contentType: String,
    val body: ByteArray,
) {
    override fun equals(other: Any?): Boolean =
        this === other || (other is RecordedRequest && method == other.method &&
            path == other.path && contentType == other.contentType && body.contentEquals(other.body))

    override fun hashCode(): Int =
        (method.hashCode() * 31 + path.hashCode()) * 31 + contentType.hashCode() * 31 + body.contentHashCode()
}

/** One recorded frame of a stream body, when the recording separates them. */
data class RecordedFrame(val flags: Int, val payload: ByteArray)

/** One recorded step. */
data class RecordedStep(
    /** The fixture's own name, so a failure can name the fixture and the step. */
    val fixtureName: String,
    /** The step number: the index in the recording, which is what the harness's header names. */
    val step: Int,
    val request: RecordedRequest,
    val status: Int,
    val responseContentType: String,
    val responseBody: ByteArray,
    val frames: List<RecordedFrame>?,
    /** The recording's own `expect` block, verbatim. Never interpreted here. */
    val expect: JsonValue?,
)

/**
 * Reading a recorded fixture off disk — the one place the corpus file format is
 * interpreted.
 *
 * ## Why both roles share it
 *
 * Two things read a recording: the **fixture server**, which replays the bytes it
 * finds, and the **corpus driver**, which replays a call and compares the answer
 * to what the recording says. They agreed before, by having two copies of the
 * same reader, and they stopped agreeing the moment the corpus's `body` stopped
 * being always a string: the server read only the string form and silently
 * served an **empty** body for every recorded Connect refusal, so the driver
 * compared a `failed_precondition` the server never sent. A disagreement between
 * what a suite thinks the corpus says and what it was served is the most
 * expensive kind of bug in a conformance harness, so there is one reader.
 *
 * ## The corpus's `body` is a string *or* an object
 *
 * A recorded body is the bytes the server actually sent, and the recorder wrote
 * it as parsed JSON rather than as a string when it was JSON. Both forms are in
 * `sdks/fixtures`: `mock_error_approval_expired` records `request.body` as the
 * string `{"approvalId":"…"}` and `response.body` as an object, while
 * `live_watch` records both as `bodyBase64`. An object body is therefore
 * re-serialized to recover its bytes, and a body that is neither is empty rather
 * than an error — a recording with no body is a recording of an empty body.
 */
object CorpusRecording {
    /** A recorded body's bytes, honouring both the `bodyBase64` and `body` forms. */
    fun bodyOf(holder: JsonValue?): ByteArray {
        val node = holder?.asObject() ?: return ByteArray(0)
        node.string("bodyBase64")?.let { return Base64Lenient.decode(it) ?: ByteArray(0) }
        return when (val body = node["body"]) {
            null, is Json.JsonNull -> ByteArray(0)
            is Json.JsonString -> body.value.toByteArray(Charsets.UTF_8)
            // An object or an array: re-serialized with `writeCompact`, because
            // the bytes a fixture server compares against are the ones in the
            // file, and a re-serialization that reordered or re-escaped a
            // character would make every recorded refusal a mismatch.
            else -> Json.writeCompact(body).toByteArray(Charsets.UTF_8)
        }
    }

    /**
     * One recorded header, by name, case-insensitively.
     *
     * The corpus spells headers lower case, and HTTP header names are
     * case-insensitive, so the lookup cannot be.
     */
    fun headerOf(holder: JsonValue?, name: String): String? =
        holder?.asObject()?.get("headers")?.asObject()?.entries
            ?.firstOrNull { it.key.equals(name, ignoreCase = true) }
            ?.value?.asString()

    /** Every step of one recording, in file order. */
    fun readSteps(fixtureName: String, json: JsonValue): List<RecordedStep> {
        val root = json.asObject()
            ?: throw IllegalArgumentException("$fixtureName is not a JSON object")
        val recorded = root["steps"]?.asArray()?.let { it } ?: listOf(json)
        return recorded.mapIndexed { index, step ->
            val node = step.asObject()
                ?: throw IllegalArgumentException("$fixtureName step $index is not a JSON object")
            val request = node["request"]?.asObject()
                ?: throw IllegalArgumentException("$fixtureName step $index has no request")
            val response = node["response"]?.asObject()
                ?: throw IllegalArgumentException("$fixtureName step $index has no response")
            RecordedStep(
                fixtureName = fixtureName,
                step = index,
                request = RecordedRequest(
                    method = request.string("method") ?: "POST",
                    path = request.string("path") ?: "",
                    contentType = headerOf(request, "content-type") ?: "",
                    body = bodyOf(request),
                ),
                status = response["status"]?.asLong()?.toInt() ?: 0,
                responseContentType = headerOf(response, "content-type") ?: "",
                responseBody = bodyOf(response),
                frames = response["frames"]?.asArray()?.map { frame ->
                    val f = frame.asObject()!!
                    RecordedFrame(
                        flags = f["flags"]?.asLong()?.toInt() ?: 0,
                        payload = Base64Lenient.decode(f.string("payload") ?: "") ?: ByteArray(0),
                    )
                },
                expect = node["expect"],
            )
        }
    }

    /** Every required fixture, read from the manifest and nothing else. */
    fun requiredFixtures(manifest: JsonValue): List<RequiredFixture> {
        val fixtures = manifest.asObject()?.get("fixtures")?.asArray() ?: emptyList()
        return fixtures.mapNotNull { fixture ->
            val node = fixture.asObject() ?: return@mapNotNull null
            if (node["required"]?.asBoolean() != true) {
                return@mapNotNull null
            }
            RequiredFixture(
                name = node.string("name") ?: return@mapNotNull null,
                file = node.string("file") ?: return@mapNotNull null,
                reason = node.string("reason") ?: "",
                clauses = node["pinnedBy"]?.asArray()?.mapNotNull { it.asString() } ?: emptyList(),
            )
        }
    }
}

/**
 * Base64 for reading a **recording**, which is not the wire.
 *
 * This is the recording side, and it is deliberately not `dev.loams.Base64`: the
 * recordings are `node`'s own `JSON.stringify(Buffer)` output, so they are
 * always padded, while the *wire* carries unpadded values that 7 of the corpus's
 * 10 distinct `details[].value` strings show (D653). Two decoders with two jobs
 * would be worse than one, so this one is the JDK's and the SDK's is the strict
 * one. It is here because a test reading a recording must not depend on the
 * SDK's decoder being right.
 */
object Base64Lenient {
    /** Decodes standard base64, or returns null when the JDK refuses it. */
    fun decode(text: String): ByteArray? = try {
        java.util.Base64.getDecoder().decode(text)
    } catch (_: IllegalArgumentException) {
        null
    }
}