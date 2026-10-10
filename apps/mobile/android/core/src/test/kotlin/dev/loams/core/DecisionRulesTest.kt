package dev.loams.core

import dev.loams.core.decision.Decision
import dev.loams.core.decision.DecisionProblem
import dev.loams.core.decision.DecisionRules
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class DecisionRulesTest {
    private fun check(d: Decision, destructive: Boolean = false, typed: String = "", reason: String = "", live: Boolean = true, pending: Boolean = true) =
        DecisionRules.check(d, pending, destructive, "logs-2026", typed, reason, live)

    @Test fun reject_requires_reason() = assertEquals(DecisionProblem.REASON_REQUIRED, check(Decision.REJECT))

    @Test fun reject_with_reason_is_fine() = assertNull(check(Decision.REJECT, reason = "not now"))

    @Test fun destructive_requires_typed_target() {
        assertEquals(DecisionProblem.CONFIRMATION_MISMATCH, check(Decision.APPROVE, destructive = true, typed = "logs"))
        assertNull(check(Decision.APPROVE, destructive = true, typed = "logs-2026"))
    }

    @Test fun offline_disables_approve() = assertEquals(DecisionProblem.OFFLINE, check(Decision.APPROVE, live = false))

    @Test fun decided_cannot_be_decided_again() = assertEquals(DecisionProblem.NOT_PENDING, check(Decision.APPROVE, pending = false))
}
