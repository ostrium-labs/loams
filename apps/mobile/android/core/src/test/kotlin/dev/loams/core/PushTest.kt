package dev.loams.core

import dev.loams.core.push.PushKeyPair
import dev.loams.core.push.PushMessage
import dev.loams.core.push.PushOpener
import dev.loams.core.push.Shown
import dev.loams.core.push.Unsealer
import java.security.GeneralSecurityException
import java.util.Base64
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class PushTest {
    private val fx = Fixtures.json("push/sealed.json")
    private fun b64(key: String) = Base64.getDecoder().decode(fx[key]!!.jsonPrimitive.content)
    private val instance = fx["instance_id"]!!.jsonPrimitive.content
    private val notification = fx["notification_id"]!!.jsonPrimitive.content

    @Test
    fun sealed_payload_round_trip_with_the_mock_fixture() {
        val plain = Unsealer.open(b64("sealed"), instance, notification, b64("recipient_private_key"))
        assertArrayEquals(b64("plaintext"), plain)
        assertArrayEquals(b64("info"), Unsealer.info(instance, notification))
    }

    @Test(expected = GeneralSecurityException::class)
    fun unseal_refuses_another_notification_id() {
        Unsealer.open(b64("sealed"), instance, "ntf_other", b64("recipient_private_key"))
    }

    @Test
    fun seal_then_open() {
        val kp = PushKeyPair.generate()
        val sealed = Unsealer.seal("hi".toByteArray(), "i", "n", kp.publicKey)
        assertEquals("hi", Unsealer.open(sealed, "i", "n", kp.privateKey).toString(Charsets.UTF_8))
    }

    @Test
    fun push_message_has_only_the_four_fields() {
        val data = fx["message"]!!.jsonObject.mapValues { it.value.jsonPrimitive.content }
        assertEquals(PushMessage.FIELDS, data.keys)
        val msg = PushMessage.parse(data)!!
        assertEquals(notification, msg.notificationId)
        assertNull(PushMessage.parse(data + ("v" to "2")))
    }

    @Test
    fun unknown_instance_or_bad_payload_shows_generic() {
        val data = fx["message"]!!.jsonObject.mapValues { it.value.jsonPrimitive.content }
        val decode: (ByteArray) -> Shown = { Shown("t", "b", "n", null) }
        assertEquals(PushOpener.GENERIC, PushOpener({ null }, decode).open(data))
        assertEquals(PushOpener.GENERIC, PushOpener({ PushKeyPair.generate().privateKey }, decode).open(data))
        assertEquals(Shown("t", "b", "n", null), PushOpener({ b64("recipient_private_key") }, decode).open(data))
    }
}
