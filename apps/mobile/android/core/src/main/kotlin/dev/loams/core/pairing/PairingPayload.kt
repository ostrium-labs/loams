package dev.loams.core.pairing

import java.net.URI
import java.time.Instant
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.intOrNull
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.longOrNull

/**
 * A v1 pairing payload as a QR code carries it (design §37 §7.2.1).
 *
 * [issuer] is the Loams gateway, not Authentik. [spki] is null for instances with publicly trusted
 * certificates; otherwise the phone pins it before sending a byte. [jkt] is the instance key
 * thumbprint that anchors the instance's identity either way.
 */
data class PairingPayload(
    val issuer: URI,
    val instanceId: String,
    val spki: List<String>?,
    val jkt: String,
    val code: String,
    val userCode: String,
    val exp: Instant,
)

/** Why a scanned text is not a usable pairing payload. */
enum class PairingError {
    /** A `v` this build does not understand; it may carry a field that must not be skipped. */
    UNSUPPORTED_VERSION,

    /** Valid JSON, but another product's QR code (`kind` is not `loams-pair`). */
    WRONG_KIND,

    /** Past `exp`: ask for a fresh code. */
    EXPIRED,

    /** The issuer is not https (or, in debug builds, not a loopback http mock). */
    INSECURE_ISSUER,

    /** Not JSON, or a required field is missing or has the wrong type. */
    MALFORMED,
}

sealed interface PairingResult {
    data class Ok(val payload: PairingPayload) : PairingResult

    data class Refused(val error: PairingError) : PairingResult
}

object PairingPayloads {
    const val KIND = "loams-pair"
    const val VERSION = 1

    /** Hosts a debug build may reach over plain http: the emulator's host alias and loopback. */
    private val LOOPBACK_HOSTS = setOf("10.0.2.2", "127.0.0.1", "localhost", "::1", "[::1]")

    private val json = Json { ignoreUnknownKeys = true }

    /**
     * Reads a scanned QR code or pasted text. Unknown fields are ignored (forward compatible
     * within v1), but `v`, `kind`, `exp` and the issuer scheme are checked strictly: a pairing
     * payload is a credential exchange, not a place to be lenient.
     *
     * @param allowInsecureLoopback debug builds only: accept an `http` issuer on a loopback host,
     *   for the local mock.
     */
    fun parse(text: String, now: Instant, allowInsecureLoopback: Boolean = false): PairingResult {
        val obj = runCatching { json.parseToJsonElement(text.trim()) as? JsonObject }.getOrNull()
            ?: return PairingResult.Refused(PairingError.MALFORMED)
        val kind = obj.string("kind") ?: return PairingResult.Refused(PairingError.MALFORMED)
        if (kind != KIND) return PairingResult.Refused(PairingError.WRONG_KIND)
        val v = (obj["v"] as? JsonPrimitive)?.intOrNull ?: return PairingResult.Refused(PairingError.MALFORMED)
        if (v != VERSION) return PairingResult.Refused(PairingError.UNSUPPORTED_VERSION)

        val issuerText = obj.string("issuer") ?: return PairingResult.Refused(PairingError.MALFORMED)
        val instanceId = obj.string("instance_id") ?: return PairingResult.Refused(PairingError.MALFORMED)
        val jkt = obj.string("jkt") ?: return PairingResult.Refused(PairingError.MALFORMED)
        val code = obj.string("code") ?: return PairingResult.Refused(PairingError.MALFORMED)
        val userCode = obj.string("user_code") ?: return PairingResult.Refused(PairingError.MALFORMED)
        val exp = (obj["exp"] as? JsonPrimitive)?.longOrNull ?: return PairingResult.Refused(PairingError.MALFORMED)
        val spki = when (val s = obj["spki"]) {
            null, is JsonNull -> null
            is JsonArray -> s.map { (it as? JsonPrimitive)?.contentOrNull ?: return PairingResult.Refused(PairingError.MALFORMED) }
            else -> return PairingResult.Refused(PairingError.MALFORMED)
        }
        val issuer = runCatching { URI(issuerText) }.getOrNull()
            ?.takeIf { it.host != null && it.scheme != null }
            ?: return PairingResult.Refused(PairingError.MALFORMED)

        if (!issuerAllowed(issuer, allowInsecureLoopback)) return PairingResult.Refused(PairingError.INSECURE_ISSUER)
        if (!now.isBefore(Instant.ofEpochSecond(exp))) return PairingResult.Refused(PairingError.EXPIRED)

        return PairingResult.Ok(PairingPayload(issuer, instanceId, spki, jkt, code, userCode, Instant.ofEpochSecond(exp)))
    }

    /** Whether the app may talk to [issuer] at all. The same rule applies to typed issuer URLs. */
    fun issuerAllowed(issuer: URI, allowInsecureLoopback: Boolean): Boolean = when (issuer.scheme?.lowercase()) {
        "https" -> true
        "http" -> allowInsecureLoopback && issuer.host?.lowercase() in LOOPBACK_HOSTS
        else -> false
    }

    private fun JsonObject.string(key: String): String? =
        (this[key] as? JsonPrimitive)?.takeIf { it.isString }?.content?.takeIf { it.isNotEmpty() }
}
