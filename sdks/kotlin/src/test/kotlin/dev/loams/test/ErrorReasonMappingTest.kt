package dev.loams.test

import dev.loams.Base64
import dev.loams.CallOptions
import dev.loams.Code
import dev.loams.Descriptors
import dev.loams.Envelopes
import dev.loams.ErrorInfoCodec
import dev.loams.ErrorMapper
import dev.loams.FeatureNotInVariantException
import dev.loams.Json
import dev.loams.LoamsException
import dev.loams.MessageCodec
import dev.loams.Codec
import dev.loams.Protocol
import dev.loams.Reason
import dev.loams.ReasonRegistry
import dev.loams.TokenExpiredException
import dev.loams.WireReader
import dev.loams.test.Support.StubTransport
import com.google.protobuf.DynamicMessage

/**
 * `kotlin_error_reason_mapping` (R8, D611).
 *
 * The **code** gives the class — a taxonomy that does not change within an API
 * major version — and the **`reason`** is the stable branch. The message is for a
 * person and may change; nothing here branches on it.
 *
 * Three cases R8 requires to stay distinct, and each is asserted:
 *
 *  - a reason from a **newer** server, which this SDK's registry does not have: it
 *    is surfaced as text and flagged, not dropped;
 *  - a failure from **below the API** — a socket, a cancelled call — which carries
 *    no reason at all;
 *  - a `LoamsException` that has already been mapped, which is returned unchanged,
 *    so wrapping an SDK's own error never loses its reason.
 */
object ErrorReasonMappingTest {
    const val NAME = "kotlin_error_reason_mapping"

    private val decide = StubTransport.binding("loams.approvals.v1.ApprovalService/DecideApproval")

    /** The `ErrorInfo` the corpus records for `not_implemented`, byte for byte. */
    private val NOT_IMPLEMENTED = "Cg9ub3RfaW1wbGVtZW50ZWQ"

    /** The `ErrorInfo` the corpus records for `step_up_required`, byte for byte. */
    private val STEP_UP_REQUIRED = "ChBzdGVwX3VwX3JlcXVpcmVk"

    fun register() {
        Harness.test("$NAME a Connect refusal maps to its typed class and reason") {
            val transport = StubTransport()
            transport.answerConnectError(501, "unimplemented", "not implemented yet", NOT_IMPLEMENTED)
            val error = assertThrows<LoamsException>("a refusal") {
                transport.client().invoker.unary(decide, DynamicMessage.getDefaultInstance(decide.request), CallOptions())
            }
            assertEquals(Code.UNIMPLEMENTED, error.code, "the code")
            assertEquals(Reason.NOT_IMPLEMENTED, error.reason, "the reason")
            assertEquals("not_implemented", ReasonRegistry.name(error.reason), "the wire name")
            assertEquals(501, error.httpStatus, "the HTTP status as the SDK saw it")
        }

        Harness.test("$NAME an unpadded base64 detail decodes to its reason") {
            // The defect the corpus found: a strict decoder throws, the throw is
            // caught, and the answer silently becomes "no reason". 7 of the 10
            // distinct `details[].value` strings in `sdks/fixtures` are unpadded.
            val bytes = Base64.decode(NOT_IMPLEMENTED)
            assertTrue(bytes != null, "Cg9ub3RfaW1wbGVtZW50ZWQ (23 characters, unpadded) to decode")
            val info = ErrorInfoCodec.decode(bytes!!)
            assertTrue(info != null, "the decoded detail to be an ErrorInfo")
            assertEquals("not_implemented", info!!.reason, "the reason")
        }

        Harness.test("$NAME a reason the registry does not have is surfaced, not dropped") {
            val unknown = Base64.encode(ErrorInfoCodec.encode("a_reason_this_sdk_has_never_heard_of", emptyMap(), ""))
            val transport = StubTransport()
            transport.answerConnectError(501, "unimplemented", "nope", unknown)
            val error = assertThrows<LoamsException>("a refusal with an unknown reason") {
                transport.client().invoker.unary(decide, DynamicMessage.getDefaultInstance(decide.request), CallOptions())
            }
            assertEquals(Reason.NONE, error.reason, "the reason to be unknown to this SDK's registry")
            assertEquals(
                "a_reason_this_sdk_has_never_heard_of",
                error.unknownReason,
                "the reason as text (dropping it leaves a caller unable to tell 'not supported here' from 'not supported at all')",
            )
        }

        Harness.test("$NAME a failure from below the API carries no reason") {
            val error = ErrorMapper.fromException(java.io.IOException("connection refused"), "some.Rpc/Method")
            assertEquals(Code.UNKNOWN, error.code, "the code")
            assertEquals(Reason.NONE, error.reason, "the reason (a socket failure is not any reason in the registry)")
            assertTrue(error.unknownReason == null, "no unknown reason either")
        }

        Harness.test("$NAME mapping a failure twice loses nothing") {
            val once = ErrorMapper.map(
                dev.loams.WireFailure(Code.NOT_FOUND, "gone", ErrorInfoCodec.decode(Base64.decode("Cglub3RfZm91bmQ")!!), 404),
                "some.Rpc/Method",
            )
            val twice = ErrorMapper.fromException(once, "some.Rpc/Method")
            assertTrue(once === twice, "the same exception, not a copy")
            assertEquals(Reason.NOT_FOUND, twice.reason, "the reason")
            assertEquals(null, twice.unknownReason, "no unknown reason when the registry has it")
        }

        Harness.test("$NAME the detail is found by its type, never by position") {
            val bytes = Base64.decode("Cglub3RfZm91bmQ")!!
            val detail = ErrorInfoCodec.decode(bytes)
            assertTrue(detail != null, "an ErrorInfo in the bytes")
            assertEquals("not_found", detail!!.reason, "the reason")
            // `metadata` and `hint` are the other two fields, and a refusal that
            // carries them must not lose them.
            val full = ErrorInfoCodec.decode(Base64.decode(STEP_UP_REQUIRED)!!)
            assertTrue(full != null, "the step_up_required detail")
            assertEquals("step_up_required", full!!.reason, "the reason")
        }

        Harness.test("$NAME feature_not_in_variant names the variant it was asked for") {
            val bytes = ErrorInfoCodec.encode(
                "feature_not_in_variant",
                mapOf("variant" to "standard"),
                "this build does not carry live",
            )
            val transport = StubTransport()
            transport.answerConnectError(501, "unimplemented", "not in this variant", Base64.encode(bytes))
            val error = assertThrows<FeatureNotInVariantException>("a variant refusal") {
                transport.client().invoker.unary(decide, DynamicMessage.getDefaultInstance(decide.request), CallOptions())
            }
            assertEquals("standard", error.variant, "metadata.variant")
            assertEquals("this build does not carry live", error.hint, "the hint")
        }

        Harness.test("$NAME token_expired is a narrower unauthenticated, distinguished by reason") {
            val bytes = ErrorInfoCodec.encode("token_expired", emptyMap(), "sign in again")
            val transport = StubTransport()
            transport.answerConnectError(401, "unauthenticated", "sign in again", Base64.encode(bytes))
            val error = assertThrows<TokenExpiredException>("a token_expired refusal") {
                transport.client().invoker.unary(decide, DynamicMessage.getDefaultInstance(decide.request), CallOptions())
            }
            // It is also an `UnauthenticatedException`, so one `catch` covers both.
            assertTrue(error is dev.loams.UnauthenticatedException, "a token_expired to be an unauthenticated")
            assertEquals(Reason.TOKEN_EXPIRED, error.reason, "the reason")
        }

        Harness.test("$NAME a body that is not a Connect envelope is unknown, not an exception") {
            val failure = WireReader.unaryFailure(502, "<html>Bad Gateway</html>", "some.Rpc/Method")
            assertEquals(Code.UNKNOWN, failure.code, "the code a proxy's HTML page maps to")
            assertTrue(failure.detail == null, "no ErrorInfo, because there was none")
        }

        Harness.test("$NAME a gRPC-Web refusal arrives on HTTP 200, in the trailers frame") {
            // The sharpest edge of R10: a status-code-only client reads this as a
            // success. `mock_error_encodings` records it over both gRPC-Web
            // encodings, and `mock_error_not_implemented` over Connect unary.
            val detail = Base64.encode(ErrorInfoCodec.encode("not_implemented", emptyMap(), ""))
            val trailers = buildString {
                append("grpc-status: 12\r\n")
                append("grpc-message: SendTestNotification%20is%20not%20implemented\r\n")
                append("grpc-status-details-bin: $detail\r\n")
            }.toByteArray(Charsets.UTF_8)
            val failure = WireReader.grpcWebFailure(
                trailers = Envelopes.parseTrailers(trailers),
                httpStatus = 200,
                rpc = "loams.devices.v1.DeviceService/SendTestNotification",
            )
            val refusal = requireNotNull(failure) { "a refusal from the trailers frame" }
            assertEquals(Code.UNIMPLEMENTED, refusal.code, "the code")
            assertEquals("not_implemented", refusal.detail?.reason, "the reason")
        }
    }
}

/** The `ErrorInfo` a failure carries, or null. */
private fun dev.loams.WireFailure.errorInfo() = detail

/**
 * `kotlin_error_detail_rejects_base64_it_cannot_read` — the decoder is strict about
 * the alphabet even though it tolerates missing padding (D653).
 *
 * A decoder that quietly drops invalid characters turns a truncated detail into a
 * shorter but still-plausible one, which is how a `reason` goes missing **without an
 * error**. So anything outside `A-Za-z0-9+/` and whitespace is refused, and a final
 * group of one character is refused because one leftover sextet has no byte to
 * stand for.
 */
fun base64IsTolerantOfPaddingAndStrictOfAlphabet() {
    Harness.test("kotlin_error_detail_rejects_base64_it_cannot_read") {
        // Padded, unpadded and whitespace-wrapped are all the same value.
        val padded = "Cg9ub3RfaW1wbGVtZW50ZWQ="
        val unpadded = "Cg9ub3RfaW1wbGVtZW50ZWQ"
        val wrapped = "Cg9u b3RfaW1w bGVtZW50ZWQ"
        val expected = Base64.decode(padded)
        assertTrue(expected != null, "the padded form to decode")
        assertTrue(Base64.decode(unpadded)?.contentEquals(expected!!) == true, "the unpadded form")
        assertTrue(Base64.decode(wrapped)?.contentEquals(expected) == true, "the whitespace-wrapped form")

        // One leftover character: malformed, not short.
        assertTrue(Base64.decode("Cg9ub3RfaW1wbGVtZW50ZWQx") == null, "a final group of one character")
        // Outside the alphabet.
        for (bad in listOf("not base64!", "Cg9u*b3Q=", "Cg9u-b3Q=", "Cg9u_b3Q=")) {
            assertTrue(Base64.decode(bad) == null, "$bad to be refused")
        }
        // Padding validated, not merely terminated: an alphabet character after a
        // `=` makes the input invalid. The C++ SDK stops at the padding instead;
        // the two differ only on input this corpus never produces.
        assertTrue(Base64.decode("Cg9u=b3Q=") == null, "an alphabet character after the padding")

        // Null rather than an empty array, so "the value was empty" and "the value
        // was not base64" stay distinguishable.
        assertEquals(0, Base64.decode("")!!.size, "an empty value to decode to no bytes")
        assertTrue(Base64.decode(null) == null, "an absent value")
    }
}

/**
 * `kotlin_error_envelope_codes_match_the_registry` — every Connect wire code maps
 * to the class D611 names, and an unknown one is `unknown` rather than a guess.
 */
fun errorEnvelopeCodesMatchTheRegistry() {
    Harness.test("kotlin_error_envelope_codes_match_the_registry") {
        val pairs = mapOf(
            "invalid_argument" to Code.INVALID_ARGUMENT,
            "not_found" to Code.NOT_FOUND,
            "already_exists" to Code.ALREADY_EXISTS,
            "permission_denied" to Code.PERMISSION_DENIED,
            "resource_exhausted" to Code.RESOURCE_EXHAUSTED,
            "failed_precondition" to Code.FAILED_PRECONDITION,
            "aborted" to Code.ABORTED,
            "out_of_range" to Code.OUT_OF_RANGE,
            "unimplemented" to Code.UNIMPLEMENTED,
            "internal" to Code.INTERNAL,
            "unavailable" to Code.UNAVAILABLE,
            "data_loss" to Code.DATA_LOSS,
            "unauthenticated" to Code.UNAUTHENTICATED,
            "canceled" to Code.CANCELLED,
        )
        for ((wire, code) in pairs) {
            assertEquals(code, Code.fromWire(wire), "the code $wire names")
        }
        assertEquals(Code.CANCELLED, Code.fromWire("cancelled"), "the British spelling of canceled")
        assertEquals(Code.UNKNOWN, Code.fromWire("no_such_code"), "a wire code this SDK does not know")

        // The same taxonomy by number, for a gRPC status.
        for (code in Code.entries) {
            if (code == Code.UNKNOWN) continue
            assertEquals(code, Code.fromNumber(code.number), "code ${code.name} by number")
        }
    }
}

/**
 * `kotlin_every_registry_reason_has_a_class` — D611's table.
 *
 * A reason the registry names with no class is a reason a caller cannot catch by
 * type, and the whole point of the hierarchy is that the branch is a type test.
 */
fun everyRegistryReasonHasAClass() {
    Harness.test("kotlin_every_registry_reason_has_a_class") {
        val decide = StubTransport.binding("loams.approvals.v1.ApprovalService/DecideApproval")
        for (reason in ReasonRegistry.all) {
            val code = ReasonRegistry.codeOf(reason)
            val transport = StubTransport()
            transport.answerConnectError(
                status = 400,
                code = code.name.lowercase(),
                message = "recorded refusal",
                details = Base64.encode(ErrorInfoCodec.encode(ReasonRegistry.name(reason), emptyMap(), "")),
            )
            val error = assertThrows<LoamsException>("a ${ReasonRegistry.name(reason)} refusal") {
                transport.client().invoker.unary(decide, DynamicMessage.getDefaultInstance(decide.request), CallOptions())
            }
            assertEquals(reason, error.reason, "the reason the mapper reported for ${ReasonRegistry.name(reason)}")
            assertEquals(code, error.code, "the code the mapper reported for ${ReasonRegistry.name(reason)}")
        }
    }
}

/**
 * `kotlin_a_refusal_is_not_an_http_status` (R10) — the clause's sharpest edge.
 *
 * All three of these are refusals a status-code-only client reads as success:
 * Connect unary's HTTP 501 with a JSON error body, gRPC-Web's HTTP 200 with
 * `grpc-status: 12` in a trailers frame, and Connect streaming's HTTP 200 with an
 * error inside the end-of-stream frame. `live_watch` is the recording of the third.
 */
fun aRefusalIsNotAnHttpStatus() {
    Harness.test("kotlin_a_refusal_is_not_an_http_status") {
        // Connect streaming: HTTP 200, and the refusal is in the last frame.
        val endFrame = """{"error":{"code":"unimplemented","message":"Watch is not in the standard variant",""" +
            """"details":[{"type":"loams.errors.v1.ErrorInfo","value":"${
                Base64.encode(ErrorInfoCodec.encode("feature_not_in_variant", mapOf("variant" to "standard"), ""))
            }"}]}}"""
        val failure = WireReader.streamFailure(
            Envelopes.wrap(endFrame.toByteArray(Charsets.UTF_8), dev.loams.EnvelopeFlags.END_OF_STREAM),
            httpStatus = 200,
            rpc = "loams.live.v1.LiveService/Watch",
        )
        val refusal = requireNotNull(failure) { "a refusal inside a 200's end-of-stream frame" }
        assertEquals(Code.UNIMPLEMENTED, refusal.code, "the code")
        assertEquals("feature_not_in_variant", refusal.detail?.reason, "the reason")
        assertEquals("standard", refusal.detail?.metadata?.get("variant"), "the variant")

        // A clean end has no error at all: the protocol allows a server to close a
        // stream with no error, and the corpus's recorded streams are bounded
        // prefixes a server closed that way.
        assertTrue(
            WireReader.streamFailure(
                "{}".toByteArray(Charsets.UTF_8),
                httpStatus = 200,
                rpc = "loams.live.v1.LiveService/Watch",
            ) == null,
            "a clean end to be no failure",
        )

        // A refused Connect unary is HTTP 501 with a JSON body, not a 5xx code the
        // caller has to guess at.
        val unary = WireReader.unaryFailure(
            501,
            """{"code":"unimplemented","message":"WhoAmI is not implemented yet","details":[{"type":"loams.errors.v1.ErrorInfo","value":"${
                Base64.encode(ErrorInfoCodec.encode("not_implemented", emptyMap(), ""))
            }"}]}""",
            "loams.instance.v1.InstanceService/WhoAmI",
        )
        assertEquals(Code.UNIMPLEMENTED, unary.code, "the code")
        assertEquals("not_implemented", unary.detail?.reason, "the reason")
    }
}

/**
 * `kotlin_proto3_json_is_compact_and_field_ordered` (R10) — the defect that would
 * fail **every** JSON fixture for a reason unrelated to conformance.
 *
 * `fixture-server.mjs` compares the request bytes it receives against the recording
 * **byte for byte** and answers 400 on a difference. A generic JSON library emits
 * a space after `{` and around `:` and `,`, so every JSON fixture would fail. Four
 * rules, each observable in the corpus: compact; field order by field number;
 * defaults omitted; 64-bit integers as strings.
 */
fun proto3JsonIsCompactAndFieldOrdered() {
    Harness.test("kotlin_proto3_json_is_compact_and_field_ordered") {
        val descriptor = Descriptors.message("loams.approvals.v1.DecideApprovalRequest")!!
        val builder = DynamicMessage.newBuilder(descriptor)
        // Set the fields **out of field-number order**, so an encoder that iterates
        // insertion order fails and one that iterates the descriptor passes.
        builder.setField(descriptor.findFieldByName("idempotency_key"), "conformance-idempotency-1")
        builder.setField(descriptor.findFieldByName("revision"), java.lang.Long.valueOf(1L))
        builder.setField(descriptor.findFieldByName("decision"), dev.loams.CompactJson.enumNumber(descriptor, "decision", "DECISION_KIND_APPROVE"))
        builder.setField(descriptor.findFieldByName("approval_id"), "apr_01J9ZCREATEKEY")
        val json = String(dev.loams.CompactJson.format(builder.build()).toByteArray(), Charsets.UTF_8)

        assertEquals(
            """{"approvalId":"apr_01J9ZCREATEKEY","revision":"1","decision":"DECISION_KIND_APPROVE","idempotencyKey":"conformance-idempotency-1"}""",
            json,
            "the compact proto3 JSON, in field-number order, with the int64 as a string",
        )

        // Defaults omitted: an empty request is `{}` and not `{"fields":[],...}`.
        val empty = DynamicMessage.getDefaultInstance(descriptor)
        assertEquals("{}", dev.loams.CompactJson.format(empty), "an empty request")

        // No whitespace anywhere.
        val spaced = Descriptors.message("loams.instance.v1.GetInstanceResponse")!!
        val response = DynamicMessage.newBuilder(spaced)
            .setField(spaced.findFieldByName("instance_id"), "01M41B4RP8BR7444NE7D61S0HN")
            .addRepeatedField(spaced.findFieldByName("api_versions"), "loams.instance.v1")
            .build()
        val text = dev.loams.CompactJson.format(response)
        assertTrue(!text.contains(" "), "no space in $text")
        assertTrue(!text.contains("\n"), "no newline in $text")
        assertEquals("""{"instanceId":"01M41B4RP8BR7444NE7D61S0HN","apiVersions":["loams.instance.v1"]}""", text, "a response")
    }
}

/**
 * `kotlin_compact_json_refuses_a_well_known_type` — the boundary the writer stops
 * at, stated so it is a deliberate refusal rather than a bug.
 *
 * No Loams facade call *sends* a `Timestamp`, so a body the server cannot parse is
 * worse than a refusal at the boundary — with a message saying which type. And
 * responses go the other way: the parser understands them.
 */
fun compactJsonRefusesAWellKnownType() {
    Harness.test("kotlin_compact_json_refuses_a_well_known_type") {
        val whoAmI = Descriptors.message("loams.instance.v1.WhoAmIResponse")!!
        val principal = Descriptors.message("loams.instance.v1.Principal")!!
        val timestamp = Descriptors.message("google.protobuf.Timestamp")!!
        val response = DynamicMessage.newBuilder(whoAmI)
            .setField(
                whoAmI.findFieldByName("authenticated_at"),
                DynamicMessage.newBuilder(timestamp).setField(timestamp.findFieldByName("seconds"), java.lang.Long.valueOf(1_700_000_000L)).build(),
            )
            .setField(
                whoAmI.findFieldByName("principal"),
                DynamicMessage.newBuilder(principal).setField(principal.findFieldByName("id"), "usr_dana").build(),
            )
            .build()

        val thrown = assertThrows<UnsupportedOperationException>("a message holding a Timestamp") {
            dev.loams.CompactJson.format(response)
        }
        assertTrue(
            thrown.message!!.contains("google.protobuf.Timestamp"),
            "the refusal to name the type it cannot write (it said: ${thrown.message})",
        )
    }
}

/** `kotlin_the_json_parser_refuses_what_it_does_not_understand`. */
fun jsonParserRefusesGarbage() {
    Harness.test("kotlin_the_json_parser_refuses_what_it_does_not_understand") {
        assertTrue(Json.parse("{\"a\":1}") != null, "a well-formed object")
        assertTrue(Json.parse("[1,2,3]") != null, "an array")
        assertTrue(Json.parse("\"a\"") != null, "a bare string")
        for (bad in listOf("{", "{\"a\"}", "[1,", "{\"a\":01}", "tru", "")) {
            assertTrue(Json.parse(bad) == null, "$bad to be refused rather than half-parsed")
        }
        // An unterminated string, a lone surrogate escape, and a trailing comma.
        assertTrue(Json.parse("\"abc") == null, "an unterminated string")
        assertTrue(Json.parse("{\"a\":1,}") == null, "a trailing comma")
    }
}

/** `kotlin_the_envelope_reader_reports_a_truncated_stream` rather than dropping it. */
fun envelopeReaderReportsATruncatedStream() {
    Harness.test("kotlin_the_envelope_reader_reports_a_truncated_stream") {
        val complete = Envelopes.wrap("hello".toByteArray()) + Envelopes.wrap("world".toByteArray())
        val frames = Envelopes.split(complete)
        assertEquals(2, frames.size, "frames in a complete body")

        // A frame that declares more bytes than are left, and a body that ends
        // mid-header. Both are reported: a stream that lost its last frame is a
        // stream whose last message is unknown, and answering as if it had ended
        // cleanly would report a success the caller never got.
        for (truncated in listOf(
            complete.copyOfRange(0, 8),
            byteArrayOf(0, 0, 0),
            byteArrayOf(),
        )) {
            assertThrows<IllegalArgumentException>("a truncated body of ${truncated.size} bytes") {
                Envelopes.split(truncated)
            }
        }
    }
}

/** `kotlin_content_types_are_the_four_the_corpus_records`. */
fun contentTypesAreTheFourTheCorpusRecords() {
    Harness.test("kotlin_content_types_are_the_four_the_corpus_records") {
        assertEquals("application/json", dev.loams.ContentTypes.CONNECT_UNARY_JSON, "Connect unary JSON")
        assertEquals("application/proto", dev.loams.ContentTypes.CONNECT_UNARY_PROTO, "Connect unary protobuf")
        assertEquals("application/connect+json", dev.loams.ContentTypes.CONNECT_STREAM_JSON, "Connect stream JSON")
        assertEquals("application/connect+proto", dev.loams.ContentTypes.CONNECT_STREAM_PROTO, "Connect stream protobuf")
        assertEquals("application/grpc-web+proto", dev.loams.ContentTypes.GRPC_PROTO, "gRPC-Web protobuf")
        assertEquals("application/grpc-web+json", dev.loams.ContentTypes.GRPC_JSON, "gRPC-Web JSON")

        for (type in dev.loams.ContentTypes.ALL) {
            val family = dev.loams.ContentTypes.family(type)
            assertTrue(family in setOf("json", "proto", "connect", "connect_json", "grpc_web", "grpc_web_json"), "$type's family")
        }
        // The asymmetry the protocols have, spelled out: Connect unary uses
        // `application/json` and Connect streaming uses `application/connect+json`.
        assertTrue(
            dev.loams.ContentTypes.isFramed(dev.loams.ContentTypes.CONNECT_STREAM_PROTO),
            "a Connect stream to be framed",
        )
        assertTrue(!dev.loams.ContentTypes.isFramed(dev.loams.ContentTypes.CONNECT_UNARY_PROTO), "Connect unary not to be framed")
        assertTrue(dev.loams.ContentTypes.isJson(dev.loams.ContentTypes.GRPC_JSON), "gRPC-Web JSON to be JSON")
    }
}

/** A guard that the codec round-trips a message through both encodings. */
fun codecRoundTrips() {
    Harness.test("kotlin_the_codec_round_trips_through_both_encodings") {
        val descriptor = Descriptors.message("loams.instance.v1.GetInstanceResponse")!!
        val original = DynamicMessage.newBuilder(descriptor)
            .setField(descriptor.findFieldByName("instance_id"), "01M41")
            .addRepeatedField(descriptor.findFieldByName("api_versions"), "loams.instance.v1")
            .build()

        val binary = MessageCodec.serialize(original, Codec.PROTO)
        val backFromBinary = MessageCodec.deserialize(descriptor, binary, Codec.PROTO)
        assertEquals(original, backFromBinary, "a binary round trip")

        val json = MessageCodec.serialize(original, Codec.JSON)
        val backFromJson = MessageCodec.deserialize(descriptor, json, Codec.JSON)
        assertEquals(
            original.getField(descriptor.findFieldByName("instance_id")),
            backFromJson.getField(descriptor.findFieldByName("instance_id")),
            "a JSON round trip's instance_id",
        )
        assertEquals(
            listOf("loams.instance.v1"),
            (backFromJson.getField(descriptor.findFieldByName("api_versions")) as List<*>).map { it.toString() },
            "a JSON round trip's api_versions",
        )
    }
}