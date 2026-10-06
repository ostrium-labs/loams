package dev.loams.app

import dev.loams.app.backend.Backend
import dev.loams.app.backend.DecideOutcome
import dev.loams.app.backend.DemoBackend
import dev.loams.app.signing.DecisionSigner
import dev.loams.app.ui.approvals.ApprovalsViewModel
import dev.loams.core.decision.Decision
import dev.loams.proto.loams.approvals.v1.Approval
import java.util.Base64
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class ApprovalsViewModelTest {
    private class RecordingSigner : DecisionSigner {
        val inputs = mutableListOf<String>()

        override suspend fun sign(instanceId: String, signingInput: ByteArray, title: String, subtitle: String): ByteArray {
            inputs += String(signingInput)
            return ByteArray(64)
        }
    }

    private val backend = DemoBackend()
    private fun approval(id: String): Approval = backend.approvals.value.first { it.id == id }

    @Test
    fun destructive_requires_typed_target_before_signing() = runTest {
        val signer = RecordingSigner()
        val vm = ApprovalsViewModel(backend, signer)
        val msg = vm.decideNow(approval("apr_drop_logs"), Decision.APPROVE, reason = "", typed = "logs")
        assertEquals("Type logs-2026 to confirm.", msg)
        assertTrue("nothing may be signed before the checks pass", signer.inputs.isEmpty())
    }

    @Test
    fun approve_signs_the_canonical_claims_and_removes_the_card() = runTest {
        val signer = RecordingSigner()
        val vm = ApprovalsViewModel(backend, signer, clockMillis = { 1_790_899_200_000 })
        val msg = vm.decideNow(approval("apr_drop_logs"), Decision.APPROVE, reason = "", typed = "logs-2026")
        assertTrue(msg, msg.startsWith("Approved"))
        val payload = String(Base64.getUrlDecoder().decode(signer.inputs.single().split('.')[1]))
        assertTrue(payload, payload.startsWith("""{"approval_id":"apr_drop_logs","decision":"approve","iat":1790899200,"jti":""""))
        assertTrue(payload.endsWith(""","revision":1}"""))
        assertTrue(backend.approvals.value.none { it.id == "apr_drop_logs" })
    }

    @Test
    fun reject_requires_reason() = runTest {
        val vm = ApprovalsViewModel(backend, RecordingSigner())
        assertEquals("Say why you are rejecting it.", vm.decideNow(approval("apr_agent_reindex"), Decision.REJECT, reason = " ", typed = ""))
    }

    @Test
    fun a_retry_after_not_sent_reuses_the_idempotency_key() = runTest {
        val keys = mutableListOf<String>()
        val flaky = object : Backend by backend {
            override suspend fun decide(approval: Approval, decision: Decision, reason: String, proof: String, idempotencyKey: String): DecideOutcome {
                keys += idempotencyKey
                return if (keys.size == 1) DecideOutcome.NotSent("Not sent: timeout") else backend.decide(approval, decision, reason, proof, idempotencyKey)
            }
        }
        val vm = ApprovalsViewModel(flaky, RecordingSigner())
        val a = approval("apr_agent_reindex")
        assertTrue(vm.decideNow(a, Decision.APPROVE, "", "").contains("retry"))
        assertTrue(vm.decideNow(a, Decision.APPROVE, "", "").startsWith("Approved"))
        assertEquals(2, keys.size)
        assertEquals(keys[0], keys[1])
    }
}
