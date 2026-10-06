package dev.loams.test.Support

import com.google.protobuf.ByteString
import com.google.protobuf.Descriptors.Descriptor
import com.google.protobuf.Descriptors.EnumValueDescriptor
import com.google.protobuf.Descriptors.FieldDescriptor
import com.google.protobuf.DynamicMessage
import com.google.protobuf.Message
import dev.loams.Base64
import dev.loams.Code
import dev.loams.CompactJson
import dev.loams.Descriptors
import dev.loams.Json
import dev.loams.JsonValue
import dev.loams.LoamsException
import dev.loams.Reason
import dev.loams.ReasonRegistry

/**
 * What a recording says must be true of the SDK's answer, and the reader for
 * each of those facts.
 *
 * Split out of `CorpusDriver` so the driver is the thing that *replays* and this
 * is the thing that *judges*: a failure in here is a disagreement about what the
 * recording claims, and a failure in the driver is a disagreement about what got
 * sent. One file with both makes a red run ambiguous.
 *
 * # Every field is read out of the descriptor, never off a generated property
 *
 * The corpus names response fields in proto3 JSON (`apiVersions`, `snapshotReset`)
 * and the descriptors are proto3, so the **JSON name** is the one the corpus and
 * `fixture-server.mjs` agree on. That is also why an enum is compared **by name**
 * and a 64-bit integer **as a string**: an enum is a number in memory and a name
 * in the recording, so comparing the number would pass for every approval state in
 * the proto and fail for every reason that matters.
 *
 * # A field named in `expect` may live one level down
 *
 * `mock_state_idempotent_decide` records `expect.state` and `expect.revision` for a
 * `DecideApprovalResponse`, and neither is a field of that message — they are
 * fields of the `approval` it wraps. A reader that looks only at the top level
 * reports "the response has no state" against a response that plainly has one.
 *
 * So the lookup is: the response's own descriptor first, then the first singular
 * message field of the response — in field-number order, which is the order the
 * proto declares them — that declares the wanted name. That is derived from the
 * response message, never from a list of wrapper field names, so a new response
 * that wraps its subject differently is handled by the same rule (D654).
 */
object Expectations {

    /** Keys of an `expect` block that are **not** response fields. */
    private val STRUCTURAL_KEYS = setOf(
        "status", "reason", "grpcStatus", "identicalToStep", "frames", "frameKinds",
        "code", "message", "about", "httpStatus", "transport", "note",
    )

    /**
     * The scalar field names an `expect` block may be **compared** on.
     *
     * Every recording's `expect` names only a handful, and this is the set this
     * checker compares. It is not a list of fixture names, and it is not the
     * reachability set below — reachability is computed per message from the
     * descriptor, which is what makes a corpus that grows a field this corpus has
     * never seen *pass* rather than fail spuriously.
     *
     * Every entry is a scalar or an enum, or a repeated field compared for
     * containment: a message field has no single value to compare against, so a
     * recording that wanted one would have to name the thing it wants out of it.
     */
    private val READ_BACK_FIELDS = listOf(
        "apiVersions", "state", "revision", "cursor", "snapshotReset",
        "instanceId", "name", "edition", "serverVersion", "features", "setupRequired",
        "kind", "reason", "nextPageToken", "pageSize", "approvals", "devices",
        "operations", "notifications", "id", "operationId", "promiseId", "summary",
        "hint", "environment", "namespace", "project", "version", "package",
        "available", "unstable", "services", "idempotencyKey", "snapshot", "upsert",
        "remove", "heartbeat", "requestId", "fieldViolations",
    )

    /**
     * Every scalar field name reachable from [message], by proto name and by JSON
     * name.
     *
     * Derived from the **schema**, by walking the message and every message it
     * reaches, skipping well-known types (which render as their own JSON form and
     * are never named by an `expect` block here). It answers "is this name a field
     * anywhere in this response's shape", which is a question about protos and not
     * about the corpus.
     */
    private fun reachableScalarNames(message: DynamicMessage): Set<String> {
        val out = mutableSetOf<String>()
        val seen = mutableSetOf<String>()
        val queue = ArrayDeque<Descriptor>()
        queue.add(message.descriptorForType)
        while (queue.isNotEmpty()) {
            val descriptor = queue.removeFirst()
            if (!seen.add(descriptor.fullName)) continue
            for (field in descriptor.fields) {
                val type = field.type
                if (type == FieldDescriptor.Type.MESSAGE || type == FieldDescriptor.Type.GROUP) {
                    val nested = field.messageType ?: continue
                    if (!nested.fullName.startsWith("google.protobuf.")) {
                        queue.add(nested)
                    }
                    continue
                }
                out.add(field.jsonName)
                out.add(field.name)
            }
        }
        return out
    }

    /** The two refusals R8 distinguishes, checked from the recording's own words. */
    fun checkError(fixtureName: String, step: Int, expect: JsonValue?, error: LoamsException): List<String> {
        val node = expect?.asObject() ?: return emptyList()
        val problems = mutableListOf<String>()

        val reason = node["reason"]
        if (reason is Json.JsonNull) {
            // `reason: null` is a recorded fact: the server sent no `ErrorInfo`
            // and the SDK must not invent one (R8, and `mock_status_unauthenticated`).
            if (error.reason != Reason.NONE) {
                problems.add(
                    "$fixtureName step $step: expect.reason is null, and the SDK reported " +
                        ReasonRegistry.name(error.reason)
                )
            }
        } else if (reason != null) {
            val want = reason.asString()
            val got = if (error.reason == Reason.NONE) error.unknownReason else ReasonRegistry.name(error.reason)
            if (want != null && got != want) {
                problems.add(
                    "$fixtureName step $step: expect.reason is $want, and the SDK reported ${got ?: "none"}"
                )
            }
        }

        node["grpcStatus"]?.asLong()?.let { want ->
            val wantCode = Code.fromNumber(want.toInt())
            if (error.code != wantCode) {
                problems.add(
                    "$fixtureName step $step: expect.grpcStatus is $want, and the SDK reported ${error.code}"
                )
            }
        }
        return problems
    }

    /** Checks a step the SDK answered rather than refused. */
    fun checkSuccess(
        fixtureName: String,
        step: Int,
        status: Int,
        expect: JsonValue?,
        response: DynamicMessage?,
        expected: Boolean,
    ): List<String> {
        val problems = mutableListOf<String>()
        if (!expected) {
            problems.add(
                "$fixtureName step $step: the recording answers HTTP $status and the SDK returned a message, " +
                    "so a refusal was read as a success"
            )
            return problems
        }
        if (response == null) {
            problems.add("$fixtureName step $step: the recording answers HTTP $status and the SDK returned nothing")
            return problems
        }
        problems.addAll(checkMessage(fixtureName, step, expect, response))
        return problems
    }

    /**
     * The response fields the recording's `expect` names.
     *
     * The reachability sweep runs **first** and reports every key it cannot read,
     * rather than skipping it. A recording that names a field nothing can reach
     * would otherwise be silently unchecked, and a checker that silently skips what
     * it does not understand is the one way a corpus driver can go green without
     * testing anything.
     */
    fun checkMessage(fixtureName: String, step: Int, expect: JsonValue?, message: DynamicMessage): List<String> {
        val node = expect?.asObject() ?: return emptyList()
        val problems = mutableListOf<String>()
        val reachable = reachableScalarNames(message)

        for (key in node.entries.keys) {
            if (key in STRUCTURAL_KEYS) continue
            if (key in reachable) continue
            problems.add(
                "$fixtureName step $step: expect.$key is set, and neither " +
                    "${message.descriptorForType.name} nor any message it reaches declares a $key field"
            )
        }

        for (name in READ_BACK_FIELDS) {
            val want = node[name] ?: continue
            if (name !in reachable) {
                // Already reported by the sweep above; do not say it twice.
                continue
            }
            val holder = holder(message, name)
            if (holder == null) {
                problems.add(
                    "$fixtureName step $step: expect.$name is set, and neither " +
                        "${message.descriptorForType.name} nor the message it wraps declares a $name field"
                )
                continue
            }
            val wantArray = want.asArray()
            if (wantArray != null) {
                // A repeated field: the recording states which entries must be
                // **present** — the mock serves five api versions and names one —
                // so containment is the check and equality would be wrong.
                val actual = readRepeated(holder, name)
                for (entry in wantArray) {
                    val text = entry.asString() ?: continue
                    if (text !in actual) {
                        problems.add(
                            "$fixtureName step $step: expect.$name contains $text, and the response has [$actual]"
                        )
                    }
                }
                continue
            }
            val got = render(holder, name)
            val claim = wanted(want)
            if (got != claim) {
                problems.add(
                    "$fixtureName step $step: expect.$name is $claim, and the response says ${got ?: "none"}"
                )
            }
        }
        return problems
    }

    /**
     * The value a recording states, rendered the way [render] renders the SDK's.
     *
     * Almost nothing, and the one thing is a boolean: a JSON `true` renders as
     * `true`, and a recording writes `snapshotReset: false` in lower case, as does
     * proto3 JSON.
     */
    private fun wanted(value: JsonValue): String = when (value) {
        is Json.JsonBoolean -> if (value.value) "true" else "false"
        is Json.JsonNull -> ""
        is Json.JsonString -> value.value
        is Json.JsonNumber -> value.raw
        else -> Json.writeCompact(value)
    }

    /**
     * The message a named field lives on: this one, or the first singular message
     * field it wraps that declares the name.
     */
    private fun holder(message: DynamicMessage, jsonName: String): DynamicMessage? {
        if (find(message.descriptorForType, jsonName) != null) {
            return message
        }
        for (field in message.descriptorForType.fields.sortedBy { it.number }) {
            val type = field.type
            if (type != FieldDescriptor.Type.MESSAGE ||
                type == FieldDescriptor.Type.GROUP ||
                field.isRepeated
            ) {
                // A repeated field is a collection of messages rather than one
                // message this response is *about*, so descending into it would be
                // reading some other response's subject. Singular fields only.
                continue
            }
            val nested = field.messageType ?: continue
            if (find(nested, jsonName) == null) continue
            @Suppress("UNCHECKED_CAST")
            val value = message.getField(field) as? Message ?: continue
            return DynamicMessage.newBuilder(nested).mergeFrom(value.toByteString()).build()
        }
        return null
    }

    private fun find(descriptor: Descriptor, jsonName: String): FieldDescriptor? =
        descriptor.fields.firstOrNull { it.jsonName == jsonName || it.name == jsonName }

    /**
     * One field, rendered the way the recording renders it: an enum by name, a
     * 64-bit integer as a string, a bool as `true`/`false`.
     *
     * `null` means "declared with presence, and unset" — a proto3 `optional`, a
     * oneof case that is not this one, or a message field that was never set. A
     * field with **implicit** presence has no such question to ask, so its value
     * is rendered either way and a default compares as the default — which is the
     * honest answer for an `expect.state` the response does not carry, and a far
     * better one than reporting that the field is missing.
     */
    private fun render(message: DynamicMessage, jsonName: String): String? {
        val field = find(message.descriptorForType, jsonName) ?: return null
        if (field.isMap) return ""
        if (field.isRepeated) return readRepeated(message, jsonName).joinToString(", ")
        if (field.hasPresence && !message.hasField(field)) return null

        val raw = message.getField(field)
        return when (field.type) {
            FieldDescriptor.Type.BOOL -> if (raw == true) "true" else "false"
            FieldDescriptor.Type.STRING -> raw as? String
            FieldDescriptor.Type.ENUM -> enumName(field, raw)
            FieldDescriptor.Type.UINT64,
            FieldDescriptor.Type.FIXED64,
            FieldDescriptor.Type.UINT32,
            FieldDescriptor.Type.FIXED32,
            -> (raw as Number).toString()
            FieldDescriptor.Type.INT64,
            FieldDescriptor.Type.SFIXED64,
            FieldDescriptor.Type.INT32,
            FieldDescriptor.Type.SFIXED32,
            FieldDescriptor.Type.SINT32,
            FieldDescriptor.Type.SINT64,
            -> (raw as Number).toString()
            FieldDescriptor.Type.DOUBLE -> CompactJson.number(raw as Double)
            FieldDescriptor.Type.FLOAT -> CompactJson.number((raw as Float).toDouble())
            FieldDescriptor.Type.BYTES -> String(Base64.encode((raw as ByteString).toByteArray()), Charsets.UTF_8)
            FieldDescriptor.Type.MESSAGE,
            FieldDescriptor.Type.GROUP,
            -> (raw as Message).descriptorForType.name
            else -> raw?.toString()
        }
    }

    /** A repeated field's values, rendered as the recording renders them. */
    private fun readRepeated(message: DynamicMessage, jsonName: String): List<String> {
        val field = find(message.descriptorForType, jsonName) ?: return emptyList()
        @Suppress("UNCHECKED_CAST")
        val items = message.getField(field) as? List<Any> ?: return emptyList()
        return items.map { item ->
            if (field.type == FieldDescriptor.Type.ENUM) enumName(field, item) else item?.toString() ?: ""
        }
    }

    /**
     * An enum field's value as the name the recording spells it.
     *
     * Through the descriptor, and never by casting the value: a `DynamicMessage`
     * boxes its own enum number rather than an `EnumValueDescriptor`, so the cast
     * throws. A number the descriptor does not declare — an enum value from a
     * newer server than this SDK was generated from — is rendered as its decimal
     * text, which is what proto3 JSON permits and what keeps an unknown value from
     * reading as a missing field.
     */
    private fun enumName(field: FieldDescriptor, value: Any?): String {
        val number = when (value) {
            null -> 0
            is EnumValueDescriptor -> value.number
            else -> (value as Number).toInt()
        }
        return field.containingType?.findValueByNumber(number)?.name ?: number.toString()
    }

    /** Whether every key an `expect` block names is reachable from [message]. */
    fun unreadableKeys(expect: JsonValue?, message: DynamicMessage): List<String> {
        val node = expect?.asObject() ?: return emptyList()
        val reachable = reachableScalarNames(message)
        return node.entries.keys.filter { it !in STRUCTURAL_KEYS && it !in reachable }
    }

    /** The descriptor of a binding's response, for a caller's own assertions. */
    fun responseDescriptorOf(fullName: String): Descriptor? = Descriptors.message(fullName)
}