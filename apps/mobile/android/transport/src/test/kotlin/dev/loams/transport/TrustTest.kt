package dev.loams.transport

import java.net.InetAddress
import javax.net.ssl.SSLHandshakeException
import mockwebserver3.MockResponse
import mockwebserver3.MockWebServer
import okhttp3.Request
import okhttp3.tls.HandshakeCertificates
import okhttp3.tls.HeldCertificate
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Before
import org.junit.Test

class TrustTest {
    private val server = MockWebServer()
    private val localhost = "localhost"
    private val loopback = InetAddress.getByName("127.0.0.1")

    // One address only, so a refused handshake is not retried on another route (::1).
    private val base = okhttp3.OkHttpClient.Builder().dns { listOf(loopback) }.build()

    private fun cert(host: String) = HeldCertificate.Builder().addSubjectAlternativeName(host).build()

    private fun serve(cert: HeldCertificate) {
        server.useHttps(HandshakeCertificates.Builder().heldCertificate(cert).build().sslSocketFactory())
        server.enqueue(MockResponse.Builder().body("ok").build())
        server.start(loopback, 0)
    }

    @Before fun setUp() = Unit

    @After fun tearDown() = server.close()

    private fun get(policy: TrustPolicy): String =
        Http.client(policy, base).newCall(Request.Builder().url("https://$localhost:${server.port}/").build()).execute().use { it.body.string() }

    @Test
    fun pinned_instance_connects() {
        val c = cert(localhost)
        serve(c)
        assertEquals("ok", get(TrustPolicy.Pinned(setOf(PinnedTrustManager.spkiPin(c.certificate)))))
    }

    @Test
    fun pin_mismatch_is_hard_stop() {
        serve(cert(localhost))
        val other = cert(localhost)
        try {
            get(TrustPolicy.Pinned(setOf(PinnedTrustManager.spkiPin(other.certificate))))
            fail("connected with the wrong pin")
        } catch (e: SSLHandshakeException) {
            assertTrue(generateSequence<Throwable>(e) { it.cause }.any { it is PinMismatchException })
        }
    }

    @Test
    fun hostname_checked_with_pin() {
        val c = cert("not-this-host.example")
        serve(c)
        try {
            get(TrustPolicy.Pinned(setOf(PinnedTrustManager.spkiPin(c.certificate))))
            fail("a pinned key on the wrong host name was accepted")
        } catch (e: javax.net.ssl.SSLPeerUnverifiedException) {
            // OkHttp's hostname verifier still runs with a pinned key.
        }
    }

    @Test
    fun system_trust_refuses_self_signed() {
        serve(cert(localhost))
        try {
            get(TrustPolicy.System)
            fail("a self-signed certificate passed system trust")
        } catch (e: SSLHandshakeException) {
            // expected
        }
    }

    @Test
    fun redirects_are_not_followed() {
        server.enqueue(MockResponse.Builder().code(302).addHeader("Location", "https://elsewhere.example/").build())
        server.start()
        val code = Http.client(TrustPolicy.System).newCall(Request.Builder().url(server.url("/")).build()).execute().use { it.code }
        assertEquals(302, code)
    }
}
