package dev.loams.core.auth

import java.security.MessageDigest
import java.security.SecureRandom
import java.util.Base64

/** PKCE (RFC 7636) with S256, for browser sign-in at Authentik. */
data class Pkce(val verifier: String, val challenge: String) {
    val method: String get() = "S256"

    companion object {
        fun generate(random: SecureRandom = SecureRandom()): Pkce {
            val bytes = ByteArray(32).also(random::nextBytes)
            return fromVerifier(b64(bytes))
        }

        fun fromVerifier(verifier: String): Pkce {
            require(verifier.length in 43..128) { "a PKCE verifier is 43 to 128 characters" }
            return Pkce(verifier, b64(MessageDigest.getInstance("SHA-256").digest(verifier.toByteArray(Charsets.US_ASCII))))
        }

        private fun b64(b: ByteArray) = Base64.getUrlEncoder().withoutPadding().encodeToString(b)
    }
}
