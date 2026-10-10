package dev.loams.app.session

import android.os.Build
import dev.loams.app.BuildConfig
import dev.loams.app.backend.Backend
import dev.loams.app.backend.DemoBackend
import dev.loams.app.backend.RemoteBackend
import dev.loams.core.errors.Reason
import dev.loams.core.jose.Jwk
import dev.loams.core.pairing.PairingError
import dev.loams.core.pairing.PairingPayloads
import dev.loams.core.pairing.PairingResult
import dev.loams.data.keys.DeviceKeys
import dev.loams.data.keys.KeyPolicy
import dev.loams.data.session.SessionRecord
import dev.loams.data.session.SessionStore
import dev.loams.push.FcmStubRegistrar
import dev.loams.push.PushKeys
import dev.loams.transport.Clients
import dev.loams.transport.DeviceRegistration
import dev.loams.transport.DeviceTokens
import dev.loams.transport.Http
import dev.loams.transport.InstanceCheck
import dev.loams.transport.InstanceIdentityException
import dev.loams.transport.PairingOutcome
import dev.loams.transport.QrPairing
import dev.loams.transport.TokenEndpoint
import dev.loams.transport.TokenResult
import dev.loams.transport.TokenSource
import dev.loams.transport.TrustPolicy
import dev.loams.proto.loams.instance.v1.GetInstanceRequest
import com.connectrpc.getOrThrow
import java.net.URI
import java.time.Instant
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock

sealed interface SessionState {
    data object Loading : SessionState

    data object SignedOut : SessionState

    data class Active(val backend: Backend, val record: SessionRecord?) : SessionState
}

/** A pairing attempt's result, in words the pairing screen shows. */
sealed interface PairResult {
    data object Paired : PairResult

    data class Failed(val message: String) : PairResult

    /** Typed-code pairing: the user must compare this fingerprint with the console's first. */
    data class ConfirmFingerprint(val issuer: String, val instanceId: String, val thumbprint: String) : PairResult
}

/**
 * The one paired instance (or demo mode), its tokens and its backend. The access token lives in
 * memory only; the refresh token is in [SessionStore], encrypted with a Keystore key.
 */
class SessionManager(
    private val store: SessionStore,
    private val keys: DeviceKeys,
    private val pushKeys: PushKeys,
    private val push: FcmStubRegistrar,
) {
    private val _state = MutableStateFlow<SessionState>(SessionState.Loading)
    val state: StateFlow<SessionState> = _state.asStateFlow()

    @Volatile private var accessToken: String? = null
    private val tokenSource = TokenSource { accessToken }

    private val allowInsecure = BuildConfig.ALLOW_INSECURE_LOOPBACK

    private val deviceName: String get() = "${Build.MANUFACTURER} ${Build.MODEL}".trim()

    /** At start: refresh the stored session (rotating the refresh token), or sign out. */
    suspend fun restore() {
        val record = store.current()
        val refresh = store.refreshToken()
        if (record == null || refresh == null) {
            _state.value = SessionState.SignedOut
            return
        }
        val result = runCatching { TokenEndpoint(record.issuer, Http.client(record.trust)).refresh(refresh) }.getOrNull()
        when {
            result is TokenResult.Ok -> {
                store.saveRefreshToken(result.tokens.refreshToken)
                activate(record, result.tokens)
            }
            // Revoked, or the refresh token was reused: the pairing is gone.
            result is TokenResult.Refused && isDefinitive(result) -> signOut()
            // Offline or a gateway hiccup: show the instance read-only. The streams ask for a
            // token through refreshAccess() when the network is back.
            else -> _state.value = SessionState.Active(backendFor(record), record)
        }
    }

    enum class Refresh { REFRESHED, REVOKED, UNAVAILABLE }

    private val refreshLock = Mutex()

    /**
     * Gets a new access token with the stored refresh token (rotating it). Called when a call
     * answers UNAUTHENTICATED: after the access token expired, or after an offline start.
     * Signs out only on a definitive refusal; a gateway error is UNAVAILABLE and retried.
     */
    suspend fun refreshAccess(failedToken: String?): Refresh = refreshLock.withLock {
        // Another caller already refreshed while this one waited.
        if (accessToken != null && accessToken != failedToken) return@withLock Refresh.REFRESHED
        val record = store.current() ?: return@withLock Refresh.REVOKED
        val refresh = store.refreshToken() ?: return@withLock Refresh.REVOKED
        val result = runCatching { TokenEndpoint(record.issuer, Http.client(record.trust)).refresh(refresh) }.getOrNull()
            ?: return@withLock Refresh.UNAVAILABLE
        when (result) {
            is TokenResult.Ok -> {
                store.saveRefreshToken(result.tokens.refreshToken)
                accessToken = result.tokens.accessToken
                Refresh.REFRESHED
            }
            is TokenResult.Refused -> if (isDefinitive(result)) {
                signOut()
                Refresh.REVOKED
            } else {
                Refresh.UNAVAILABLE
            }
        }
    }

    private fun isDefinitive(r: TokenResult.Refused) = r.error == "invalid_grant" || r.reason == Reason.DEVICE_REVOKED

    fun startDemo() {
        _state.value = SessionState.Active(DemoBackend(), null)
    }

    /** QR (or pasted) payload: pin, check the instance, create keys, redeem (§37 §7.2.2 path 1). */
    suspend fun pairFromPayload(text: String): PairResult {
        val payload = when (val r = PairingPayloads.parse(text, Instant.now(), allowInsecure)) {
            is PairingResult.Ok -> r.payload
            is PairingResult.Refused -> return PairResult.Failed(messageFor(r.error))
        }
        return try {
            when (val outcome = QrPairing.pair(payload, deviceName) { id -> Jwk.fromEcPublicKey(keys.ensure(KeyPolicy.decide(id))).toJson() }) {
                is PairingOutcome.Paired -> {
                    keys.ensure(KeyPolicy.dpop(payload.instanceId))
                    val i = outcome.instance
                    finish(SessionRecord(i.issuer, i.instanceId, i.jkt, payload.spki?.toSet(), i.tokens.deviceId, "qr"), i.tokens)
                }
                is PairingOutcome.Refused -> PairResult.Failed(messageFor(outcome.result.reason, outcome.result.description))
            }
        } catch (e: InstanceIdentityException) {
            PairResult.Failed(e.message ?: "This server's identity changed.")
        } catch (e: Exception) {
            PairResult.Failed(keyOrNetworkMessage(e))
        }
    }

    /**
     * Typed-code pairing, step 1: trust on first use. Nothing was pinned in advance, so the user
     * compares the instance key's fingerprint with what the console shows (the harness's honesty
     * rule, §37 §7.2.2).
     */
    suspend fun startTypedPairing(issuer: String): PairResult {
        val uri = runCatching { URI(issuer.trim().trimEnd('/')) }.getOrNull()
        if (uri?.host == null || !PairingPayloads.issuerAllowed(uri, allowInsecure)) {
            return PairResult.Failed("Enter the https address of your Loams instance.")
        }
        return try {
            val http = Http.client(TrustPolicy.System)
            val info = Clients(uri.toString(), http, { null }).instance.getInstance(GetInstanceRequest.getDefaultInstance()).getOrThrow()
            val thumb = InstanceCheck.fetchThumbprints(http, info.jwksUri).firstOrNull()
                ?: return PairResult.Failed("This server publishes no instance key.")
            PairResult.ConfirmFingerprint(uri.toString(), info.instanceId, thumb)
        } catch (e: Exception) {
            PairResult.Failed(keyOrNetworkMessage(e))
        }
    }

    /** Typed-code pairing, step 2, after the user confirmed the fingerprint. */
    suspend fun finishTypedPairing(confirm: PairResult.ConfirmFingerprint, userCode: String): PairResult {
        if (!userCode.matches(Regex("\\d{8}"))) return PairResult.Failed("The code is 8 digits.")
        return try {
            val jwk = Jwk.fromEcPublicKey(keys.ensure(KeyPolicy.decide(confirm.instanceId))).toJson()
            val endpoint = TokenEndpoint(confirm.issuer, Http.client(TrustPolicy.System))
            when (val r = endpoint.redeemPairing(null, userCode, DeviceRegistration(deviceName, jwk, Build.MODEL, BuildConfig.VERSION_NAME))) {
                is TokenResult.Ok -> finish(SessionRecord(confirm.issuer, confirm.instanceId, confirm.thumbprint, null, r.tokens.deviceId, "typed"), r.tokens)
                is TokenResult.Refused -> PairResult.Failed(messageFor(r.reason, r.description))
            }
        } catch (e: Exception) {
            PairResult.Failed(keyOrNetworkMessage(e))
        }
    }

    /** Browser sign-in at Authentik finished: exchange its token at the gateway (path 2). */
    suspend fun finishBrowserSignIn(issuer: String, instanceId: String, jkt: String, authentikAccessToken: String): PairResult = try {
        val jwk = Jwk.fromEcPublicKey(keys.ensure(KeyPolicy.decide(instanceId))).toJson()
        val endpoint = TokenEndpoint(issuer, Http.client(TrustPolicy.System))
        when (val r = endpoint.exchange(authentikAccessToken, DeviceRegistration(deviceName, jwk, Build.MODEL, BuildConfig.VERSION_NAME))) {
            is TokenResult.Ok -> finish(SessionRecord(issuer, instanceId, jkt, null, r.tokens.deviceId, "browser"), r.tokens)
            is TokenResult.Refused -> PairResult.Failed(messageFor(r.reason, r.description))
        }
    } catch (e: Exception) {
        PairResult.Failed(keyOrNetworkMessage(e))
    }

    /** Signing out (or a revocation) deletes this instance's keys, tokens and cached state. */
    suspend fun signOut() {
        store.current()?.let { r ->
            keys.delete(r.instanceId)
            pushKeys.delete(r.instanceId)
        }
        accessToken = null
        store.clear()
        _state.value = SessionState.SignedOut
    }

    /** The FCM stub token this device registered with, for the debug push poller. */
    suspend fun pushToken(): String = push.token()

    private suspend fun finish(record: SessionRecord, tokens: DeviceTokens): PairResult {
        store.save(record, tokens.refreshToken)
        activate(record, tokens)
        // Register for (stubbed) push right away; failures only mean no notifications.
        val backend = (state.value as? SessionState.Active)?.backend
        runCatching { backend?.registerPush(push.token(), pushKeys.ensure(record.instanceId)) }
        return PairResult.Paired
    }

    private fun activate(record: SessionRecord, tokens: DeviceTokens) {
        accessToken = tokens.accessToken
        _state.value = SessionState.Active(backendFor(record), record)
    }

    private fun backendFor(record: SessionRecord): Backend =
        RemoteBackend(Clients(record.issuer, Http.client(record.trust), tokenSource), record.instanceId, record.deviceId, record.trust) {
            when (refreshAccess(accessToken)) {
                Refresh.REFRESHED -> true
                Refresh.REVOKED -> false
                Refresh.UNAVAILABLE -> throw java.io.IOException("the token endpoint is unavailable")
            }
        }

    /** A fixed message for the UI; the exception itself goes to logcat only. */
    private fun keyOrNetworkMessage(e: Exception): String {
        android.util.Log.w("Loams", "pairing failed", e)
        return when {
            e is javax.net.ssl.SSLHandshakeException -> "This server's identity changed or its certificate is not trusted. Nothing was sent."
            e.javaClass.name.contains("InvalidAlgorithmParameter") || e is IllegalStateException ->
                "Set a screen lock on this phone first: the approval key needs one."
            else -> "Could not reach the server. Check the address and your connection."
        }
    }

    companion object {
        fun messageFor(error: PairingError): String = when (error) {
            PairingError.UNSUPPORTED_VERSION -> "This pairing code needs a newer version of the app."
            PairingError.WRONG_KIND -> "That is not a Loams pairing code."
            PairingError.EXPIRED -> "This pairing code expired. Create a new one in the console."
            PairingError.INSECURE_ISSUER -> "This pairing code points to a server without https."
            PairingError.MALFORMED -> "That is not a Loams pairing code."
        }

        fun messageFor(reason: Reason, description: String?): String = when (reason) {
            Reason.PAIRING_EXPIRED -> "This pairing code expired. Create a new one in the console."
            Reason.PAIRING_USED -> "This pairing code was already used. Create a new one in the console."
            Reason.DEVICE_REVOKED -> "This device was revoked."
            else -> description ?: "The server refused the pairing."
        }
    }
}
