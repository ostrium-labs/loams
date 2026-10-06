package dev.loams

import com.google.protobuf.ByteString
import com.google.protobuf.Descriptors.Descriptor
import com.google.protobuf.Descriptors.EnumDescriptor
import com.google.protobuf.Descriptors.EnumValueDescriptor
import com.google.protobuf.Descriptors.FieldDescriptor
import com.google.protobuf.DynamicMessage
import com.google.protobuf.Message

/**
 * The proto3 JSON mapping, written out (R10).
 *
 * `protobuf-java-util`'s `JsonFormat` ships a formatter, and it is not what this SDK
 * sends. The formatter emits `{ "field": value }` — a space after the opening brace
 * and around every colon and comma — while the proto3 JSON mapping as the corpus
 * records it is compact:
 * `{"approvalId":"apr_…","revision":"1","decision":"DECISION_KIND_APPROVE"}`.
 * `sdks/conformance/fixture-server.mjs` compares the request bytes it receives
 * against the recording **byte for byte** and answers 400 on a difference, so a
 * formatter's spaces make this SDK fail **every** JSON fixture for a reason that has
 * nothing to do with conformance. Hence this file.
 *
 * # What "the proto3 JSON mapping" means here, precisely
 *
 * Four rules, and each one is a place the mapping is observable in the corpus:
 *
 *  - **compact** — no whitespace between tokens at all;
 *  - **field order by field number**, which is what a writer walking a descriptor
 *    produces and what the recordings have. Insertion order is *not* a substitute:
 *    the recorded `mock_state_stream_resume` step 1 sets `approvalId`, `revision`,
 *    `decision` and `idempotencyKey`, which is field order, and a
 *    `LinkedHashMap` of whatever the caller set last would not be;
 *  - **defaults omitted** — proto3's implicit presence, so an empty
 *    `GetInstanceRequest` is `{}` and not `{"name":""}`;
 *  - **64-bit integers as strings** — `"revision":"1"`, not `"revision":1`, because
 *    a JSON number cannot hold the full range of an `int64`. The corpus depends on
 *    this: `mock_state_idempotent_decide`'s expectation is on `revision: "2"`, a
 *    string.
 *
 * Enums are written by **name**, never by number, because the recordings name them
 * (`"DECISION_KIND_APPROVE"`, `"APPROVAL_STATE_APPROVED"`) and a server rejects a
 * number where the proto3 JSON mapping requires a name.
 *
 * # What is deliberately not here
 *
 * **Well-known types.** `Timestamp`, `Duration`, `Any`, `Struct`, `Value`,
 * `ListValue` and `FieldMask` each have their own JSON form. No Loams facade call
 * *sends* one — `GetInstance`, `DecideApproval`, `WatchApprovals`, `Query`, `Mutate`,
 * `Deploy` — so the gap cannot be reached from the facade. Rather than write a
 * half-correct mapping for types no call sends, [format] **refuses** a message
 * containing one, naming the type. A request that cannot be encoded is a failure at
 * the boundary with a message saying so, which is better than a body the server will
 * reject with a parse error three layers away. Responses go the other way and use
 * [parse], which does understand them.
 *
 * **Extensions and unknown fields.** Neither appears in a request this SDK builds,
 * and writing them would need a registry that has nothing to register.
 */
object CompactJson {

    /**
     * The message's proto3 JSON form: compact, in field-number order, with proto3
     * defaults omitted and 64-bit integers as strings.
     *
     * @throws UnsupportedOperationException the message contains a well-known type,
     *   whose JSON mapping this writer does not implement. Named, rather than
     *   silently mis-encoded.
     */
    fun format(message: DynamicMessage): String {
        val out = StringBuilder()
        writeMessage(out, message)
        return out.toString()
    }

    /** The message's proto3 JSON form, as UTF-8 bytes. */
    fun formatBytes(message: DynamicMessage): ByteArray = format(message).toByteArray(Charsets.UTF_8)

    private fun writeMessage(out: StringBuilder, message: Message) {
        val descriptor = message.descriptorForType
        if (isWellKnown(descriptor.fullName)) {
            throw UnsupportedOperationException(
                "${descriptor.fullName} has its own proto3 JSON mapping, which this writer does not " +
                    "implement; no Loams facade call sends one, and a body the server cannot parse is worse " +
                    "than a refusal at the boundary"
            )
        }

        out.append('{')
        var first = true
        // Field-number order is a property of the descriptor, not of the order the
        // caller happened to set fields in.
        for (field in descriptor.fields.sortedBy { it.number }) {
            if (!shouldWrite(message, field)) {
                continue
            }
            if (!first) {
                out.append(',')
            }
            first = false
            writeString(out, field.jsonName)
            out.append(':')
            writeField(out, field, message, field)
        }
        out.append('}')
    }

    /**
     * Whether a field is written: proto3 implicit presence means a field at its
     * default is absent, and a field with explicit presence (a proto3 `optional`, a
     * message field, a oneof case) is written when it is set.
     *
     * A **repeated** field has no *presence* — it has a count — and
     * `DynamicMessage.hasField` refuses it, so the count is what is asked, and the
     * mapping's default-omitted rule then says an empty repeated field is absent.
     * That is what makes an empty `WatchApprovalsRequest` serialize to `{}` rather
     * than to `{"environments":[],"states":[]}`.
     *
     * Asking `hasField` on a repeated field throws `IllegalArgumentException: field
     * ... has no presence`, and the throw would make any JSON call whose request or
     * **response** carries a repeated field fail outright.
     */
    private fun shouldWrite(message: Message, field: FieldDescriptor): Boolean {
        if (field.containingOneof != null) {
            // A oneof case is written when it is the set one, even at its default
            // value: the case *is* the information.
            return message.hasField(field)
        }
        if (field.isMapField || field.isRepeated) {
            return entries(field, message)
        }
        if (field.hasPresence()) {
            return message.hasField(field)
        }
        if (field.type == FieldDescriptor.Type.MESSAGE || field.type == FieldDescriptor.Type.GROUP) {
            // A message field has presence even when proto3 does not say so: unset
            // is null and set is an instance.
            return message.getField(field) != null
        }
        return !isDefaultValue(field, message.getField(field))
    }

    /**
     * Whether a repeated or map field holds anything.
     *
     * A map field needs no separate arm: to protobuf-java's reflection a map **is** a
     * repeated field of synthesised `key`/`value` entry messages, so `getField` answers
     * both with a list and the count is the same question.
     */
    private fun entries(field: FieldDescriptor, message: Message): Boolean {
        @Suppress("UNCHECKED_CAST")
        val list = message.getField(field) as? List<Any>
        return !list.isNullOrEmpty()
    }

    private fun isDefaultValue(field: FieldDescriptor, value: Any?): Boolean = when (field.type) {
        FieldDescriptor.Type.BOOL -> value != true
        FieldDescriptor.Type.STRING -> (value as? String).isNullOrEmpty()
        FieldDescriptor.Type.BYTES -> (value as? ByteString)?.size() ?: 0 == 0
        FieldDescriptor.Type.ENUM -> when (value) {
            is EnumValueDescriptor -> value.number == 0
            is Int -> value == 0
            is Number -> value.toInt() == 0
            else -> true
        }
        FieldDescriptor.Type.INT32,
        FieldDescriptor.Type.SINT32,
        FieldDescriptor.Type.SFIXED32,
        -> (value as? Int ?: 0) == 0
        FieldDescriptor.Type.UINT32,
        FieldDescriptor.Type.FIXED32,
        -> (value as? Int ?: 0) == 0
        FieldDescriptor.Type.INT64,
        FieldDescriptor.Type.SINT64,
        FieldDescriptor.Type.SFIXED64,
        -> (value as? Long ?: 0L) == 0L
        FieldDescriptor.Type.UINT64,
        FieldDescriptor.Type.FIXED64,
        -> (value as? java.lang.Long ?: 0L) == 0L
        FieldDescriptor.Type.FLOAT -> (value as? Float ?: 0f) == 0f
        FieldDescriptor.Type.DOUBLE -> (value as? Double ?: 0.0) == 0.0
        else -> value == null
    }

    private fun writeField(out: StringBuilder, field: FieldDescriptor, message: Message, entryField: FieldDescriptor) {
        if (field.isMapField) {
            @Suppress("UNCHECKED_CAST")
            val map = message.getField(field) as Map<Any, Any>
            out.append('{')
            var first = true
            // A map's key is a string in proto3 JSON, whatever its declared type,
            // and an `int32` key is written as its decimal text.
            for ((key, value) in map) {
                if (!first) {
                    out.append(',')
                }
                first = false
                writeString(out, key.toString())
                out.append(':')
                writeScalar(out, mapValueField(field), value)
            }
            out.append('}')
            return
        }
        if (field.isRepeated) {
            @Suppress("UNCHECKED_CAST")
            val list = message.getField(field) as List<Any>
            out.append('[')
            var first = true
            for (item in list) {
                if (!first) {
                    out.append(',')
                }
                first = false
                writeScalar(out, field, item)
            }
            out.append(']')
            return
        }
        writeScalar(out, field, message.getField(field))
    }

    /**
     * A map field's value descriptor.
     *
     * A map is not a field type: the proto compiler synthesises a nested
     * `map<K, V>` *message* whose field 1 is the key and whose field 2 is the value,
     * and `FieldDescriptor` has no accessor for the value type. It is read off that
     * synthetic message, which is where protoc put it.
     */
    private fun mapValueField(mapField: FieldDescriptor): FieldDescriptor =
        requireNotNull(mapField.messageType.findFieldByName("value")) {
            "${mapField.fullName} is a map with no value field, which no proto compiler emits"
        }

    private fun writeScalar(out: StringBuilder, field: FieldDescriptor, value: Any?) {
        when (field.type) {
            FieldDescriptor.Type.MESSAGE,
            FieldDescriptor.Type.GROUP,
            -> writeMessage(out, value as Message)

            FieldDescriptor.Type.ENUM -> {
                when (value) {
                    is EnumValueDescriptor -> writeString(out, value.name)
                    is Int -> writeString(out, field.enumType.findValueByNumber(value)?.name ?: value.toString())
                    is Number -> {
                        val num = value.toInt()
                        writeString(out, field.enumType.findValueByNumber(num)?.name ?: num.toString())
                    }
                    else -> writeString(out, value?.toString() ?: "")
                }
            }

            FieldDescriptor.Type.BOOL -> out.append(if (value == true) "true" else "false")

            FieldDescriptor.Type.STRING -> writeString(out, value as? String ?: "")

            FieldDescriptor.Type.BYTES -> writeString(out, Base64.encode((value as? String)?.toByteArray() ?: ByteArray(0)))

            // 64-bit integers are JSON **strings**: a JSON number is a double to most
            // readers and cannot hold an int64's range. The corpus's `revision`
            // expectations are strings for exactly this reason.
            FieldDescriptor.Type.INT64,
            FieldDescriptor.Type.SINT64,
            FieldDescriptor.Type.SFIXED64,
            -> writeString(out, (value as? Long ?: 0L).toString())

            FieldDescriptor.Type.UINT64,
            FieldDescriptor.Type.FIXED64,
            -> writeString(out, java.lang.Long.toUnsignedString(value as? Long ?: 0L))

            FieldDescriptor.Type.INT32,
            FieldDescriptor.Type.SINT32,
            FieldDescriptor.Type.SFIXED32,
            -> out.append((value as? Int ?: 0).toString())

            FieldDescriptor.Type.UINT32,
            FieldDescriptor.Type.FIXED32,
            -> out.append(Integer.toUnsignedString(value as? Int ?: 0))

            FieldDescriptor.Type.FLOAT -> out.append(number((value as? Float ?: 0f).toDouble()))
            FieldDescriptor.Type.DOUBLE -> out.append(number(value as? Double ?: 0.0))

            else -> throw UnsupportedOperationException(
                "field ${field.fullName} is a ${field.type}, which this writer does not implement"
            )
        }
    }

    /**
     * A JSON number for a float or a double.
     *
     * A `NaN` or an infinity has no JSON form and cannot be serialized — named,
     * rather than written as the string `"NaN"`, which no JSON parser would read back
     * as a number.
     */
    fun number(value: Double): String {
        if (value.isNaN()) {
            throw UnsupportedOperationException("a NaN double has no proto3 JSON form and cannot be serialized")
        }
        if (value.isInfinite()) {
            throw UnsupportedOperationException("an infinite double has no proto3 JSON form and cannot be serialized")
        }
        // `%.17g` would print `0.10000000000000001` for `0.1`; the shortest form that
        // round-trips is what a corpus byte-compares against, and Java's `Double`/
        // `Float` `toString` is specified to be exactly that.
        return if (value == Math.floor(value) && !value.isInfinite() && Math.abs(value) < 1e15) {
            value.toLong().toString()
        } else {
            value.toString()
        }
    }

    /** The number of an enum value the descriptor declares, by name. */
    fun enumNumber(descriptor: Descriptor, fieldName: String, enumName: String): Int {
        val field = requireNotNull(descriptor.findFieldByName(fieldName)) {
            "$fieldName is not a field of ${descriptor.fullName}"
        }
        val enumType = requireNotNull(field.enumType) { "$fieldName is not an enum" }
        return requireNotNull(enumType.findValueByName(enumName)) {
            "${enumType.fullName} declares no value $enumName"
        }.number
    }

    /** An enum's value descriptor by name, or null. */
    fun enumValue(enumType: EnumDescriptor, name: String): EnumValueDescriptor? = enumType.findValueByName(name)

    /**
     * Writes a JSON string, escaping exactly what RFC 8259 requires.
     *
     * Written out rather than delegated to [Json] so this file has no dependence on
     * the reader's escape policy changing under it — a body the corpus has never seen
     * is a 400.
     */
    private fun writeString(out: StringBuilder, value: String) {
        out.append('"')
        for (c in value) {
            when {
                c == '"' -> out.append("\\\"")
                c == '\\' -> out.append("\\\\")
                c == '\b' -> out.append("\\b")
                c == '' -> out.append("\\f")
                c == '\n' -> out.append("\\n")
                c == '\r' -> out.append("\\r")
                c == '\t' -> out.append("\\t")
                c.code < 0x20 -> out.append("\\u").append("%04x".format(c.code))
                else -> out.append(c)
            }
        }
        out.append('"')
    }

    private fun isWellKnown(fullName: String): Boolean = fullName in WELL_KNOWN

    /** The well-known types whose own JSON mapping this writer does not implement. */
    private val WELL_KNOWN = setOf(
        "google.protobuf.Timestamp",
        "google.protobuf.Duration",
        "google.protobuf.Any",
        "google.protobuf.Struct",
        "google.protobuf.Value",
        "google.protobuf.ListValue",
        "google.protobuf.FieldMask",
        "google.protobuf.Empty",
    )
}