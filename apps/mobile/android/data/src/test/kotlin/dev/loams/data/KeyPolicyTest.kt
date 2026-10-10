package dev.loams.data

import dev.loams.data.keys.KeyPolicy
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class KeyPolicyTest {
    @Test
    fun decide_requires_user_presence_for_every_use() {
        val p = KeyPolicy.decide("01J9")
        assertEquals("decide-01J9", p.alias)
        assertTrue(p.userPresenceEveryUse)
        assertTrue(p.allowDeviceCredential)
    }

    @Test
    fun enrolment_change_invalidates_decide_key() {
        assertTrue(KeyPolicy.decide("x").invalidatedByEnrollment)
    }

    @Test
    fun dpop_key_needs_no_user_so_background_refresh_works() {
        val p = KeyPolicy.dpop("01J9")
        assertEquals("dpop-01J9", p.alias)
        assertFalse(p.userPresenceEveryUse)
    }
}
