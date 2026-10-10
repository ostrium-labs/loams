package dev.loams.app.ui.welcome

import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import dev.loams.app.session.AuthentikSignIn
import dev.loams.app.session.PairResult
import android.util.Log
import kotlinx.coroutines.CancellationException
import dev.loams.app.session.SessionManager
import dev.loams.transport.Http
import dev.loams.transport.TrustPolicy
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import okhttp3.Request

/** Pairing and sign-in before a session exists (design §37 §7.2.2, the three ways to pair). */
class WelcomeViewModel(private val session: SessionManager) : ViewModel() {
    private val _busy = MutableStateFlow(false)
    val busy: StateFlow<Boolean> = _busy.asStateFlow()

    private val _message = MutableStateFlow<String?>(null)
    val message: StateFlow<String?> = _message.asStateFlow()

    /** Typed-code pairing waiting for the user to compare the fingerprint. */
    private val _confirm = MutableStateFlow<PairResult.ConfirmFingerprint?>(null)
    val confirm: StateFlow<PairResult.ConfirmFingerprint?> = _confirm.asStateFlow()

    fun say(text: String?) {
        _message.value = text
    }

    /** The browser sign-in in flight; kept here so it survives activity recreation. */
    var pendingSignIn: AuthentikSignIn.Pending? = null

    private fun run(block: suspend () -> PairResult) {
        viewModelScope.launch {
            _busy.value = true
            val result = try {
                block()
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                // Never crash on a network error. The UI gets a fixed message; details go to logcat.
                Log.w(TAG, "pairing failed", e)
                PairResult.Failed("Could not reach the server. Check the address and your connection.")
            } finally {
                _busy.value = false
            }
            when (val r = result) {
                PairResult.Paired -> _message.value = null
                is PairResult.Failed -> _message.value = r.message
                is PairResult.ConfirmFingerprint -> _confirm.value = r
            }
        }
    }

    fun pairFromPayload(text: String) = run { session.pairFromPayload(text) }

    fun startTyped(issuer: String) = run { session.startTypedPairing(issuer) }

    fun confirmTyped(userCode: String) {
        val c = _confirm.value ?: return
        _confirm.value = null
        run { session.finishTypedPairing(c, userCode) }
    }

    fun cancelTyped() {
        _confirm.value = null
    }

    fun finishBrowser(issuer: String, instanceId: String, jkt: String, authentikToken: String) =
        run { session.finishBrowserSignIn(issuer, instanceId, jkt, authentikToken) }

    /** Debug builds only: ask the local mock for a fresh pairing payload, as the console would show. */
    fun pairWithMock(server: String) = run {
        val text = withContext(Dispatchers.IO) {
            Http.client(TrustPolicy.System).newCall(Request.Builder().url("${server.trimEnd('/')}/mock/pairing").build()).execute().use { it.body.string() }
        }
        session.pairFromPayload(text)
    }

    private companion object {
        const val TAG = "Loams"
    }
}
