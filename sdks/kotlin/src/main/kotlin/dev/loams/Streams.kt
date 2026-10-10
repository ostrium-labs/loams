package dev.loams

import com.google.protobuf.Descriptors.FieldDescriptor
import com.google.protobuf.DynamicMessage

/**
 * A cursor a stream resumes from: an opaque server-issued position (R7).
 *
 * @property value the cursor's own encoding, which the server defines and the SDK treats
 *   as opaque.
 */
data class StreamCursor(val value: String) {
    /** Whether this cursor names a position at all. */
    val isEmpty: Boolean get() = value.isEmpty()

    companion object {
        /** The empty cursor, meaning "from the start". */
        val NONE: StreamCursor = StreamCursor("")
    }
}

/**
 * How one stream reconnects: how the cursor is read off a message, and how a request is
 * rewritten to carry it.
 *
 * Deliberately **not** generic over the message types. A generic parameter here would
 * force every caller to name both types at the call site and would put the descriptor
 * types into the runtime's signatures; the two methods below carry the same information
 * with a typed body, so the compiler still checks that the cursor comes from a response
 * and the resume produces a request.
 */
interface StreamResume {
    /**
     * How many reconnects this stream may make. Negative means the client's own default,
     * so a caller who did not think about a budget inherits it rather than silently
     * getting zero reconnects.
     */
    val maxRetries: Int

    /** The cursor of a message the caller has applied, or an empty cursor. */
    fun cursorOf(message: DynamicMessage): StreamCursor

    /** The request to re-open with, carrying [cursor]. */
    fun resume(cursor: StreamCursor, request: DynamicMessage): DynamicMessage
}

/**
 * A resume policy over one RPC's own schema, built without the caller naming a type.
 *
 * This is what a caller writes, because the two field names — `cursor` on the response,
 * `resume_cursor` on the request — come from the **descriptor** rather than from the
 * caller. A proto that renames either changes this policy, and a caller who wrote a
 * lambda against the old name gets a refusal at construction rather than a stream that
 * silently never resumes.
 *
 * @property binding the RPC, which is where both field names are read from.
 * @property cursorField the response field carrying the cursor.
 * @property requestField the request field the cursor goes into.
 * @property maxRetries how many reconnects this stream may make; -1 means the client's.
 */
class SchemaStreamResume(
    val binding: CallBinding,
    val cursorField: FieldDescriptor,
    val requestField: FieldDescriptor,
    override val maxRetries: Int = -1,
) : StreamResume {
    init {
        require(cursorField.containingType == binding.response) {
            "${cursorField.fullName} is not a field of ${binding.response.fullName}, which is " +
                "${binding.rpc}'s response"
        }
        require(requestField.containingType == binding.request) {
            "${requestField.fullName} is not a field of ${binding.request.fullName}, which is " +
                "${binding.rpc}'s request"
        }
    }

    override fun cursorOf(message: DynamicMessage): StreamCursor {
        if (message.descriptorForType != binding.response) {
            // A message of another type is not a caller error at this seam — a stream
            // could yield a type the policy was not written for if a caller wired it
            // across two RPCs — and treating it as "no cursor" would resume from nothing
            // and silently duplicate. Refusing names both types.
            throw ErrorMapper.internal(
                binding.rpc,
                "the resume policy for ${binding.rpc} reads ${binding.response.name} but was handed a " +
                    "${message.descriptorForType.name}",
            )
        }
        return StreamCursor(message.getField(cursorField).toString())
    }

    override fun resume(cursor: StreamCursor, request: DynamicMessage): DynamicMessage {
        if (request.descriptorForType != binding.request) {
            throw ErrorMapper.internal(
                binding.rpc,
                "the resume policy for ${binding.rpc} rewrites ${binding.request.name} but was handed a " +
                    "${request.descriptorForType.name}",
            )
        }
        return DynamicMessage.newBuilder(request.descriptorForType)
            .mergeFrom(request.toByteString())
            .setField(requestField, cursor.value)
            .build()
    }

    companion object {
        /**
         * A policy for an RPC whose response carries `cursor` and whose request accepts
         * `resumeCursor`, both read off the descriptor.
         *
         * @throws IllegalArgumentException the RPC declares neither field, which is a
         *   proto that cannot resume and a caller who should find out at construction.
         */
        fun forRpc(rpc: String, maxRetries: Int = -1): SchemaStreamResume {
            val binding = requireNotNull(Descriptors.bindingForRpc(rpc)) {
                "the committed descriptor set declares no RPC '$rpc'"
            }
            return forBinding(binding, maxRetries)
        }

        /** As [forRpc], for a binding the caller already has. */
        fun forBinding(binding: CallBinding, maxRetries: Int = -1): SchemaStreamResume {
            val cursorField = binding.response.fields.firstOrNull { it.jsonName == "cursor" || it.name == "cursor" }
                ?: throw IllegalArgumentException(
                    "${binding.response.fullName} declares no `cursor` field, so ${binding.rpc} cannot resume " +
                        "from one and a resume policy for it would silently re-read from the start"
                )
            val requestField = binding.request.fields.firstOrNull {
                it.jsonName == "resumeCursor" || it.name == "resume_cursor"
            }
                ?: throw IllegalArgumentException(
                    "${binding.request.fullName} declares no `resume_cursor` field, so ${binding.rpc} has " +
                        "nowhere to put the cursor it would resume from"
                )
            return SchemaStreamResume(binding, cursorField, requestField, maxRetries)
        }
    }
}

/**
 * One server stream, and the counts a caller needs about it.
 *
 * A handle rather than a bare sequence because two of R7's clauses are about things the
 * sequence alone cannot say: how many heartbeats the server sent
 * (`mock_state_stream_heartbeat` records two frames and one message, and only the second
 * number is visible from the sequence), and how many times the stream re-opened (the
 * whole of "resume from the cursor without re-yielding"). [messages] is still an
 * ordinary Kotlin [Sequence], so a caller who does not care about either writes a `for`
 * loop.
 *
 * @property messages the data messages, reconnecting from the last applied cursor when
 *   the caller asked for it (R7).
 */
class ServerStreamHandle internal constructor(
    val messages: Sequence<DynamicMessage>,
    private val heartbeatsSupplier: () -> Int,
    private val reconnectsSupplier: () -> Int,
    val frameKinds: List<String>,
) {
    constructor(
        messages: Sequence<DynamicMessage>,
        heartbeats: Int,
        reconnects: Int,
        frameKinds: List<String>,
    ) : this(messages, { heartbeats }, { reconnects }, frameKinds)

    /** How many heartbeat frames the server sent. */
    val heartbeats: Int get() = heartbeatsSupplier()

    /** How many times the stream re-opened from its cursor. */
    val reconnects: Int get() = reconnectsSupplier()
}