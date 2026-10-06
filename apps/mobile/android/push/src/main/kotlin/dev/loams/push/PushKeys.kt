package dev.loams.push

import android.content.Context
import android.util.Base64
import dev.loams.core.push.PushKeyPair
import dev.loams.data.crypto.KeystoreAead

/**
 * One X25519 HPKE key pair per instance (AP2 Task 8). The private key is AES-GCM encrypted with a
 * Keystore key; only its public half is sent to the server.
 */
class PushKeys(context: Context) {
    private val prefs = context.getSharedPreferences("push-keys", Context.MODE_PRIVATE)
    private val aead = KeystoreAead("loams-push-keys")

    /** Synchronized and committed synchronously: the public half goes to the server right after. */
    @Synchronized
    fun ensure(instanceId: String): ByteArray {
        privateKey(instanceId)?.let { return PushKeyPair.publicFromPrivate(it) }
        val kp = PushKeyPair.generate()
        prefs.edit().putString(instanceId, Base64.encodeToString(aead.encrypt(kp.privateKey), Base64.NO_WRAP)).commit()
        return kp.publicKey
    }

    fun privateKey(instanceId: String): ByteArray? =
        prefs.getString(instanceId, null)?.let { runCatching { aead.decrypt(Base64.decode(it, Base64.NO_WRAP)) }.getOrNull() }

    fun delete(instanceId: String) {
        prefs.edit().remove(instanceId).apply()
    }
}
