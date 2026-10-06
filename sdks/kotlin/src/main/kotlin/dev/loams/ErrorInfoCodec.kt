package dev.loams

import com.google.protobuf.ByteString
import com.google.protobuf.Descriptors.Descriptor
import com.google.protobuf.DynamicMessage
import com.google.protobuf.InvalidProtocolBufferException

/**
 * The Connect code, the canonical classification of a failure. It does not change
 * within an API major version.
 *
 * The numbers are the gRPC status numbers, because a gRPC-Web trailers frame carries
 * one and the mapping between the two vocabularies has to be somewhere. The
 * `snake_case` names are the Connect ones, because a Connect error body's `code` is
 * one of those. Keeping both on one enum is what stops the mapping being a second
 * table somewhere.
 */
enum class Code(val number: Int) {
    /** The operation was cancelled, typically by the caller. */
    CANCELLED(1),

    /** An error whose code the server did not say, or a failure from below the API. */
    UNKNOWN(2),

    /** The client specified an invalid argument. */
    INVALID_ARGUMENT(3),

    /** The deadline passed before the operation could complete. */
    DEADLINE_EXCEEDED(4),

    /** Some requested entity was not found. */
    NOT_FOUND(5),

    /** The entity that a client attempted to create already exists. */
    ALREADY_EXISTS(6),

    /** The caller does not have permission to execute the specified operation. */
    PERMISSION_DENIED(7),

    /** Some resource has been exhausted, perhaps a per-user quota. */
    RESOURCE_EXHAUSTED(8),

    /** The operation was rejected because the system is not in the state it requires. */
    FAILED_PRECONDITION(9),

    /** The operation was aborted, typically due to a concurrency issue. */
    ABORTED(10),

    /** The operation was attempted past the valid range. */
    OUT_OF_RANGE(11),

    /** The operation is not implemented or is not supported in this build. */
    UNIMPLEMENTED(12),

    /** An internal error. A bug in the server or the SDK. */
    INTERNAL(13),

    /** The service is currently unavailable. */
    UNAVAILABLE(14),

    /** Unrecoverable data loss or corruption. */
    DATA_LOSS(15),

    /** The request does not have valid authentication credentials. */
    UNAUTHENTICATED(16);

    /** The Connect `snake_case` wire name for this code. */
    val wireName: String
        get() = name.lowercase()

    companion object {
        private val BY_WIRE: Map<String, Code> = buildMap {
            // `Code.entries` spelled out: inside `buildMap` the receiver's own `entries`
            // resolves first and `code` would be a `Map.Entry`.
            for (code in Code.entries) {
                put(code.wireName, code)
            }
            // Both spellings, because Connect servers in the wild send either and a
            // refusal that mapped to `unknown` would lose the class a caller can act
            // on for the sake of two letters.
            put("cancelled", CANCELLED)
        }

        private val BY_NUMBER: Map<Int, Code> = Code.entries.associateBy { it.number }

        /** The code a Connect wire name names, or [UNKNOWN]. */
        fun fromWire(wire: String): Code = BY_WIRE[wire] ?: UNKNOWN

        /** The code a gRPC status number names, or [UNKNOWN]. */
        fun fromNumber(status: Int): Code = BY_NUMBER[status] ?: UNKNOWN

        /** The Connect wire name a code's number names. */
        fun wireNameOf(status: Int): String = fromNumber(status).wireName

        /**
         * The code an HTTP status implies, for the one place there is no Connect code to
         * read.
         *
         * A gRPC-Web body that is not framed at all is answered here — a proxy, or a
         * server that does not speak gRPC-Web. The mapping is the **conventional** one
         * (`client-error` codes to 4xx, `server-error` codes to 5xx) and it is a guess,
         * which is why it is only reached when there is no envelope to read and
         * [Code.UNKNOWN] is the alternative: a 404 that reported `unknown` would leave a
         * caller unable to tell a missing resource from a broken proxy.
         */
        fun fromHttpStatus(status: Int): Code = when (status) {
            400 -> INVALID_ARGUMENT
            401 -> UNAUTHENTICATED
            403 -> PERMISSION_DENIED
            404 -> NOT_FOUND
            409 -> ALREADY_EXISTS
            412 -> FAILED_PRECONDITION
            429 -> RESOURCE_EXHAUSTED
            499 -> CANCELLED
            500 -> INTERNAL
            501 -> UNIMPLEMENTED
            503 -> UNAVAILABLE
            504 -> DEADLINE_EXCEEDED
            else -> when {
                status in 400..499 -> INVALID_ARGUMENT
                status in 500..599 -> INTERNAL
                else -> UNKNOWN
            }
        }
    }
}

/**
 * The fields of `loams.errors.v1.ErrorInfo` the runtime reads: the stable cause, the
 * structured context and the caller's next step.
 *
 * @property reason the stable cause, `snake_case`. Empty when the detail carried none.
 * @property metadata structured context, for example `{"variant": "standard"}`. Never secrets.
 * @property hint a short next step in the caller's locale.
 */
data class ErrorInfoShape(
    val reason: String,
    val metadata: Map<String, String>,
    val hint: String,
)

/**
 * Reads the `loams.errors.v1.ErrorInfo` detail out of the two shapes the wire uses,
 * so the rest of the SDK never sees either.
 *
 * Two shapes, one answer. A Connect JSON error body carries `details[]` as
 * `{"type", "value"}` with `value` base64 of the serialized detail. A gRPC or gRPC-Web
 * refusal carries the same detail inside `grpc-status-details-bin`, which is base64 of
 * a `google.rpc.Status` whose `details[]` are `google.protobuf.Any`. The lookup is by
 * type URL in both cases, **never by position**.
 */
object ErrorInfoCodec {
    /** The type name of the detail this SDK reads. */
    const val ERROR_INFO_TYPE: String = "loams.errors.v1.ErrorInfo"

    /** The type URL a server puts in an `Any` for that detail. */
    const val ERROR_INFO_TYPE_URL: String = "type.googleapis.com/loams.errors.v1.ErrorInfo"

    private val errorInfoDescriptor: Descriptor by lazy {
        requireNotNull(Descriptors.message(ERROR_INFO_TYPE)) {
            "the committed descriptor set carries no $ERROR_INFO_TYPE, so a refusal's reason cannot be read. " +
                "Regenerate gen/loams-descriptor.binpb with './build.sh descriptors'."
        }
    }

    /**
     * Encodes an `ErrorInfo` with the given fields, as its serialized bytes.
     *
     * Used by the SDK's own tests to build a refusal a server would send, and by
     * nothing on a caller's path. Field order follows the descriptor, so the bytes are
     * the ones protoc would emit — which is what makes a test's `not_implemented`
     * detail byte-identical to the corpus's.
     */
    fun encode(reason: String, metadata: Map<String, String>, hint: String): ByteArray {
        val builder = DynamicMessage.newBuilder(errorInfoDescriptor)
        builder.setField(errorInfoDescriptor.findFieldByName("reason"), reason)
        builder.setField(errorInfoDescriptor.findFieldByName("hint"), hint)
        val mapField = errorInfoDescriptor.findFieldByName("metadata")
        val entryType = mapField.messageType
        val keyField = entryType.findFieldByName("key")
        val valueField = entryType.findFieldByName("value")
        for ((key, value) in metadata) {
            val entry = DynamicMessage.newBuilder(entryType)
                .setField(keyField, key)
                .setField(valueField, value)
                .build()
            builder.addRepeatedField(mapField, entry)
        }
        return builder.build().toByteArray()
    }

    /**
     * The `ErrorInfo` a Connect detail list carries, looked up **by type**.
     *
     * The value is decoded by [Base64.decode], and it is unpadded-tolerant for the
     * reason `Base64` gives: 7 of the 10 distinct `details[].value` strings in
     * `sdks/fixtures` are unpadded. A value that is not base64 at all is reported as
     * "no `ErrorInfo`" and the search moves on, which is what R8 asks for: a refusal
     * whose reason this runtime cannot read is still a typed refusal, not an
     * exception.
     */
    fun fromDetails(details: List<ConnectDetail>): ErrorInfoShape? {
        for (detail in details) {
            // `endsWith` rather than `==`: some servers put the bare name in `type` and
            // others the fully-qualified URL, and refusing the second spelling would
            // lose a reason for the sake of a prefix.
            if (!detail.type.endsWith(ERROR_INFO_TYPE)) {
                continue
            }
            val bytes = Base64.decode(detail.value)
            if (bytes == null) {
                // Base64 the server did not write. "No ErrorInfo" is the honest
                // answer; the alternative is a Loams failure with no reason.
                continue
            }
            val decoded = decode(bytes)
            if (decoded != null) {
                return decoded
            }
        }
        return null
    }

    /**
     * Decodes a detail from its serialized bytes, or null when the bytes are not an
     * `ErrorInfo` this runtime can read.
     *
     * Reported as "no `ErrorInfo`" rather than thrown: the alternative is a Loams
     * failure with no reason and no hint, which is the one thing R8 says must not
     * happen. A **malformed** body is different from an *absent* one, and this is where
     * that difference is enforced — bytes that do not decode are not a detail at all,
     * so a caller is told the reason is unknown rather than handed a partial one.
     */
    fun decode(bytes: ByteArray): ErrorInfoShape? = try {
        val message = DynamicMessage.parseFrom(errorInfoDescriptor, bytes)
        ErrorInfoShape(
            reason = message.getField(errorInfoDescriptor.findFieldByName("reason")).toString(),
            metadata = readMap(message, errorInfoDescriptor.findFieldByName("metadata")),
            hint = message.getField(errorInfoDescriptor.findFieldByName("hint")).toString(),
        )
    } catch (refused: InvalidProtocolBufferException) {
        null
    }

    /**
     * The detail whose type URL is [ERROR_INFO_TYPE_URL], searched across the `Any`
     * values a `google.rpc.Status` carries.
     *
     * `google.rpc.Status` is read **by hand** rather than through a generated type,
     * because no proto in this repository declares it and adding one for a message that
     * only ever appears inside an error trailer would be a second source of truth for
     * the wire format. Only the two fields this reads are walked — `code` and `details`
     * — and an unknown field is skipped rather than refused, because a server is free
     * to add one.
     */
    fun fromStatusBytes(statusBytes: ByteArray): ErrorInfoShape? {
        if (statusBytes.isEmpty()) return null
        val reader = StatusBytesReader(statusBytes)
        try {
            while (reader.hasMore()) {
                when (reader.tag()) {
                    // repeated google.protobuf.Any details
                    3 -> {
                        val shape = fromAnyBytes(reader.lengthDelimited())
                        if (shape != null) {
                            return shape
                        }
                    }
                    // int32 code, string message, and anything a newer server adds
                    else -> reader.skipField()
                }
            }
        } catch (_: Exception) {
            // Malformed status bytes, try direct decode
        }
        val direct = decode(statusBytes)
        return if (direct != null && direct.reason.isNotEmpty()) direct else null
    }

    private fun fromAnyBytes(anyBytes: ByteArray): ErrorInfoShape? {
        val reader = StatusBytesReader(anyBytes)
        var typeUrl: String? = null
        var value: ByteArray? = null
        while (reader.hasMore()) {
            when (reader.tag()) {
                1 -> typeUrl = String(reader.lengthDelimited(), Charsets.UTF_8) // string type_url
                2 -> value = reader.lengthDelimited() // bytes value
                else -> reader.skipField()
            }
        }
        if (typeUrl == null || value == null || !typeUrl.endsWith(ERROR_INFO_TYPE_URL)) {
            return null
        }
        return decode(value)
    }

    private fun readMap(message: DynamicMessage, field: com.google.protobuf.Descriptors.FieldDescriptor?): Map<String, String> {
        if (field == null) return emptyMap()
        @Suppress("UNCHECKED_CAST")
        val entries = message.getField(field) as? List<DynamicMessage> ?: return emptyMap()
        val entryDescriptor = field.messageType
        val keyField = entryDescriptor.findFieldByName("key")
        val valueField = entryDescriptor.findFieldByName("value")
        val out = LinkedHashMap<String, String>()
        for (entry in entries) {
            out[entry.getField(keyField).toString()] = entry.getField(valueField).toString()
        }
        return out
    }
}