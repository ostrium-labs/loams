package dev.loams

import com.google.protobuf.ByteString
import com.google.protobuf.Descriptors.Descriptor
import com.google.protobuf.Descriptors.FieldDescriptor
import com.google.protobuf.DynamicMessage
import java.security.SecureRandom

/**
 * Idempotency keys (design §44 §7.4, D610; runtime contract R3).
 *
 * A mutating call that carries an `idempotency_key` field is given one **per logical
 * call**, before the first attempt, and **the same key goes out on every retry**. A
 * key regenerated per attempt turns one write into two, which is the exact failure the
 * field exists to prevent, and the corpus's `mock_state_idempotent_decide` is the
 * recording of it: the same `DecideApproval` sent twice with the same key against a
 * mock whose approval state really moved, answering identical bytes and leaving the
 * approval at revision 2 the second time.
 *
 * # Why the schema, not the object
 *
 * Whether to key is asked of the request's **descriptor**, not of the object a caller
 * happened to build. That matters for proto3 `optional`: `MutateRequest.idempotency_key`
 * is `optional string`, so a caller who leaves it out sends no key at all, the
 * mutation is not retryable, and an SDK that inspected the object would have to guess
 * whether a null means "not declared" or "not set". It also must not confuse
 * `MutateRequest` (which has the field) with `DeployRequest` (which does not), or it
 * would invent a field the schema does not declare — and a request with a field the
 * server does not know is rejected.
 *
 * # UUIDv7 rather than a dependency
 *
 * An idempotency key has to be unique across every client that has ever talked to an
 * instance **and** sort by creation time, so an operator can correlate one in a log.
 * `UUID.randomUUID()` is a v4 and is what a Kotlin developer reaches for by default,
 * which is exactly why it is not what this uses: thirty lines of layout are better
 * than a key an operator cannot read a time out of.
 */
object Idempotency {
    /** The field name D610 names. Asked of the schema, never of the object. */
    const val KEY_FIELD: String = "idempotency_key"

    /**
     * A request the runtime has decided to key, and whether it made that decision.
     *
     * @property request the message to send: a copy with the key set, or the original
     *   when nothing was set.
     * @property keyed whether the message carries a key the retry policy may rely on.
     */
    data class KeyedRequest(val request: DynamicMessage, val keyed: Boolean)

    /**
     * Whether a request type's schema declares [KEY_FIELD].
     *
     * Resolved from the descriptor rather than from a property, so the answer is about
     * the **schema**: a proto3 `optional string` and a plain `string` are the same field
     * as far as this is concerned, and a type whose key field is not a string is not a
     * key field at all.
     */
    fun schemaHasKey(descriptor: Descriptor): Boolean {
        val field = descriptor.findFieldByName(KEY_FIELD) ?: return false
        return field.type == FieldDescriptor.Type.STRING
    }

    /**
     * Sets the key on a message whose schema declares one, and says whether the result
     * is keyed.
     *
     * @param request the caller's message. Never mutated: see below.
     * @param supplied the caller's own key, or empty to have one minted. Supplying one
     *   makes the retry yours rather than the SDK's, which is right when the key is what
     *   your own storage dedupes on. Ignored when [mint] is false.
     * @param mint whether the SDK may put a key on this call at all (D610). False means
     *   the request goes out exactly as the caller built it and the call is reported
     *   unkeyed — which is **not** the same answer as "this schema has no key field",
     *   and the two were previously conflated into one. A caller who has already deduped
     *   this operation elsewhere, or who is deliberately replaying a recorded request,
     *   needs the first; a message whose schema has no key field needs the second.
     *
     * The caller's message is **not** mutated. A message the caller still holds is a
     * message they might reuse for a second logical call, and stamping this call's key
     * onto it would make two calls share one key — the same class of bug as regenerating
     * a key per attempt, and harder to see. So the message is cloned first, which for a
     * descriptor-driven message means a round trip through its own serialization: a deep
     * copy with no reflection, and cheap next to an RPC.
     *
     * The clone is skipped entirely on the [mint] `false` and already-populated paths,
     * which are the two that return the message without changing it.
     */
    fun apply(request: DynamicMessage, supplied: String, mint: Boolean = true): KeyedRequest {
        // The schema decides whether a key is even possible. This is asked before
        // `mint`, because "this message cannot be keyed" and "this call must not be
        // keyed" are independent: a caller may pass either flag for a type with no key
        // field and both answers are "unkeyed", and neither should be reported as an
        // error.
        val field = request.descriptorForType.findFieldByName(KEY_FIELD)
        if (field == null || field.type != FieldDescriptor.Type.STRING) {
            // A message without the field is left exactly as the caller wrote it:
            // keying it would invent a field the schema does not declare.
            return KeyedRequest(request, keyed = false)
        }

        // A key the caller already wrote is honoured whatever `mint` says. They set it
        // deliberately — it is what their own storage dedupes on — so stripping it to
        // honour `mint: false` would silently change the meaning of a call the caller
        // has already made idempotent.
        val existing = request.getField(field) as? String
        if (!existing.isNullOrEmpty()) {
            return KeyedRequest(request, keyed = true)
        }

        if (!mint) {
            return KeyedRequest(request, keyed = false)
        }

        val key = if (supplied.isNotEmpty()) supplied else UuidV7.new()
        // One builder, one clone: the clone and the set are the same object, so there
        // is no second serialization round trip to get wrong.
        val clone = DynamicMessage.newBuilder(request.descriptorForType)
            .mergeFrom(request.toByteString())
            .setField(field, key)
            .build()
        return KeyedRequest(clone, keyed = true)
    }
}

/**
 * UUIDv7 as the canonical lowercase hyphenated string: 48 bits of Unix milliseconds,
 * 4 bits of version, 12 bits of a counter within the millisecond, 2 bits of variant,
 * 62 random bits.
 *
 * Hand-written rather than taken from a package, for the same reason the Go SDK writes
 * its own: thirteen SDKs need the identical layout, and the alternative is a dependency
 * whose only job is thirty lines. The clock is the wall clock rather than a monotonic
 * one because the field is a timestamp an operator reads out of a log, not a duration.
 */
object UuidV7 {
    private val random = SecureRandom()

    /** The 12 bits of randomness behind the counter within a millisecond. */
    private val counterLock = Any()
    private var lastMillis = -1L
    private var counter = 0

    /** A fresh UUIDv7, lowercase and hyphenated. */
    fun new(): String {
        val bytes = ByteArray(16)
        random.nextBytes(bytes)

        val millis = System.currentTimeMillis()

        // Monotonic within this process: two keys minted in the same millisecond would
        // otherwise share their timestamp, which is legal for a UUID but makes the sort
        // order — the reason for choosing v7 — a coin flip. The clock is read inside the
        // lock so the read-modify-write cannot interleave.
        val slot: Pair<Long, Int> = synchronized(counterLock) {
            var stamp = millis
            var index: Int
            if (stamp > lastMillis) {
                lastMillis = stamp
                index = random.nextInt(0x1000)
            } else {
                stamp = lastMillis
                index = (counter + 1) and 0x0FFF
                if (index == 0) {
                    // The counter wrapped, so the millisecond is spent: move the clock
                    // forward by one rather than reusing a timestamp, which would put two
                    // keys with the same sort position in a log.
                    lastMillis += 1
                    stamp = lastMillis
                }
                counter = index
            }
            stamp to index
        }
        val (stamp, index) = slot

        // Big-endian for the timestamp, read out with shifts rather than by dividing:
        // dividing keeps the fractional bits of the lower digits and truncates the
        // carry, which puts the wrong byte in.
        bytes[0] = (stamp ushr 40).toByte()
        bytes[1] = (stamp ushr 32).toByte()
        bytes[2] = (stamp ushr 24).toByte()
        bytes[3] = (stamp ushr 16).toByte()
        bytes[4] = (stamp ushr 8).toByte()
        bytes[5] = stamp.toByte()

        bytes[6] = ((index ushr 8) or 0x70).toByte() // version 7
        bytes[7] = index.toByte()
        bytes[8] = ((bytes[8].toInt() and 0x3F) or 0x80).toByte() // variant 10

        // Written group by group rather than by computing each index's offset: the four
        // groups are 8-4-4-12 hex digits, and the arithmetic that maps a byte index to a
        // character index is the kind of thing that is right until the layout changes.
        val text = CharArray(36)
        hex(bytes, 0, 4, text, 0)
        text[8] = '-'
        hex(bytes, 4, 2, text, 9)
        text[13] = '-'
        hex(bytes, 6, 2, text, 14)
        text[18] = '-'
        hex(bytes, 8, 2, text, 19)
        text[23] = '-'
        hex(bytes, 10, 6, text, 24)
        return String(text)
    }

    private const val HEX = "0123456789abcdef"

    private fun hex(bytes: ByteArray, from: Int, count: Int, into: CharArray, at: Int) {
        for (index in 0 until count) {
            val b = bytes[from + index].toInt() and 0xFF
            into[at + index * 2] = HEX[b ushr 4]
            into[at + index * 2 + 1] = HEX[b and 0x0F]
        }
    }

    /**
     * The Unix milliseconds a UUIDv7 encodes, or null for anything that is not one.
     *
     * The timestamp is the first **twelve** hex digits, not eight: 48 bits, and
     * milliseconds since the epoch use 41 of them. Reading eight digits returns a number
     * around 2^25, which is January 1970; reading sixteen silently includes the version
     * and the counter. Exposed because "is this key from before or after that incident"
     * is the question an operator actually asks of one.
     */
    fun readMillis(value: String): Long? {
        if (value.length != 36 ||
            value[8] != '-' || value[13] != '-' || value[18] != '-' || value[23] != '-'
        ) {
            return null
        }
        if (value[14] != '7') {
            return null
        }
        if (value[19] !in "89ab") {
            return null
        }
        var millis = 0L
        for (index in 0 until 12) {
            // The twelve digits are the first 8 (characters 0..7) and then the next 4
            // (characters 9..12): the hyphen at 8 is where the fourth digit would be, so
            // each index past it shifts by one.
            val at = if (index < 8) index else index + 1
            val digit = hexValue(value[at]) ?: return null
            millis = (millis shl 4) or digit.toLong()
        }
        return millis
    }

    private fun hexValue(character: Char): Int? = when (character) {
        in '0'..'9' -> character - '0'
        in 'a'..'f' -> character - 'a' + 10
        else -> null
    }
}