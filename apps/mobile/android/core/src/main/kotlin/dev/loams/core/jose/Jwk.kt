package dev.loams.core.jose

import java.security.MessageDigest
import java.security.interfaces.ECPublicKey
import java.util.Base64

/** The JWK shapes the apps handle: EC P-256 (device keys) and OKP Ed25519 (instance keys). */
data class Jwk(val kty: String, val crv: String, val x: String, val y: String? = null) {
    /** RFC 7638 thumbprint: SHA-256 over the required members in byte order, base64url. */
    fun thumbprint(): String {
        val members = when (kty) {
            "EC" -> """{"crv":"$crv","kty":"EC","x":"$x","y":"${y ?: error("EC JWK needs y")}"}"""
            "OKP" -> """{"crv":"$crv","kty":"OKP","x":"$x"}"""
            else -> throw IllegalArgumentException("unsupported kty $kty")
        }
        return b64(MessageDigest.getInstance("SHA-256").digest(members.toByteArray(Charsets.UTF_8)))
    }

    fun toJson(): String = if (y != null) {
        """{"kty":"$kty","crv":"$crv","x":"$x","y":"$y"}"""
    } else {
        """{"kty":"$kty","crv":"$crv","x":"$x"}"""
    }

    companion object {
        fun fromEcPublicKey(key: ECPublicKey): Jwk =
            Jwk("EC", "P-256", b64(fixed(key.w.affineX.toByteArray())), b64(fixed(key.w.affineY.toByteArray())))

        private fun fixed(v: ByteArray): ByteArray {
            val t = v.dropWhile { it.toInt() == 0 }.toByteArray()
            return ByteArray(32 - t.size) + t
        }

        private fun b64(b: ByteArray) = Base64.getUrlEncoder().withoutPadding().encodeToString(b)
    }
}
