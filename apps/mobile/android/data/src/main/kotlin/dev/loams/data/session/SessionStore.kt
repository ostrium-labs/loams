package dev.loams.data.session

import android.content.Context
import android.util.Base64
import androidx.datastore.core.DataStore
import androidx.datastore.preferences.core.Preferences
import androidx.datastore.preferences.core.edit
import androidx.datastore.preferences.core.stringPreferencesKey
import androidx.datastore.preferences.preferencesDataStore
import dev.loams.data.crypto.KeystoreAead
import dev.loams.transport.TrustPolicy
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.map

private val Context.sessionStore: DataStore<Preferences> by preferencesDataStore(name = "session")

/** What survives a restart about the paired instance. Nothing secret is stored in plaintext. */
data class SessionRecord(
    val issuer: String,
    val instanceId: String,
    val jkt: String,
    val spki: Set<String>?,
    val deviceId: String,
    val method: String,
) {
    val trust: TrustPolicy get() = spki?.let { TrustPolicy.Pinned(it) } ?: TrustPolicy.System
}

/**
 * One paired instance (multi-instance is a later task). The refresh token is AES-GCM encrypted
 * with a Keystore key; the app is excluded from backups, so neither leaves the device.
 */
class SessionStore(private val context: Context) {
    private val aead = KeystoreAead("loams-tokens")

    val record: Flow<SessionRecord?> = context.sessionStore.data.map { p ->
        val issuer = p[ISSUER] ?: return@map null
        SessionRecord(
            issuer = issuer,
            instanceId = p[INSTANCE] ?: return@map null,
            jkt = p[JKT].orEmpty(),
            spki = p[SPKI]?.takeIf { it.isNotEmpty() }?.split(',')?.toSet(),
            deviceId = p[DEVICE].orEmpty(),
            method = p[METHOD].orEmpty(),
        )
    }

    suspend fun current(): SessionRecord? = record.first()

    suspend fun save(record: SessionRecord, refreshToken: String) {
        val sealed = Base64.encodeToString(aead.encrypt(refreshToken.toByteArray()), Base64.NO_WRAP)
        context.sessionStore.edit { p ->
            p[ISSUER] = record.issuer
            p[INSTANCE] = record.instanceId
            p[JKT] = record.jkt
            p[SPKI] = record.spki?.joinToString(",").orEmpty()
            p[DEVICE] = record.deviceId
            p[METHOD] = record.method
            p[REFRESH] = sealed
        }
    }

    suspend fun saveRefreshToken(refreshToken: String) {
        val sealed = Base64.encodeToString(aead.encrypt(refreshToken.toByteArray()), Base64.NO_WRAP)
        context.sessionStore.edit { it[REFRESH] = sealed }
    }

    suspend fun refreshToken(): String? {
        val sealed = context.sessionStore.data.first()[REFRESH] ?: return null
        return runCatching { String(aead.decrypt(Base64.decode(sealed, Base64.NO_WRAP))) }.getOrNull()
    }

    suspend fun clear() {
        context.sessionStore.edit { it.clear() }
    }

    private companion object {
        val ISSUER = stringPreferencesKey("issuer")
        val INSTANCE = stringPreferencesKey("instance_id")
        val JKT = stringPreferencesKey("jkt")
        val SPKI = stringPreferencesKey("spki")
        val DEVICE = stringPreferencesKey("device_id")
        val METHOD = stringPreferencesKey("method")
        val REFRESH = stringPreferencesKey("refresh_sealed")
    }
}
