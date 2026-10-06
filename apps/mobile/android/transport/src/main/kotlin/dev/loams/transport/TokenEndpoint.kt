package dev.loams.transport

import dev.loams.core.Loams
import dev.loams.core.errors.Reason
import java.io.IOException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.longOrNull
import okhttp3.FormBody
import okhttp3.OkHttpClient
import okhttp3.Request

/** Tokens issued to this device (design §37 §7.2.2). The access token stays in memory only. */
data class DeviceTokens(val accessToken: String, val refreshToken: String, val expiresInSeconds: Long, val deviceId: String) {
    override fun toString() = "DeviceTokens(device=$deviceId, expiresIn=$expiresInSeconds, tokens=<redacted>)"
}

/** The device fields every grant that creates a device carries. */
data class DeviceRegistration(
    val deviceName: String,
    val decisionJwkJson: String,
    val model: String = "",
    val appVersion: String = "",
    val platform: String = "android",
    val clientId: String = Loams.CLIENT_ID,
)

sealed interface TokenResult {
    data class Ok(val tokens: DeviceTokens) : TokenResult

    data class Refused(val error: String, val reason: Reason, val description: String?) : TokenResult
}

/**
 * The Loams gateway's OAuth token endpoint, `POST {issuer}/api/v1/oauth/token`: the pairing
 * extension grant, the RFC 8693 exchange of an Authentik token, and refresh.
 *
 * TODO(auth plan, Q438): every call carries a DPoP proof (RFC 9449) from dpop-<instance>.
 */
class TokenEndpoint(private val issuer: String, http: OkHttpClient) {
    private val http = Http.withDeadline(http)
    private val json = Json { ignoreUnknownKeys = true }

    /** Redeems a pairing with exactly one of [code] (from the QR) or [userCode] (typed). */
    suspend fun redeemPairing(code: String?, userCode: String?, device: DeviceRegistration): TokenResult {
        require((code == null) != (userCode == null)) { "send exactly one of code or user_code" }
        return post(
            device.fields() + listOfNotNull(
                "grant_type" to Loams.PAIRING_GRANT,
                code?.let { "code" to it },
                userCode?.let { "user_code" to it },
            ),
        )
    }

    /** Exchanges an Authentik access token for device-bound Loams tokens (RFC 8693). */
    suspend fun exchange(authentikAccessToken: String, device: DeviceRegistration): TokenResult = post(
        device.fields() + listOf(
            "grant_type" to Loams.TOKEN_EXCHANGE_GRANT,
            "subject_token" to authentikAccessToken,
            "subject_token_type" to "urn:ietf:params:oauth:token-type:access_token",
        ),
    )

    suspend fun refresh(refreshToken: String): TokenResult =
        post(listOf("grant_type" to "refresh_token", "refresh_token" to refreshToken, "client_id" to Loams.CLIENT_ID))

    private fun DeviceRegistration.fields() = listOf(
        "client_id" to clientId,
        "device_name" to deviceName,
        "platform" to platform,
        "model" to model,
        "app_version" to appVersion,
        "decision_jwk" to decisionJwkJson,
    )

    private suspend fun post(fields: List<Pair<String, String>>): TokenResult = withContext(Dispatchers.IO) {
        val body = FormBody.Builder().apply { fields.forEach { (k, v) -> add(k, v) } }.build()
        val request = Request.Builder().url(issuer.trimEnd('/') + "/api/v1/oauth/token").post(body).build()
        http.newCall(request).execute().use { resp ->
            val text = resp.body.string()
            val obj = runCatching { json.parseToJsonElement(text) as JsonObject }.getOrNull()
                ?: throw IOException("token endpoint answered ${resp.code} without JSON")
            if (resp.isSuccessful) {
                TokenResult.Ok(
                    DeviceTokens(
                        accessToken = obj.str("access_token") ?: throw IOException("no access_token"),
                        refreshToken = obj.str("refresh_token").orEmpty(),
                        expiresInSeconds = obj["expires_in"]?.jsonPrimitive?.longOrNull ?: 0,
                        deviceId = obj.str("device_id").orEmpty(),
                    ),
                )
            } else {
                TokenResult.Refused(
                    obj.str("error") ?: "http_${resp.code}",
                    // Only the machine field; error_description is free text.
                    Reason.fromWire(obj.str("loams_reason")),
                    obj.str("error_description"),
                )
            }
        }
    }

    private fun JsonObject.str(k: String) = this[k]?.jsonPrimitive?.content
}
