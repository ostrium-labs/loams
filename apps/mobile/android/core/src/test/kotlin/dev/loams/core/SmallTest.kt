package dev.loams.core

import dev.loams.core.auth.Pkce
import dev.loams.core.errors.Reason
import dev.loams.core.jose.Jwk
import java.security.MessageDigest
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import org.junit.Assert.assertEquals
import org.junit.Test

class SmallTest {
    @Test
    fun jwk_thumbprints_match_rfc_vectors() {
        for (case in Fixtures.json("jwk/thumbprints.json")["cases"]!!.jsonArray.map { it.jsonObject }) {
            val j = case["jwk"]!!.jsonObject
            val jwk = Jwk(j["kty"]!!.jsonPrimitive.content, j["crv"]!!.jsonPrimitive.content, j["x"]!!.jsonPrimitive.content, j["y"]?.jsonPrimitive?.content)
            assertEquals(case["thumbprint"]!!.jsonPrimitive.content, jwk.thumbprint())
        }
    }

    @Test
    fun pkce_s256_matches_rfc7636() {
        val f = Fixtures.json("pkce/rfc7636.json")
        assertEquals(f["code_challenge"]!!.jsonPrimitive.content, Pkce.fromVerifier(f["code_verifier"]!!.jsonPrimitive.content).challenge)
        val generated = Pkce.generate()
        assertEquals(43, generated.verifier.length)
    }

    @Test
    fun reasons_round_trip_and_unknown_is_unknown() {
        for (r in Reason.entries.filter { it != Reason.UNKNOWN }) assertEquals(r, Reason.fromWire(r.wire))
        assertEquals(Reason.UNKNOWN, Reason.fromWire("something_new"))
        assertEquals(Reason.UNKNOWN, Reason.fromWire(null))
    }

    /** The tree hash scripts/proto-hash.sh computes, so the app is tied to conformance/proto-ref.lock. */
    @Test
    fun proto_ref_matches_lock() {
        val root = Fixtures.repoRoot
        val lock = Fixtures.file("conformance/proto-ref.lock").readLines()
            .filterNot { it.startsWith("#") || it.isBlank() }
            .associate { it.substringBefore('=') to it.substringAfter('=') }
        val sha = { b: ByteArray -> MessageDigest.getInstance("SHA-256").digest(b).joinToString("") { "%02x".format(it) } }
        val lines = Fixtures.file("proto").walkTopDown().filter { it.isFile && it.name.endsWith(".proto") }
            .map { it.relativeTo(root).invariantSeparatorsPath }
            .sorted()
            .joinToString("") { "${sha(Fixtures.file(it).readBytes())}  $it\n" }
        assertEquals(lock["tree_sha256"], sha(lines.toByteArray()))
    }
}
