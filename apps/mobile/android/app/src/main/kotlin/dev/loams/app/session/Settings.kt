package dev.loams.app.session

import android.content.Context
import androidx.datastore.preferences.core.booleanPreferencesKey
import androidx.datastore.preferences.core.edit
import androidx.datastore.preferences.core.stringPreferencesKey
import androidx.datastore.preferences.preferencesDataStore
import dev.loams.app.BuildConfig
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.map

private val Context.settings by preferencesDataStore(name = "settings")

/** Non-secret preferences. */
class Settings(private val context: Context) {
    /** The server the welcome screen offers: the local mock in debug builds. */
    val serverUrl: Flow<String> = context.settings.data.map { it[SERVER] ?: BuildConfig.DEFAULT_SERVER }

    /** Debug only: show the mock's sealed pushes while the app is open (stands in for FCM). */
    val pollMockPush: Flow<Boolean> = context.settings.data.map { it[POLL] ?: BuildConfig.ALLOW_INSECURE_LOOPBACK }

    suspend fun serverUrlNow(): String = serverUrl.first()

    suspend fun setServerUrl(url: String) {
        context.settings.edit { it[SERVER] = url.trim().trimEnd('/') }
    }

    suspend fun setPollMockPush(on: Boolean) {
        context.settings.edit { it[POLL] = on }
    }

    private companion object {
        val SERVER = stringPreferencesKey("server_url")
        val POLL = booleanPreferencesKey("poll_mock_push")
    }
}
