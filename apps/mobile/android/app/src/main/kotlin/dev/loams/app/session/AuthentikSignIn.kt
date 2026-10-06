package dev.loams.app.session

import android.content.Context
import android.content.Intent
import android.net.Uri
import dev.loams.app.BuildConfig
import dev.loams.core.Loams
import dev.loams.core.auth.Pkce
import dev.loams.core.pairing.PairingPayloads
import dev.loams.proto.loams.instance.v1.GetInstanceRequest
import dev.loams.transport.Clients
import dev.loams.transport.Http
import dev.loams.transport.InstanceCheck
import dev.loams.transport.TrustPolicy
import com.connectrpc.getOrThrow
import java.net.HttpURLConnection
import java.net.URI
import java.net.URL
import kotlin.coroutines.resume
import kotlin.coroutines.resumeWithException
import kotlinx.coroutines.suspendCancellableCoroutine
import net.openid.appauth.AppAuthConfiguration
import net.openid.appauth.AuthorizationException
import net.openid.appauth.AuthorizationRequest
import net.openid.appauth.AuthorizationResponse
import net.openid.appauth.AuthorizationService
import net.openid.appauth.AuthorizationServiceConfiguration
import net.openid.appauth.ResponseTypeValues
import net.openid.appauth.connectivity.ConnectionBuilder
import net.openid.appauth.connectivity.DefaultConnectionBuilder

/**
 * Browser sign-in at the instance's Authentik (design §37 §7.2.2 path 2): OIDC authorization code
 * with PKCE S256 in Custom Tabs through AppAuth, then an RFC 8693 exchange at the Loams gateway.
 * Against the mock, "Authentik" is the mock's fake one, which redirects straight back.
 */
class AuthentikSignIn(context: Context) {
    /** What the second half needs after the browser returns. */
    data class Pending(val issuer: String, val instanceId: String, val jkt: String, val request: AuthorizationRequest)

    // Debug builds may reach the mock's fake Authentik over http on loopback only.
    private val connections: ConnectionBuilder = if (BuildConfig.ALLOW_INSECURE_LOOPBACK) LoopbackConnectionBuilder else DefaultConnectionBuilder.INSTANCE
    private val service = AuthorizationService(
        context,
        AppAuthConfiguration.Builder()
            .setConnectionBuilder(connections)
            .setSkipIssuerHttpsCheck(BuildConfig.ALLOW_INSECURE_LOOPBACK)
            .build(),
    )

    /** Looks up the instance's Authentik and builds the browser intent. */
    suspend fun start(issuer: String): Pair<Pending, Intent> {
        val base = issuer.trim().trimEnd('/')
        val uri = URI(base)
        require(uri.host != null && PairingPayloads.issuerAllowed(uri, BuildConfig.ALLOW_INSECURE_LOOPBACK)) { "Enter the https address of your Loams instance." }
        val http = Http.client(TrustPolicy.System)
        val info = Clients(base, http, { null }).instance.getInstance(GetInstanceRequest.getDefaultInstance()).getOrThrow()
        require(info.hasIdentityProvider()) { "This instance has no browser sign-in configured." }
        // Trust on first use for the instance key, recorded with the session.
        val jkt = InstanceCheck.fetchThumbprints(http, info.jwksUri).firstOrNull().orEmpty()
        val config = discover(Uri.parse(info.identityProvider.issuer))
        val pkce = Pkce.generate()
        val request = AuthorizationRequest.Builder(
            config,
            info.identityProvider.androidClientId.ifEmpty { Loams.CLIENT_ID },
            ResponseTypeValues.CODE,
            Uri.parse(REDIRECT),
        )
            .setScopes("openid", "profile", "email", "offline_access")
            .setCodeVerifier(pkce.verifier, pkce.challenge, pkce.method)
            .build()
        return Pending(base, info.instanceId, jkt, request) to service.getAuthorizationRequestIntent(request)
    }

    /** Finishes the code exchange at Authentik and returns its access token. */
    suspend fun complete(result: Intent): String {
        val response = AuthorizationResponse.fromIntent(result)
            ?: throw AuthorizationException.fromIntent(result) ?: IllegalStateException("sign-in was cancelled")
        return suspendCancellableCoroutine { cont ->
            service.performTokenRequest(response.createTokenExchangeRequest()) { token, error ->
                when {
                    token?.accessToken != null -> cont.resume(token.accessToken!!)
                    else -> cont.resumeWithException(error ?: IllegalStateException("no access token"))
                }
            }
        }
    }

    fun dispose() = service.dispose()

    private suspend fun discover(idpIssuer: Uri): AuthorizationServiceConfiguration = suspendCancellableCoroutine { cont ->
        AuthorizationServiceConfiguration.fetchFromIssuer(idpIssuer, { config, error ->
            if (config != null) cont.resume(config) else cont.resumeWithException(error ?: IllegalStateException("discovery failed"))
        }, connections)
    }

    companion object {
        /** Custom scheme until the app link https://loams.dev/app/auth/callback is verified (Q281). */
        const val REDIRECT = "dev.loams.app:/oauth2redirect"
    }
}

/** Debug only: like AppAuth's default builder, but allows plain http to loopback hosts. */
private object LoopbackConnectionBuilder : ConnectionBuilder {
    private val loopback = setOf("10.0.2.2", "127.0.0.1", "localhost")

    override fun openConnection(uri: Uri): HttpURLConnection {
        require(uri.scheme == "https" || (uri.scheme == "http" && uri.host in loopback)) { "only https, or http to the local mock" }
        return (URL(uri.toString()).openConnection() as HttpURLConnection).apply {
            connectTimeout = 15_000
            readTimeout = 10_000
            instanceFollowRedirects = false
        }
    }
}
