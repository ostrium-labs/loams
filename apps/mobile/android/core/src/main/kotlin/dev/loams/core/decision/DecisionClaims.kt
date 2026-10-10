package dev.loams.core.decision

import java.io.ByteArrayOutputStream

enum class Decision(val wire: String) {
    APPROVE("approve"),
    REJECT("reject"),
}

/**
 * What a decision proof signs (design §37 §7.3, AP0 Ruling 7). The server rebuilds these from the
 * request and compares bytes, so the encoding is fixed: keys in byte order, no whitespace,
 * integers in decimal, strings escaped as RFC 8785 does. The golden cases shared with iOS and the
 * mock are in conformance/fixtures/decision/claims.json.
 *
 * [revision] is unsigned on the wire (uint64); it is held in a Long and printed unsigned.
 */
data class DecisionClaims(
    val approvalId: String,
    val revision: Long,
    val decision: Decision,
    val iat: Long,
    val jti: String,
) {
    fun canonicalJson(): ByteArray {
        val out = StringBuilder()
        out.append("{\"approval_id\":").append(quote(approvalId))
        out.append(",\"decision\":").append(quote(decision.wire))
        out.append(",\"iat\":").append(iat)
        out.append(",\"jti\":").append(quote(jti))
        out.append(",\"revision\":").append(java.lang.Long.toUnsignedString(revision))
        out.append('}')
        return out.toString().toByteArray(Charsets.UTF_8)
    }

    companion object {
        /** RFC 8785 string escaping. */
        fun quote(s: String): String {
            val b = StringBuilder(s.length + 2).append('"')
            for (c in s) {
                when (c) {
                    '"' -> b.append("\\\"")
                    '\\' -> b.append("\\\\")
                    '\b' -> b.append("\\b")
                    '\u000c' -> b.append("\\f")
                    '\n' -> b.append("\\n")
                    '\r' -> b.append("\\r")
                    '\t' -> b.append("\\t")
                    else -> if (c < ' ') b.append("\\u%04x".format(c.code)) else b.append(c)
                }
            }
            return b.append('"').toString()
        }
    }
}

/** Compact JWS (RFC 7515) assembly for ES256, given a signer that produces DER or raw signatures. */
object Jws {
    const val DECISION_HEADER = """{"alg":"ES256","typ":"loams-decision+jws"}"""

    /** The bytes to sign: base64url(header) "." base64url(payload). */
    fun signingInput(headerJson: String, payload: ByteArray): String =
        b64(headerJson.toByteArray(Charsets.UTF_8)) + "." + b64(payload)

    fun compact(signingInput: String, rawSignature: ByteArray): String = signingInput + "." + b64(rawSignature)

    fun b64(bytes: ByteArray): String = java.util.Base64.getUrlEncoder().withoutPadding().encodeToString(bytes)
}

/**
 * Converts a DER ECDSA signature (what `SHA256withECDSA` returns, on Android Keystore too) to the
 * fixed 64-byte r||s form that JWS ES256 requires (RFC 7518 §3.4).
 */
object EcdsaSignatures {
    /** @throws IllegalArgumentException for anything that is not a well-formed DER ECDSA signature. */
    fun derToRaw(der: ByteArray, size: Int = 32): ByteArray {
        try {
            var i = 0
            require(der.size >= 8 && der[i++].toInt() == 0x30) { "not a DER sequence" }
            val first = der[i++].toInt() and 0xff
            val bodyLength = if (first and 0x80 == 0) {
                first
            } else {
                val n = first and 0x7f
                require(n in 1..2) { "unsupported DER length" }
                var len = 0
                repeat(n) { len = (len shl 8) or (der[i++].toInt() and 0xff) }
                len
            }
            require(i + bodyLength == der.size) { "DER length does not match the input" }
            val r = readInt(der, i).also { i = it.second }.first
            val s = readInt(der, i).also { i = it.second }.first
            require(i == der.size) { "trailing bytes after the DER signature" }
            val out = ByteArrayOutputStream(size * 2)
            out.write(fixed(r, size))
            out.write(fixed(s, size))
            return out.toByteArray()
        } catch (e: IndexOutOfBoundsException) {
            throw IllegalArgumentException("truncated DER signature", e)
        }
    }

    private fun readInt(der: ByteArray, start: Int): Pair<ByteArray, Int> {
        var i = start
        require(der[i++].toInt() == 0x02) { "not a DER integer" }
        val len = der[i++].toInt() and 0xff
        require(len in 1..(der.size - i)) { "DER integer runs past the input" }
        return der.copyOfRange(i, i + len) to i + len
    }

    private fun fixed(v: ByteArray, size: Int): ByteArray {
        val trimmed = v.dropWhile { it.toInt() == 0 }.toByteArray()
        require(trimmed.size <= size) { "integer too large" }
        return ByteArray(size - trimmed.size) + trimmed
    }
}
