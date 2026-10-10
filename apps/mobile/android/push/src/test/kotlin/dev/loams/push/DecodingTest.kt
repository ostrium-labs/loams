package dev.loams.push

import dev.loams.core.Loams
import dev.loams.core.push.PushOpener
import dev.loams.proto.loams.notifications.v1.Category
import java.io.File
import java.util.Base64
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class DecodingTest {
    private val fx = Json.parseToJsonElement(
        File(System.getProperty("loams.repoRoot") ?: "../..", "conformance/fixtures/push/sealed.json").readText(),
    ).jsonObject

    @Test
    fun sealed_fixture_decodes_to_the_approval_notification_with_review() {
        val data = fx["message"]!!.jsonObject.mapValues { it.value.jsonPrimitive.content }
        val key = Base64.getDecoder().decode(fx["recipient_private_key"]!!.jsonPrimitive.content)
        val shown = PushOpener({ key }, Decoding::shown).open(data)
        val expect = fx["expect"]!!.jsonObject
        assertEquals(expect["title"]!!.jsonPrimitive.content, shown.title)
        assertEquals(expect["body"]!!.jsonPrimitive.content, shown.body)
        assertEquals(expect["approval_id"]!!.jsonPrimitive.content, shown.approvalId)
        assertTrue(Decoding.hasReviewAction(shown))
    }

    @Test
    fun approval_notification_goes_to_the_approvals_channel() {
        assertEquals(Loams.Channels.APPROVALS, Decoding.channelFor(Category.CATEGORY_APPROVALS))
        assertEquals(Loams.Channels.SECURITY, Decoding.channelFor(Category.CATEGORY_SECURITY))
        assertEquals(Loams.Channels.OPERATIONS, Decoding.channelFor(Category.CATEGORY_OPERATIONS))
    }

    @Test
    fun generic_text_names_no_content() {
        assertEquals("New activity in Loams", PushOpener.GENERIC.body)
        assertEquals(null, PushOpener.GENERIC.approvalId)
    }
}
