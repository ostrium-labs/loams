package dev.loams.transport

import java.security.MessageDigest
import java.security.cert.CertificateException
import java.security.cert.X509Certificate
import java.util.Base64
import javax.net.ssl.SSLContext
import javax.net.ssl.X509TrustManager
import okhttp3.OkHttpClient

/**
 * How the app trusts an instance's TLS certificate (AP2 Ruling 3).
 *
 * [System] uses the platform's CA store (and never user-installed CAs, which the Android network
 * security config excludes). [Pinned] replaces CA validation for that host with the SPKI pin set
 * from the QR payload, so a self-signed self-hosted server works; the hostname is still checked.
 * Either way the instance key thumbprint (`jkt`) is checked at the application layer.
 */
sealed interface TrustPolicy {
    data object System : TrustPolicy

    /** @param spki base64 SHA-256 hashes of SubjectPublicKeyInfo (current and announced next). */
    data class Pinned(val spki: Set<String>) : TrustPolicy {
        init {
            require(spki.isNotEmpty()) { "a pinned policy needs at least one pin" }
        }
    }
}

/** Thrown for a pin mismatch: a hard stop, never a fallback to CA validation. */
class PinMismatchException(message: String) : CertificateException(message)

/** Accepts exactly the chains whose leaf key is in [pins]. */
class PinnedTrustManager(private val pins: Set<String>) : X509TrustManager {
    override fun checkServerTrusted(chain: Array<out X509Certificate>?, authType: String?) {
        val leaf = chain?.firstOrNull() ?: throw PinMismatchException("empty certificate chain")
        leaf.checkValidity()
        if (spkiPin(leaf) !in pins) {
            throw PinMismatchException("This server's identity changed: its key is not the one this phone paired with.")
        }
    }

    override fun checkClientTrusted(chain: Array<out X509Certificate>?, authType: String?): Unit =
        throw CertificateException("client certificates are not used")

    override fun getAcceptedIssuers(): Array<X509Certificate> = emptyArray()

    companion object {
        /** base64(SHA-256(SubjectPublicKeyInfo)), the `spki` entries of the QR payload. */
        fun spkiPin(cert: X509Certificate): String =
            Base64.getEncoder().encodeToString(MessageDigest.getInstance("SHA-256").digest(cert.publicKey.encoded))
    }
}

/** Applies a trust policy to an OkHttp builder. The default hostname verifier stays in place. */
fun OkHttpClient.Builder.trust(policy: TrustPolicy): OkHttpClient.Builder = when (policy) {
    TrustPolicy.System -> this
    is TrustPolicy.Pinned -> {
        val tm = PinnedTrustManager(policy.spki)
        val ctx = SSLContext.getInstance("TLS").apply { init(null, arrayOf(tm), null) }
        sslSocketFactory(ctx.socketFactory, tm)
    }
}
