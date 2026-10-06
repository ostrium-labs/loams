package dev.loams.transport

import com.connectrpc.getOrThrow
import dev.loams.core.jose.Jwk
import dev.loams.proto.loams.instance.v1.GetInstanceRequest
import dev.loams.proto.loams.instance.v1.GetInstanceResponse
import java.io.IOException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import okhttp3.OkHttpClient
import okhttp3.Request

/** The instance is not the one the QR code named: a hard stop (§37 §7.2.3). */
class InstanceIdentityException(message: String) : Exception(message)

/**
 * Before any credential is sent: `GetInstance` must name the expected instance id, and the
 * instance's JWKS must contain the pinned key thumbprint (`jkt`). This holds even with a
 * publicly trusted certificate, so a swapped server with a valid certificate is still caught.
 */
object InstanceCheck {
    suspend fun verify(clients: Clients, http: OkHttpClient, expectedInstanceId: String, jkt: String): GetInstanceResponse {
        val info = clients.instance.getInstance(GetInstanceRequest.getDefaultInstance()).getOrThrow()
        if (info.instanceId != expectedInstanceId) {
            throw InstanceIdentityException("This server is a different Loams instance than the code you scanned.")
        }
        val thumbprints = fetchThumbprints(http, info.jwksUri)
        if (jkt !in thumbprints) {
            throw InstanceIdentityException("This server's identity changed: its signing key is not the one the code named.")
        }
        return info
    }

    suspend fun fetchThumbprints(http: OkHttpClient, jwksUri: String): Set<String> = withContext(Dispatchers.IO) {
        Http.withDeadline(http).newCall(Request.Builder().url(jwksUri).build()).execute().use { resp ->
            if (!resp.isSuccessful) throw IOException("JWKS answered ${resp.code}")
            val keys = Json.parseToJsonElement(resp.body.string()).jsonObject["keys"]?.jsonArray.orEmpty()
            keys.mapNotNull { k ->
                val o = k as? JsonObject ?: return@mapNotNull null
                runCatching {
                    Jwk(o["kty"]!!.jsonPrimitive.content, o["crv"]!!.jsonPrimitive.content, o["x"]!!.jsonPrimitive.content, o["y"]?.jsonPrimitive?.content).thumbprint()
                }.getOrNull()
            }.toSet()
        }
    }
}
