package dev.loams

import kotlin.reflect.KClass

import com.google.protobuf.ByteString
import com.google.protobuf.Descriptors.Descriptor
import com.google.protobuf.Descriptors.EnumValueDescriptor
import com.google.protobuf.Descriptors.FieldDescriptor
import com.google.protobuf.DynamicMessage
import com.google.protobuf.Message

/**
 * Serializes and deserializes messages in the codec a client chose.
 *
 * The **write** side is [CompactJson], because the corpus byte-compares request
 * bodies and a generic JSON formatter's spaces are a different request. The **read**
 * side is protobuf's own parser driven by the committed descriptor set, which does
 * understand the well-known types [CompactJson] refuses to write — and it must,
 * because `DecideApprovalResponse.approval.created_at` is a `Timestamp` and the
 * corpus's idempotent-decide fixture is that response.
 */
object MessageCodec {
    /** Serializes a message in the given codec. */
    fun serialize(message: DynamicMessage, codec: Codec): ByteArray = when (codec) {
        Codec.PROTO -> message.toByteArray()
        Codec.JSON -> CompactJson.formatBytes(message)
    }

    /** Deserializes into a fresh instance of the descriptor's message. */
    fun deserialize(descriptor: Descriptor, body: ByteArray, codec: Codec): DynamicMessage = when (codec) {
        Codec.PROTO -> DynamicMessage.parseFrom(descriptor, body)
        Codec.JSON -> parseJson(descriptor, String(body, Charsets.UTF_8))
    }

    /**
     * Reads proto3 JSON into a message of [descriptor].
     *
     * The **liberal** half of the proto3 JSON mapping, where [CompactJson] is the
     * strict one. Both halves are required by the spec and they differ on input that
     * is real: a reader that insisted on the writer's rules would reject a server
     * that sends an unknown field (which is what forward compatibility looks like
     * from here) or an enum number where this SDK knows only a name.
     *
     * Accepted, per the mapping:
     *
     *  - either the proto name (`api_versions`) or the JSON name (`apiVersions`);
     *  - an enum by name or by number, and an unknown number kept as a number;
     *  - a 64-bit integer as a number **or** as a string, which is the mapping's own
     *    allowance and what `revision` arrives as on one server and not another;
     *  - `null` for a field with presence, meaning "not set", and for a scalar;
     *  - a well-known type in its own JSON form, so a `Timestamp` reads as RFC 3339
     *    and not as `{"seconds":…}`;
     *  - base64 **with or without padding** for a `bytes` field, because the wire
     *    carries both and the corpus does.
     *
     * Anything else is refused, naming the field — a half-read response is worse
     * than a failure, because a caller would branch on a field the server never set.
     */
    fun parseJson(descriptor: Descriptor, text: String): DynamicMessage {
        val parsed = Json.parse(text)
            ?: throw IllegalArgumentException("the response body is not JSON, and a message the SDK cannot read is worse than a refusal")
        val builder = DynamicMessage.newBuilder(descriptor)
        readMessage(builder, parsed, descriptor)
        return builder.build()
    }

    private fun readMessage(builder: DynamicMessage.Builder, value: JsonValue, descriptor: Descriptor) {
        val node = value.obj
            ?: throw IllegalArgumentException("${descriptor.fullName} was sent as ${describe(value)}, which is not a JSON object")
        for ((name, member) in node.entries) {
            if (member.isNull()) {
                // `null` clears a field with presence and is ignored on one without;
                // either way it is not an error, which is what the mapping says.
                continue
            }
            val field = find(descriptor, name)
                ?: throw IllegalArgumentException("${descriptor.fullName} has no field $name, and the server sent one")
            readField(builder, field, member)
        }
    }

    private fun find(descriptor: Descriptor, name: String): FieldDescriptor? =
        descriptor.fields.firstOrNull { it.jsonName == name || it.name == name }

    private fun readField(builder: DynamicMessage.Builder, field: FieldDescriptor, value: JsonValue) {
        if (field.isMapField) {
            val node = value.obj
                ?: throw IllegalArgumentException("${field.fullName} was sent as ${describe(value)}, which is not a JSON object")
            // A map field is a *repeated* field of synthesised entry messages to
            // protobuf-java's reflection, so it is read from `getRepeatedFieldCount`
            // rather than from a `Map`, and written with `addRepeatedField`.
            val entryDescriptor = requireNotNull(field.messageType) { "${field.fullName} is a map with no entry type" }
            val keyField = requireNotNull(entryDescriptor.findFieldByName("key"))
            val valueField = requireNotNull(entryDescriptor.findFieldByName("value"))
            for ((key, member) in node.entries) {
                val entry = DynamicMessage.newBuilder(entryDescriptor)
                setField(entry, keyField, Json.JsonString(key))
                if (!member.isNull()) {
                    setField(entry, valueField, member)
                }
                builder.addRepeatedField(field, entry.build())
            }
            return
        }

        if (field.isRepeated) {
            val items = value.array
                ?: throw IllegalArgumentException("${field.fullName} was sent as ${describe(value)}, which is not a JSON array")
            for (item in items) {
                if (item.isNull()) {
                    // A `null` element in a repeated field is not a value: proto3 JSON
                    // writes `null` for an unset element and reading it as one would
                    // put a zero in a list the server left short.
                    continue
                }
                builder.addRepeatedField(field, scalarOf(field, item))
            }
            return
        }

        setField(builder, field, value)
    }

    private fun isWellKnown(type: String): Boolean = type.startsWith("google.protobuf.")

    private fun setField(builder: DynamicMessage.Builder, field: FieldDescriptor, value: JsonValue) {
        when (field.type) {
            FieldDescriptor.Type.MESSAGE,
            FieldDescriptor.Type.GROUP,
            -> {
                val nested = requireNotNull(field.messageType) { "${field.fullName} is a message with no type" }
                val nestedBuilder = DynamicMessage.newBuilder(nested)
                if (isWellKnown(nested.fullName)) {
                    writeWellKnown(nestedBuilder, nested, value)
                } else {
                    readMessage(nestedBuilder, value, nested)
                }
                builder.setField(field, nestedBuilder.build())
            }
            else -> builder.setField(field, scalarOf(field, value))
        }
    }

    /**
     * A well-known type read in its own JSON form.
     *
     * Only the ones that can appear in a Loams response: `Timestamp`, `Duration`,
     * `FieldMask`, `Empty` and `Struct`. `Timestamp` is the one that matters — the
     * approvals a corpus returns carry `created_at` — and it is the RFC 3339 form,
     * which is the only one a Loams server writes for it.
     *
     * Anything else is refused **naming the type**, rather than being parsed as a
     * plain message: `Timestamp` as `{"seconds":…}` parses into a message that reads
     * back as zero, which is a wrong answer with no error.
     */
    private fun writeWellKnown(builder: DynamicMessage.Builder, descriptor: Descriptor, value: JsonValue) {
        when (descriptor.fullName) {
            "google.protobuf.Timestamp" -> {
                val text = value.string
                    ?: throw IllegalArgumentException(
                        "google.protobuf.Timestamp was sent as ${describe(value)}; the proto3 JSON mapping " +
                            "requires the RFC 3339 string form"
                    )
                val parsed = parseRfc3339(text)
                builder.setField(
                    descriptor.findFieldByName("seconds") ?: return,
                    java.lang.Long.valueOf(parsed.first),
                )
                descriptor.findFieldByName("nanos")?.let { builder.setField(it, java.lang.Integer.valueOf(parsed.second)) }
            }
            "google.protobuf.Duration" -> {
                val text = value.string
                    ?: throw IllegalArgumentException("google.protobuf.Duration was sent as ${describe(value)}")
                // The seconds/nanos split is all this SDK's messages ever carry.
                val trimmed = text.removeSuffix("s")
                val dot = trimmed.indexOf('.')
                val seconds = if (dot < 0) trimmed else trimmed.substring(0, dot)
                val nanos = if (dot < 0) 0 else {
                    val fraction = trimmed.substring(dot + 1).padEnd(9, '0').take(9)
                    fraction.toIntOrNull()
                        ?: throw IllegalArgumentException("google.protobuf.Duration $text has a fractional part this reader cannot parse")
                }
                descriptor.findFieldByName("seconds")?.let { builder.setField(it, java.lang.Long.valueOf(seconds.toLongOrNull() ?: 0L)) }
                descriptor.findFieldByName("nanos")?.let { builder.setField(it, java.lang.Integer.valueOf(nanos)) }
            }
            "google.protobuf.FieldMask" -> {
                val paths = value.array
                    ?: throw IllegalArgumentException("google.protobuf.FieldMask was sent as ${describe(value)}")
                builder.setField(
                    descriptor.findFieldByName("paths"),
                    paths.mapNotNull { it.string }.joinToString(",") { it },
                )
            }
            "google.protobuf.Empty" -> Unit
            "google.protobuf.Struct" -> readStruct(builder, requireNotNull(descriptor.findFieldByName("fields")), value)
            else -> throw IllegalArgumentException(
                "${descriptor.fullName} has its own proto3 JSON mapping, which this reader does not " +
                    "implement; a value read as a plain message would read back as a zero the server never sent"
            )
        }
    }

    /** `google.protobuf.Struct`'s `Value` map, which is what every JSON value becomes. */
    private fun readStruct(builder: DynamicMessage.Builder, fields: FieldDescriptor, value: JsonValue) {
        val node = value.obj
            ?: throw IllegalArgumentException("google.protobuf.Struct was sent as ${describe(value)}")
        // `google.protobuf.Struct.fields` is a map whose value type is the `Value`
        // message, and a `Value` names its payload field by **type** — `string_value`,
        // `number_value`, `bool_value`, `struct_value`, `list_value` — so the field to
        // write is chosen by the JSON value's own type. That is why this is a `when`
        // over the value rather than a single `setField`.
        // `fields.messageType` is the synthesised `FieldsEntry`, whose field 2 (`value`)
        // is the `Value` message. A map's entry type is never the value type itself, so
        // the `Value` descriptor is one level further in.
        val entryType = requireNotNull(fields.messageType) { "${fields.fullName} is a map with no entry type" }
        val valueDescriptor = requireNotNull(entryType.findFieldByName("value")).messageType
        for ((key, member) in node.entries) {
            val entry = DynamicMessage.newBuilder(valueDescriptor)
            writeValue(entry, valueDescriptor, member)
            builder.addRepeatedField(fields, stringEntry(fields, key, entry.build()))
        }
    }

    /**
     * A `Struct.fields` entry carrying [key] and [value].
     *
     * Built through the entry message rather than through a `MapField` builder, because
     * `DynamicMessage.Builder` exposes no `putField`: to its reflection a map field is a
     * repeated field of synthesised entry messages, so it is appended like any other
     * repeated value.
     */
    private fun stringEntry(
        mapField: FieldDescriptor,
        key: String,
        value: com.google.protobuf.Message,
    ): com.google.protobuf.Message {
        val entryType = requireNotNull(mapField.messageType) { "${mapField.fullName} is a map with no entry type" }
        return DynamicMessage.newBuilder(entryType)
            .setField(entryType.findFieldByName("key"), key)
            .setField(entryType.findFieldByName("value"), value)
            .build()
    }

    /**
     * Writes a JSON value into a `google.protobuf.Value`, choosing the field by the
     * value's own type.
     */
    private fun writeValue(builder: DynamicMessage.Builder, valueDescriptor: Descriptor, value: JsonValue) {
        val field = requireNotNull(valueDescriptor.findFieldByName(VALUE_FIELD_FOR[value::class])) {
            "google.protobuf.Value declares no ${VALUE_FIELD_FOR[value::class]} field, and the proto3 JSON " +
                "mapping has no other way to write a ${value::class.simpleName}"
        }
        when (value) {
            is Json.JsonNull -> builder.clearField(field)
            is Json.JsonNumber -> builder.setField(field, java.lang.Double.valueOf(value.raw.toDouble()))
            is Json.JsonString -> builder.setField(field, value.value)
            is Json.JsonBoolean -> builder.setField(field, java.lang.Boolean.valueOf(value.value))
            is Json.JsonArray -> {
                val listDescriptor = requireNotNull(field.messageType)
                val listBuilder = DynamicMessage.newBuilder(listDescriptor)
                val values = requireNotNull(listDescriptor.findFieldByName("values"))
                for (item in value.items) {
                    val entry = DynamicMessage.newBuilder(requireNotNull(values.messageType))
                    writeValue(entry, requireNotNull(values.messageType), item)
                    listBuilder.addRepeatedField(values, entry.build())
                }
                builder.setField(field, listBuilder.build())
            }
            is Json.JsonObject -> {
                val structDescriptor = requireNotNull(field.messageType)
                val structBuilder = DynamicMessage.newBuilder(structDescriptor)
                readStruct(structBuilder, requireNotNull(structDescriptor.findFieldByName("fields")), value)
                builder.setField(field, structBuilder.build())
            }
        }
    }

    /**
     * An RFC 3339 timestamp as a `(seconds, nanos)` pair.
     *
     * Parsed by hand rather than with `java.time` because this SDK's floor is a JVM
     * 11 target and the whole of RFC 3339 — offsets, fractional seconds of any
     * length, a leap second — is more than the corpus uses and less than
     * `Instant.parse` accepts. The formats accepted here are exactly the ones the
     * corpus records and what the mapping requires.
     */
    private fun parseRfc3339(text: String): Pair<Long, Int> {
        val pattern = Regex(
            "^(\\d{4})-(\\d{2})-(\\d{2})[Tt](\\d{2}):(\\d{2}):(\\d{2})(\\.(\\d+))?([Zz]|[+-]\\d{2}:\\d{2})$"
        )
        val match = pattern.matchEntire(text)
            ?: throw IllegalArgumentException("google.protobuf.Timestamp $text is not RFC 3339, and the mapping requires it")
        val (year, month, day, hour, minute, second, _, fraction, offset) = match.destructured
        // One `LocalDateTime` rather than a `LocalDate` epoch second minus a
        // `LocalTime` second-of-day: the arithmetic is right and reads as a mistake, and
        // `LocalDate.of` throws on the day/month the regex already accepted.
        var seconds = java.time.LocalDateTime.of(
            year.toInt(), month.toInt(), day.toInt(),
            hour.toInt(), minute.toInt(), second.toInt(),
        ).toEpochSecond(java.time.ZoneOffset.UTC)
        if (offset != "Z" && offset != "z") {
            val sign = if (offset[0] == '-') -1 else 1
            val offsetSeconds = sign * (offset.substring(1, 3).toInt() * 3600 + offset.substring(4, 6).toInt() * 60)
            seconds += offsetSeconds
        }
        val nanos = if (fraction.isEmpty()) {
            0
        } else {
            fraction.padEnd(9, '0').take(9).toIntOrNull()
                ?: throw IllegalArgumentException("google.protobuf.Timestamp $text has a fractional part this reader cannot parse")
        }
        return seconds to nanos
    }

    /** The boxed value protobuf-java wants for a scalar field, from a JSON value. */
    private fun scalarOf(field: FieldDescriptor, value: JsonValue): Any = when (field.type) {
        FieldDescriptor.Type.BOOL -> asBoolean(field, value)
        FieldDescriptor.Type.STRING -> asString(field, value)
        FieldDescriptor.Type.BYTES -> asBytes(field, value)
        FieldDescriptor.Type.ENUM -> asEnum(field, value)
        FieldDescriptor.Type.INT32,
        FieldDescriptor.Type.SINT32,
        FieldDescriptor.Type.SFIXED32,
        -> java.lang.Integer.valueOf(asLong(field, value).toInt())
        FieldDescriptor.Type.UINT32,
        FieldDescriptor.Type.FIXED32,
        -> java.lang.Integer.valueOf(asLong(field, value).toInt())
        FieldDescriptor.Type.INT64,
        FieldDescriptor.Type.SINT64,
        FieldDescriptor.Type.SFIXED64,
        -> java.lang.Long.valueOf(asLong(field, value))
        FieldDescriptor.Type.UINT64,
        FieldDescriptor.Type.FIXED64,
        -> java.lang.Long.valueOf(asLong(field, value))
        FieldDescriptor.Type.FLOAT -> java.lang.Float.valueOf(asDouble(field, value).toFloat())
        FieldDescriptor.Type.DOUBLE -> java.lang.Double.valueOf(asDouble(field, value))
        FieldDescriptor.Type.MESSAGE,
        FieldDescriptor.Type.GROUP,
        -> {
            val nested = requireNotNull(field.messageType)
            val nestedBuilder = DynamicMessage.newBuilder(nested)
            if (isWellKnown(nested.fullName)) {
                writeWellKnown(nestedBuilder, nested, value)
            } else {
                readMessage(nestedBuilder, value, nested)
            }
            nestedBuilder.build()
        }
        else -> throw IllegalArgumentException("field ${field.fullName} is a ${field.type}, which this reader does not implement")
    }

    private fun asBoolean(field: FieldDescriptor, value: JsonValue): Boolean =
        value.boolean
            ?: throw IllegalArgumentException("${field.fullName} was sent as ${describe(value)}, which is not a boolean")

    private fun asString(field: FieldDescriptor, value: JsonValue): String =
        value.string
            ?: throw IllegalArgumentException("${field.fullName} was sent as ${describe(value)}, which is not a string")

    /**
     * A `bytes` field, from standard base64 **with or without** padding.
     *
     * The same unpadded wire value the error path carries, for the same reason: a
     * strict decoder would refuse a `bytes` field a server wrote without padding and
     * report a refusal for a field that was perfectly readable.
     */
    private fun asBytes(field: FieldDescriptor, value: JsonValue): ByteString {
        val text = value.string
            ?: throw IllegalArgumentException("${field.fullName} was sent as ${describe(value)}, which is not base64")
        val bytes = Base64.decode(text)
            ?: throw IllegalArgumentException("${field.fullName} is $text, which is not standard base64")
        return ByteString.copyFrom(bytes)
    }

    private fun asEnum(field: FieldDescriptor, value: JsonValue): EnumValueDescriptor {
        val enumType = requireNotNull(field.enumType) { "${field.fullName} is not an enum" }
        if (value is Json.JsonNumber) {
            val num = value.raw.toIntOrNull()
                ?: throw IllegalArgumentException("${field.fullName} is ${value.raw}, which is not an enum number")
            return enumType.findValueByNumberCreatingIfUnknown(num)
        }
        val name = value.string
            ?: throw IllegalArgumentException("${field.fullName} was sent as ${describe(value)}, which is neither an enum name nor a number")
        val byName = enumType.findValueByName(name)
        if (byName != null) return byName
        val num = name.toIntOrNull()
        if (num != null) {
            return enumType.findValueByNumberCreatingIfUnknown(num)
        }
        throw IllegalArgumentException("${enumType.fullName} declares no value $name, and it is not a number either")
    }

    /** A 64-bit integer, from a JSON number **or** a JSON string. */
    private fun asLong(field: FieldDescriptor, value: JsonValue): Long = when (value) {
        is Json.JsonNumber -> value.raw.toLongOrNull()
            ?: throw IllegalArgumentException("${field.fullName} is ${value.raw}, which is not an integer")
        is Json.JsonString -> value.value.toLongOrNull()
            ?: throw IllegalArgumentException("${field.fullName} is \"${value.value}\", which is not an integer")
        else -> throw IllegalArgumentException("${field.fullName} was sent as ${describe(value)}, which is not a number")
    }

    private fun asDouble(field: FieldDescriptor, value: JsonValue): Double = when (value) {
        is Json.JsonNumber -> value.raw.toDouble()
        is Json.JsonString -> value.value.toDoubleOrNull()
            ?: throw IllegalArgumentException("${field.fullName} is \"${value.value}\", which is not a number")
        else -> throw IllegalArgumentException("${field.fullName} was sent as ${describe(value)}, which is not a number")
    }

    /** The `google.protobuf.Value` field that holds each JSON value's own type. */
    private val VALUE_FIELD_FOR: Map<KClass<out JsonValue>, String> = mapOf(
        Json.JsonNull::class to "null_value",
        Json.JsonNumber::class to "number_value",
        Json.JsonString::class to "string_value",
        Json.JsonBoolean::class to "bool_value",
        Json.JsonArray::class to "list_value",
        Json.JsonObject::class to "struct_value",
    )

    private fun describe(value: JsonValue): String = when (value) {
        is Json.JsonObject -> "an object"
        is Json.JsonArray -> "an array"
        is Json.JsonString -> "a string"
        is Json.JsonNumber -> "the number ${value.raw}"
        is Json.JsonBoolean -> "the boolean $value"
        is Json.JsonNull -> "null"
    }
}