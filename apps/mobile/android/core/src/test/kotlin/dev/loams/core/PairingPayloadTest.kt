package dev.loams.core

import dev.loams.core.pairing.PairingError
import dev.loams.core.pairing.PairingPayloads
import dev.loams.core.pairing.PairingResult
import java.time.Instant
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.boolean
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.long
import org.junit.Assert.assertEquals
import org.junit.Assert.fail
import org.junit.Test

class PairingPayloadTest {
    private val fixture = Fixtures.json("pairing/cases.json")
    private val now = Instant.ofEpochSecond(fixture["now"]!!.jsonPrimitive.long)

    @Test
    fun every_shared_case_matches() {
        for (case in fixture["cases"]!!.jsonArray.map { it.jsonObject }) {
            val name = case["name"]!!.jsonPrimitive.content
            val expect = case["expect"]!!.jsonPrimitive.content
            val debug = case["allow_insecure_loopback"]?.jsonPrimitive?.boolean ?: false
            val result = PairingPayloads.parse(case["payload"]!!.jsonPrimitive.content, now, debug)
            when (expect) {
                "ok" -> {
                    val p = (result as? PairingResult.Ok)?.payload ?: fail("$name: $result").let { return }
                    val want = case["parsed"]!!.jsonObject
                    assertEquals(name, want["issuer"]!!.jsonPrimitive.content, p.issuer.toString())
                    assertEquals(name, want["instance_id"]!!.jsonPrimitive.content, p.instanceId)
                    assertEquals(name, want["jkt"]!!.jsonPrimitive.content, p.jkt)
                    assertEquals(name, want["code"]!!.jsonPrimitive.content, p.code)
                    assertEquals(name, want["user_code"]!!.jsonPrimitive.content, p.userCode)
                    assertEquals(name, want["exp"]!!.jsonPrimitive.long, p.exp.epochSecond)
                    val spki = want["spki"]
                    assertEquals(name, if (spki is JsonNull) null else (spki as JsonArray).map { it.jsonPrimitive.content }, p.spki)
                }
                else -> assertEquals(name, PairingResult.Refused(PairingError.valueOf(expect.uppercase())), result)
            }
        }
    }

    @Test
    fun parse_accepts_v1() {
        val text = """{"v":1,"kind":"loams-pair","issuer":"https://a.example","instance_id":"i","spki":null,"jkt":"j","code":"c","user_code":"1","exp":${now.epochSecond + 60}}"""
        assert(PairingPayloads.parse(text, now) is PairingResult.Ok)
    }

    @Test
    fun parse_refuses_expired_at_exactly_exp() {
        val text = """{"v":1,"kind":"loams-pair","issuer":"https://a.example","instance_id":"i","spki":null,"jkt":"j","code":"c","user_code":"1","exp":${now.epochSecond}}"""
        assertEquals(PairingResult.Refused(PairingError.EXPIRED), PairingPayloads.parse(text, now))
    }
}
