package dev.loams.push

import android.content.Context
import java.util.UUID

/** Where a push service delivers to this device. */
interface PushRegistrar {
    val provider: String

    suspend fun token(): String
}

/**
 * Stub for Firebase Cloud Messaging: returns a stable fake token, so registration, sealing and
 * display can be exercised against the mock. The real one calls
 * `FirebaseMessaging.getInstance().token` and needs the Firebase project and
 * `google-services.json` that come with the store accounts (TODO(Q420)).
 */
class FcmStubRegistrar(private val context: Context) : PushRegistrar {
    override val provider: String = "fcm"

    override suspend fun token(): String {
        val prefs = context.getSharedPreferences("push-stub", Context.MODE_PRIVATE)
        prefs.getString("token", null)?.let { return it }
        val t = "stub-fcm-" + UUID.randomUUID()
        prefs.edit().putString("token", t).apply()
        return t
    }
}
