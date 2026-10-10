package dev.loams.app.session

import dev.loams.push.PushHandler
import dev.loams.transport.Http
import dev.loams.transport.TrustPolicy
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.int
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import okhttp3.Request

/**
 * Debug builds only: stands in for FCM by reading the mock's push log (exactly what a push
 * gateway would forward: routing fields and the sealed payload) and handing each message to the
 * same [PushHandler] a FirebaseMessagingService would call. Runs while the app is open.
 */
class MockPushPoller(private val handler: PushHandler) {
    private val http = Http.client(TrustPolicy.System)

    suspend fun run(issuer: String, token: String) {
        var after = latestSeq(issuer, token) // only pushes that arrive from now on
        while (true) {
            delay(3_000)
            runCatching {
                val log = fetch(issuer, token, after)
                for (entry in log) {
                    val o = entry.jsonObject
                    after = maxOf(after, o["seq"]!!.jsonPrimitive.int)
                    handler.onMessage(o["data"]!!.jsonObject.mapValues { it.value.jsonPrimitive.content })
                }
            }
        }
    }

    private suspend fun latestSeq(issuer: String, token: String): Int =
        runCatching { fetch(issuer, token, 0).maxOfOrNull { it.jsonObject["seq"]!!.jsonPrimitive.int } ?: 0 }.getOrDefault(0)

    private suspend fun fetch(issuer: String, token: String, after: Int) = withContext(Dispatchers.IO) {
        val req = Request.Builder().url("$issuer/mock/push-log?token=$token&after=$after").build()
        http.newCall(req).execute().use { Json.parseToJsonElement(it.body.string()).jsonArray }
    }
}
