package dev.loams.core

import dev.loams.core.decision.Decision
import dev.loams.core.decision.DecisionClaims
import dev.loams.core.decision.EcdsaSignatures
import dev.loams.core.decision.Jws
import java.math.BigInteger
import java.security.KeyPairGenerator
import java.security.Signature
import java.security.spec.ECGenParameterSpec
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.long
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class DecisionClaimsTest {
    @Test
    fun decision_claims_canonical_json_is_stable() {
        for (case in Fixtures.json("decision/claims.json")["cases"]!!.jsonArray.map { it.jsonObject }) {
            val c = case["claims"]!!.jsonObject
            val claims = DecisionClaims(
                approvalId = c["approval_id"]!!.jsonPrimitive.content,
                // uint64 on the wire: parse unsigned into a Long.
                revision = BigInteger(c["revision"]!!.jsonPrimitive.content).toLong(),
                decision = Decision.valueOf(c["decision"]!!.jsonPrimitive.content.uppercase()),
                iat = c["iat"]!!.jsonPrimitive.long,
                jti = c["jti"]!!.jsonPrimitive.content,
            )
            assertEquals(case["name"]!!.jsonPrimitive.content, case["canonical"]!!.jsonPrimitive.content, claims.canonicalJson().toString(Charsets.UTF_8))
        }
    }

    @Test
    fun der_signature_converts_to_a_jws_es256_signature_that_verifies() {
        val kp = KeyPairGenerator.getInstance("EC").apply { initialize(ECGenParameterSpec("secp256r1")) }.generateKeyPair()
        val claims = DecisionClaims("apr_1", 3, Decision.APPROVE, 1_790_899_200, "jti")
        val input = Jws.signingInput(Jws.DECISION_HEADER, claims.canonicalJson())
        repeat(20) { // DER integers vary in length (leading zeros); exercise several
            val der = Signature.getInstance("SHA256withECDSA").run {
                initSign(kp.private)
                update(input.toByteArray())
                sign()
            }
            val raw = EcdsaSignatures.derToRaw(der)
            assertEquals(64, raw.size)
            val ok = Signature.getInstance("SHA256withECDSAinP1363Format").run {
                initVerify(kp.public)
                update(input.toByteArray())
                verify(raw)
            }
            assertTrue(ok)
        }
        assertEquals(3, Jws.compact(input, ByteArray(64)).split('.').size)
    }

    @Test
    fun malformed_der_is_an_illegal_argument() {
        val good = Signature.getInstance("SHA256withECDSA").run {
            initSign(KeyPairGenerator.getInstance("EC").apply { initialize(ECGenParameterSpec("secp256r1")) }.generateKeyPair().private)
            update(byteArrayOf(1))
            sign()
        }
        for (bad in listOf(ByteArray(0), good.copyOf(good.size - 3), byteArrayOf(0x30, 0x81.toByte()), good + byteArrayOf(0))) {
            try {
                EcdsaSignatures.derToRaw(bad)
                throw AssertionError("accepted ${bad.size} bytes")
            } catch (e: IllegalArgumentException) {
                // expected
            }
        }
    }
}
