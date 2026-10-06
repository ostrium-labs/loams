package dev.loams.test

import com.google.protobuf.ByteString
import dev.loams.CallOptions
import dev.loams.Descriptors
import dev.loams.RefreshingTokenSource
import dev.loams.TokenSource
import dev.loams.UnauthenticatedException
import dev.loams.test.Support.StubTransport
import com.google.protobuf.DynamicMessage

/**
 * `kotlin_token_source_refresh` (R1, D608).
 *
 * A bearer from a token source, and **one** refresh on `token_expired`.
 *
 * # Why the bearer is fetched per attempt
 *
 * That is the whole of the mechanism: the refresh changes the token, and the retry
 * has to carry the new one. A bearer computed once per call would be the stale one
 * on every retry after a refresh, and the failure would look like "the refresh did
 * not work" rather than "the refresh was never used".
 *
 * # Why exactly one
 *
 * A second expiry is reported rather than looped on — a loop is the obvious
 * implementation and it turns a refusal into a hang.
 */
object TokenSourceTest {
    const val NAME = "kotlin_token_source_refresh"

    private val whoAmI = StubTransport.binding("loams.instance.v1.InstanceService/WhoAmI")

    /** A source that answers one token then another, and counts its refreshes. */
    private class RotatingSource(
        private val tokens: MutableList<String>,
        /** Whether this source can refresh at all — an API key cannot. */
        override val canRefresh: Boolean = true,
    ) : TokenSource {
        var refreshes = 0
            private set
        private var at = 0

        override fun token(): String = tokens.getOrElse(at) { tokens.lastOrNull() ?: "" }

        /** What the next attempt will read, once a refresh has happened. */
        fun advance() {
            at++
        }

        override fun refresh() {
            refreshes++
            advance()
        }
    }

    /** The `Authorization` header on the nth request, or null. */
    private fun bearerOn(transport: StubTransport, at: Int): String? =
        transport.requests.getOrNull(at)?.headers?.entries
            ?.firstOrNull { it.key.equals("Authorization", ignoreCase = true) }?.value

    private fun tokenExpired(detail: String): String {
        val bytes = dev.loams.ErrorInfoCodec.encode("token_expired", emptyMap(), "sign in again")
        assertTrue(detail == "", "an empty detail argument")
        return """{"code":"unauthenticated","message":"sign in again","details":[{"type":"loams.errors.v1.ErrorInfo","value":"${
            dev.loams.Base64.encode(bytes)
        }"}]}"""
    }

    fun register() {
        Harness.test("$NAME a bearer is attached to every attempt") {
            val source = RotatingSource(mutableListOf("Bearer token-1"))
            val transport = StubTransport()
            transport.answerMessage(whoAmI.response, DynamicMessage.getDefaultInstance(whoAmI.response))

            dev.loams.LoamsClient("stub://x", transport = transport, tokenSource = source)
                .invoker.unary(whoAmI, DynamicMessage.getDefaultInstance(whoAmI.request), CallOptions())

            assertEquals("Bearer token-1", bearerOn(transport, 0), "the first attempt's bearer")
        }

        Harness.test("$NAME one token_expired gets exactly one refresh and one retry, with the NEW token") {
            val source = RotatingSource(mutableListOf("Bearer stale", "Bearer fresh"))
            val transport = StubTransport()
            transport.answer(401, dev.loams.ContentTypes.CONNECT_UNARY_JSON, tokenExpired("").toByteArray())
            transport.answerMessage(whoAmI.response, DynamicMessage.getDefaultInstance(whoAmI.response))

            dev.loams.LoamsClient("stub://x", transport = transport, tokenSource = source)
                .invoker.unary(whoAmI, DynamicMessage.getDefaultInstance(whoAmI.request), CallOptions())

            assertEquals(2, transport.requests.size, "attempts")
            assertEquals(1, source.refreshes, "refreshes")
            // The whole mechanism: the retry carries the **new** token.
            assertEquals("Bearer stale", bearerOn(transport, 0), "the first attempt's bearer")
            assertEquals("Bearer fresh", bearerOn(transport, 1), "the retry's bearer")
        }

        Harness.test("$NAME a second expiry is reported, not looped on") {
            val source = RotatingSource(mutableListOf("Bearer a", "Bearer b", "Bearer c"))
            val transport = StubTransport()
            transport.answer(401, dev.loams.ContentTypes.CONNECT_UNARY_JSON, tokenExpired("").toByteArray())
            transport.answer(401, dev.loams.ContentTypes.CONNECT_UNARY_JSON, tokenExpired("").toByteArray())
            transport.answerMessage(whoAmI.response, DynamicMessage.getDefaultInstance(whoAmI.response))

            val error = assertThrows<dev.loams.TokenExpiredException>("a second expiry") {
                dev.loams.LoamsClient("stub://x", transport = transport, tokenSource = source)
                    .invoker.unary(whoAmI, DynamicMessage.getDefaultInstance(whoAmI.request), CallOptions())
            }
            assertEquals("token_expired", dev.loams.ReasonRegistry.name(error.reason), "the reason")
            assertEquals(1, source.refreshes, "refreshes (one, not two)")
            assertEquals(2, transport.requests.size, "attempts")
        }

        Harness.test("$NAME a source that cannot refresh skips the retry rather than spending it") {
            // An API key cannot be refreshed, so a `token_expired` against it is a
            // real failure and a retry would ask the same question twice.
            val source = RotatingSource(mutableListOf("Bearer key"), canRefresh = false)
            val transport = StubTransport()
            transport.answer(401, dev.loams.ContentTypes.CONNECT_UNARY_JSON, tokenExpired("").toByteArray())
            transport.answerMessage(whoAmI.response, DynamicMessage.getDefaultInstance(whoAmI.response))

            assertThrows<dev.loams.TokenExpiredException>("a token_expired an API key cannot fix") {
                dev.loams.LoamsClient("stub://x", transport = transport, tokenSource = source)
                    .invoker.unary(whoAmI, DynamicMessage.getDefaultInstance(whoAmI.request), CallOptions())
            }
            assertEquals(1, transport.requests.size, "attempts")
            assertEquals(0, source.refreshes, "refreshes")
        }

        Harness.test("$NAME a refresh that itself fails reports the refresh's failure, not the expiry") {
            // Reporting the original expiry would hide *why* nothing improved.
            val transport = StubTransport()
            transport.answer(401, dev.loams.ContentTypes.CONNECT_UNARY_JSON, tokenExpired("").toByteArray())
            transport.answerConnectError(401, "unauthenticated", "the refresh endpoint refused")

            val failing = object : TokenSource {
                override val canRefresh = true
                private var calls = 0
                override fun token(): String {
                    calls++
                    return "Bearer t$calls"
                }

                override fun refresh() {
                    throw IllegalStateException("the refresh endpoint is down")
                }
            }
            val error = assertThrows<dev.loams.LoamsException>("a failed refresh") {
                dev.loams.LoamsClient("stub://x", transport = transport, tokenSource = failing)
                    .invoker.unary(whoAmI, DynamicMessage.getDefaultInstance(whoAmI.request), CallOptions())
            }
            assertEquals("the refresh endpoint refused", error.message, "the message to be the refresh's own")
        }

        Harness.test("$NAME a caller's own Authorization is dropped, so the refresh stays reachable") {
            // Honouring a caller-supplied bearer would make R1 unreachable for the
            // call that set it: the runtime cannot refresh a token it never minted.
            val source = RotatingSource(mutableListOf("Bearer from-source"))
            val transport = StubTransport()
            transport.answerMessage(whoAmI.response, DynamicMessage.getDefaultInstance(whoAmI.response))

            dev.loams.LoamsClient("stub://x", transport = transport, tokenSource = source)
                .invoker.unary(
                    whoAmI,
                    DynamicMessage.getDefaultInstance(whoAmI.request),
                    CallOptions(headers = mapOf("Authorization" to "Bearer caller-supplied", "X-Trace" to "t1")),
                )

            assertEquals("Bearer from-source", bearerOn(transport, 0), "the bearer that went out")
            assertEquals("t1", transport.requests.single().headers["X-Trace"], "the caller's other headers")
        }

        Harness.test("$NAME a refusal with no ErrorInfo is not token_expired") {
            // `mock_status_unauthenticated` is the recording: HTTP 401, a
            // Connect envelope, and **no** `details` at all. Inventing a reason here
            // would make a client refresh a credential that was simply absent.
            val transport = StubTransport()
            transport.answer(
                401,
                dev.loams.ContentTypes.CONNECT_UNARY_JSON,
                """{"code":"unauthenticated","message":"a bearer token is required"}""".toByteArray(),
            )
            val source = RotatingSource(mutableListOf("Bearer none"))
            val error = assertThrows<UnauthenticatedException>("a bare unauthenticated") {
                dev.loams.LoamsClient("stub://x", transport = transport, tokenSource = source)
                    .invoker.unary(whoAmI, DynamicMessage.getDefaultInstance(whoAmI.request), CallOptions())
            }
            assertEquals(dev.loams.Reason.NONE, error.reason, "the reason (there was no ErrorInfo)")
            assertEquals(0, source.refreshes, "refreshes (nothing to refresh)")
            assertEquals(1, transport.requests.size, "attempts")
        }
    }
}

/**
 * `kotlin_a_refreshing_token_source_mints_once` — the source's own half of R1, which
 * is testable without a network and is what the corpus cannot reach.
 *
 * `mock_error_step_up_required` is the recording of the **server** saying a token is
 * too old; nothing in the corpus exercises the client's own mint, so it is pinned
 * here against a real `RefreshingTokenSource`.
 */
fun refreshingTokenSourceMintsOnce() {
    Harness.test("kotlin_a_refreshing_token_source_mints_once") {
        var minted = 0
        val source = RefreshingTokenSource { minted++ }

        assertTrue(source.canRefresh, "a refreshing source to say it can refresh")
        val first = source.token()
        assertTrue(first.startsWith("Bearer "), "a bearer (got $first)")
        assertEquals(1, minted, "mints")

        // Within its window the same token comes back and nothing is minted again:
        // a token that changed every call would be a refresh storm the server sees.
        assertEquals(first, source.token(), "the token inside its window")
        assertEquals(first, source.token(), "the token inside its window")
        assertEquals(1, minted, "mints inside the window")

        source.refresh()
        val second = source.token()
        assertTrue(second != first, "a refresh to change the token")
        assertEquals(2, minted, "mints after a refresh")
    }
}

/** `kotlin_a_static_token_source_cannot_refresh` — an API key, not a session. */
fun staticTokenSourceCannotRefresh() {
    Harness.test("kotlin_a_static_token_source_cannot_refresh") {
        val source = dev.loams.StaticTokenSource("api-key")
        assertTrue(!source.canRefresh, "a static source to say it cannot refresh")
        assertEquals("api-key", source.token(), "the token")
        assertEquals("api-key", source.token(), "the token again (a static source never re-reads anything)")
    }
}

/** `kotlin_a_token_source_that_returns_nothing_sends_no_Authorization`. */
fun emptyTokenSourceSendsNoAuthorization() {
    Harness.test("kotlin_a_token_source_that_returns_nothing_sends_no_Authorization") {
        val transport = StubTransport()
        val getInstance = StubTransport.binding("loams.instance.v1.InstanceService/GetInstance")
        transport.answerMessage(getInstance.response, DynamicMessage.getDefaultInstance(getInstance.response))

        dev.loams.LoamsClient("stub://x", transport = transport, tokenSource = dev.loams.StaticTokenSource(""))
            .invoker.unary(getInstance, DynamicMessage.getDefaultInstance(getInstance.request), CallOptions())

        assertTrue(bearerOn0(transport) == null, "no Authorization header for an empty token")
    }
}

private fun bearerOn0(transport: StubTransport): String? =
    transport.requests.firstOrNull()?.headers?.entries
        ?.firstOrNull { it.key.equals("Authorization", ignoreCase = true) }?.value

/** `kotlin_the_bearer_is_not_attached_when_there_is_no_source`. */
fun noSourceSendsNoAuthorization() {
    Harness.test("kotlin_the_bearer_is_not_attached_when_there_is_no_source") {
        val transport = StubTransport()
        val getInstance = StubTransport.binding("loams.instance.v1.InstanceService/GetInstance")
        transport.answerMessage(getInstance.response, DynamicMessage.getDefaultInstance(getInstance.response))

        // `GetInstance` needs no credentials, which is what keeps a bearer out of
        // the corpus driver entirely.
        dev.loams.LoamsClient("stub://x", transport = transport)
            .invoker.unary(getInstance, DynamicMessage.getDefaultInstance(getInstance.request), CallOptions())
        assertTrue(bearerOn0(transport) == null, "no Authorization header with no token source")
    }
}

/** `kotlin_the_token_source_is_read_once_per_call_when_there_is_one_attempt`. */
fun tokenSourceReadCounts() {
    Harness.test("kotlin_the_token_source_is_read_once_per_attempt") {
        val transport = StubTransport()
        val getInstance = StubTransport.binding("loams.instance.v1.InstanceService/GetInstance")
        transport.answerConnectError(503, "unavailable", "down")
        transport.answerConnectError(503, "unavailable", "still down")
        transport.answerMessage(getInstance.response, DynamicMessage.getDefaultInstance(getInstance.response))

        var reads = 0
        val source = object : TokenSource {
            override val canRefresh = false
            override fun token(): String {
                reads++
                return "Bearer t$reads"
            }
        }
        dev.loams.LoamsClient("stub://x", transport = transport, tokenSource = source, maxRetries = 2)
            .invoker.unary(getInstance, DynamicMessage.getDefaultInstance(getInstance.request), CallOptions())

        assertEquals(3, transport.requests.size, "attempts")
        assertEquals(3, reads, "token reads (one per attempt, never one per call)")
        assertEquals("Bearer t1", bearerOn0(transport), "the first attempt's token")
    }
}

/** `kotlin_the_bearer_survives_a_caller_header_with_a_different_case`. */
fun bearerHeaderIsCaseInsensitive() {
    Harness.test("kotlin_the_bearer_survives_a_caller_header_with_a_different_case") {
        val transport = StubTransport()
        val getInstance = StubTransport.binding("loams.instance.v1.InstanceService/GetInstance")
        transport.answerMessage(getInstance.response, DynamicMessage.getDefaultInstance(getInstance.response))

        dev.loams.LoamsClient("stub://x", transport = transport, tokenSource = dev.loams.StaticTokenSource("Bearer s"))
            .invoker.unary(
                getInstance,
                DynamicMessage.getDefaultInstance(getInstance.request),
                CallOptions(headers = mapOf("authorization" to "Bearer caller")),
            )
        assertEquals(
            "Bearer s",
            bearerOn0(transport),
            "the source's token (a lower-case caller header must not shadow it either)",
        )
    }
}

/** `kotlin_a_client_without_a_token_source_still_sends_the_callers_headers`. */
fun callersHeadersSurvive() {
    Harness.test("kotlin_a_client_without_a_token_source_still_sends_the_callers_headers") {
        val transport = StubTransport()
        val getInstance = StubTransport.binding("loams.instance.v1.InstanceService/GetInstance")
        transport.answerMessage(getInstance.response, DynamicMessage.getDefaultInstance(getInstance.response))

        dev.loams.LoamsClient("stub://x", transport = transport)
            .invoker.unary(
                getInstance,
                DynamicMessage.getDefaultInstance(getInstance.request),
                CallOptions(headers = mapOf("loams-fixture-name" to "x", "loams-fixture-step" to "0")),
            )
        val headers = transport.requests.single().headers
        assertEquals("x", headers["loams-fixture-name"], "the caller's header")
        assertEquals("0", headers["loams-fixture-step"], "the caller's header")
    }
}

/** A guard that a refused call is still typed after a transport throws. */
fun transportFailureIsTyped() {
    Harness.test("kotlin_a_transport_failure_is_typed") {
        val failing = object : dev.loams.Transport {
            override fun send(request: dev.loams.TransportRequest): dev.loams.TransportResponse =
                throw java.net.SocketTimeoutException("the read timed out")
        }
        val getInstance = StubTransport.binding("loams.instance.v1.InstanceService/GetInstance")
        val error = assertThrows<dev.loams.LoamsException>("a socket timeout") {
            dev.loams.LoamsClient("stub://x", transport = failing)
                .invoker.unary(getInstance, DynamicMessage.getDefaultInstance(getInstance.request), CallOptions())
        }
        assertEquals(dev.loams.Code.DeadlineExceeded, error.code, "a timeout keeps its own code")
        assertEquals(dev.loams.Reason.NONE, error.reason, "a failure from below the API carries no reason")
    }
}

/** A guard that the request body is exactly what the codec produced, unframed. */
fun unaryBodyIsUnframed() {
    Harness.test("kotlin_a_unary_request_body_is_not_framed") {
        val transport = StubTransport()
        val decide = StubTransport.binding("loams.approvals.v1.ApprovalService/DecideApproval")
        val request = DynamicMessage.newBuilder(decide.request)
            .setField(
                decide.request.findFieldByName("approval_id"),
                ByteString.copyFromUtf8("apr_01J9ZCREATEKEY"),
            )
            .build()
        transport.answerMessage(decide.response, DynamicMessage.getDefaultInstance(decide.response))
        transport.client(codec = Codec.PROTO).invoker.unary(decide, request, CallOptions())

        val sent = transport.requests.single()
        assertEquals("/loams.approvals.v1.ApprovalService/DecideApproval", sent.path, "the RPC path")
        assertEquals("POST", sent.method, "the HTTP method")
        assertEquals(ContentTypes.CONNECT_UNARY_PROTO, sent.contentType, "the content type")
        // No 5-byte envelope on a Connect unary: the message *is* the body.
        assertTrue(!sent.body.contentEquals(dev.loams.Envelopes.wrap(sent.body)), "an unframed body")
    }
}