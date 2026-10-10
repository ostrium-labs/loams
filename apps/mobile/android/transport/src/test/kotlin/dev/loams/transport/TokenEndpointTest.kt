package dev.loams.transport

import dev.loams.core.errors.Reason
import kotlinx.coroutines.test.runTest
import mockwebserver3.MockResponse
import mockwebserver3.MockWebServer
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class TokenEndpointTest {
    private val server = MockWebServer().apply { start() }

    @After fun tearDown() = server.close()

    private fun endpoint() = TokenEndpoint(server.url("/").toString(), Http.client(TrustPolicy.System))

    @Test
    fun pairing_sends_the_grant_and_device_fields() = runTest {
        server.enqueue(MockResponse.Builder().body("""{"access_token":"a","token_type":"DPoP","expires_in":3600,"refresh_token":"r","device_id":"dev_1"}""").build())
        val r = endpoint().redeemPairing("CODE", null, DeviceRegistration("Pixel", """{"kty":"EC"}"""))
        assertEquals(TokenResult.Ok(DeviceTokens("a", "r", 3600, "dev_1")), r)
        val body = server.takeRequest().body!!.utf8()
        assertTrue(body, body.contains("grant_type=urn%3Aloams%3Aparams%3Aoauth%3Agrant-type%3Apairing"))
        assertTrue(body, body.contains("code=CODE") && !body.contains("user_code"))
        assertTrue(body, body.contains("platform=android") && body.contains("decision_jwk="))
    }

    @Test
    fun pairing_code_reuse_is_pairing_used() = runTest {
        server.enqueue(MockResponse.Builder().code(400).body("""{"error":"invalid_grant","error_description":"this pairing code was already used","loams_reason":"pairing_used"}""").build())
        val r = endpoint().redeemPairing(null, "12345678", DeviceRegistration("Pixel", "{}")) as TokenResult.Refused
        assertEquals(Reason.PAIRING_USED, r.reason)
    }

    @Test
    fun error_description_is_never_read_as_a_reason() = runTest {
        server.enqueue(MockResponse.Builder().code(400).body("""{"error":"invalid_grant","error_description":"pairing_used"}""").build())
        val r = endpoint().redeemPairing("CODE", null, DeviceRegistration("Pixel", "{}")) as TokenResult.Refused
        assertEquals(Reason.UNKNOWN, r.reason)
    }

    @Test
    fun tokens_are_redacted_in_toString() {
        val t = DeviceTokens("secret-access", "secret-refresh", 1, "dev")
        assertTrue(!t.toString().contains("secret"))
    }
}
