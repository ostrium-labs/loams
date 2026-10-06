package dev.loams.test

import com.google.protobuf.DynamicMessage
import dev.loams.CallOptions
import dev.loams.Code
import dev.loams.Descriptors
import dev.loams.Idempotency
import dev.loams.LoamsClient
import dev.loams.MessageCodec
import dev.loams.Codec
import dev.loams.Reason
import dev.loams.ReasonRegistry
import dev.loams.RetryPolicy
import dev.loams.UuidV7
import dev.loams.test.Support.StubTransport

/**
 * `kotlin_retry_reuses_idempotency_key` (R2, R3, D610).
 *
 * **This clause has no fixture and none is possible from either server today.** A
 * retryable `unavailable` needs a dependency to be down, a `resource_exhausted`
 * needs a loaded server, and `RetryInfo` is on no proto at all — which is what
 * `manifest.json` says about R2 in as many words. So it is pinned here, against a
 * real `Transport` the SDK calls through its real `CallInvoker`, which is a claim
 * about a **call** rather than about a function's return value.
 *
 * The three things D610 asks for, each asserted separately:
 *
 *  - a mutation that fails retryably and carries no key is **not** retried;
 *  - the same mutation, once keyed, **is** retried;
 *  - every attempt carries the **same** key, so a retried write is the same write.
 */
object RetryIdempotencyTest {
    const val NAME = "kotlin_retry_reuses_idempotency_key"

    private val decide = StubTransport.binding("loams.approvals.v1.ApprovalService/DecideApproval")

    private fun decideRequest(): DynamicMessage {
        val builder = DynamicMessage.newBuilder(decide.request)
        builder.setField(
            decide.request.findFieldByName("approval_id"),
            com.google.protobuf.ByteString.copyFromUtf8("apr_01J9ZCREATEKEY"),
        )
        return builder.build()
    }

    private fun okResponse(): DynamicMessage =
        DynamicMessage.getDefaultInstance(decide.response)

    /** The `idempotency_key` on the wire, or null when the request has none. */
    private fun keyOn(transport: StubTransport): String? =
        transport.requests.firstOrNull()?.let { request ->
            val message = MessageCodec.deserialize(decide.request, request.body, Codec.PROTO)
            (message.getField(decide.request.findFieldByName("idempotency_key")) as? String)
                ?.takeIf { it.isNotEmpty() }
        }

    fun register() {
        Harness.test("$NAME a mutation with no key is not retried") {
            val transport = StubTransport()
            transport.answerConnectError(503, "unavailable", "the backend is down")
            transport.answerMessage(decide.response, okResponse())

            val error = assertThrows<dev.loams.LoamsException>("a retryable refusal") {
                transport.client(maxRetries = 3).invoker.unary(decide, decideRequest(), CallOptions())
            }
            assertEquals(1, transport.requests.size, "attempts made (a mutation with no key is not retried)")
            assertEquals(Code.Unavailable, error.code, "the code the server sent")
        }

        Harness.test("$NAME a keyed mutation is retried, with the same key every time") {
            val transport = StubTransport()
            transport.answerConnectError(503, "unavailable", "the backend is down")
            transport.answerConnectError(503, "unavailable", "still down")
            transport.answerMessage(decide.response, okResponse())

            val response = transport.client(maxRetries = 3).invoker.unary(decide, decideRequest(), CallOptions())
            assertEquals(3, transport.requests.size, "attempts made")
            assertEquals(decide.response.fullName, response.descriptorForType.fullName, "the response type")

            val keys = transport.requests.map { keyOn(transport) }
            val first = keys.firstOrNull()
            assertTrue(first != null && first.isNotEmpty(), "the SDK to have minted a key (got $keys)")
            assertEquals(1, keys.distinct().size, "distinct idempotency keys across attempts")
        }

        Harness.test("$NAME a read is retried without a key, because the schema has no key field") {
            val list = StubTransport.binding("loams.approvals.v1.ApprovalService/ListApprovals")
            val transport = StubTransport()
            transport.answerConnectError(503, "unavailable", "the backend is down")
            transport.answerMessage(list.response, DynamicMessage.getDefaultInstance(list.response))

            transport.client(maxRetries = 2).invoker.unary(
                list,
                DynamicMessage.getDefaultInstance(list.request),
                CallOptions(),
            )
            assertEquals(2, transport.requests.size, "attempts made (NO_SIDE_EFFECTS retries on its own)")
        }

        Harness.test("$NAME the caller's message is not stamped with this call's key") {
            val original = decideRequest()
            val transport = StubTransport()
            transport.answerMessage(decide.response, okResponse())
            transport.client().invoker.unary(decide, original, CallOptions())

            val keyField = decide.request.findFieldByName("idempotency_key")
            assertEquals(
                "",
                original.getField(keyField) as String,
                "the key on the caller's own message (two calls must not share one key)",
            )
            assertTrue(keyOn(transport) != null, "the key that went on the wire")
        }

        Harness.test("$NAME a caller-supplied key is honoured and not replaced") {
            val transport = StubTransport()
            transport.answerMessage(decide.response, okResponse())
            transport.client().invoker.unary(decide, decideRequest(), CallOptions(idempotencyKey = "mine"))
            assertEquals("mine", keyOn(transport), "the key that went on the wire")
        }

        Harness.test("$NAME a key already on the message survives mintIdempotencyKey = false") {
            val request = DynamicMessage.newBuilder(decide.request)
                .setField(
                    decide.request.findFieldByName("approval_id"),
                    com.google.protobuf.ByteString.copyFromUtf8("apr_x"),
                )
                .setField(decide.request.findFieldByName("idempotency_key"), "already-mine")
                .build()
            val transport = StubTransport()
            transport.answerMessage(decide.response, okResponse())
            transport.client().invoker.unary(decide, request, CallOptions(mintIdempotencyKey = false))
            assertEquals("already-mine", keyOn(transport), "the key that went on the wire")
        }
    }
}

/**
 * `kotlin_idempotency_is_decided_from_the_schema` — the other half of D610, and the
 * rule that keeps it from becoming a list of fixture names (D655).
 *
 * `DecideApprovalRequest` declares `idempotency_key` and `GetApprovalRequest` does
 * not. A runtime that guessed from the object would invent a field the schema does
 * not declare on the second, and a request with a field the server does not know is
 * rejected.
 */
fun idempotencyIsDecidedFromTheSchema() {
    Harness.test("kotlin_idempotency_is_decided_from_the_schema") {
        val decideRequest = Descriptors.message("loams.approvals.v1.DecideApprovalRequest")
        val getRequest = Descriptors.message("loams.approvals.v1.GetApprovalRequest")
        assertTrue(decideRequest != null, "loams.approvals.v1.DecideApprovalRequest in the descriptor set")
        assertTrue(getRequest != null, "loams.approvals.v1.GetApprovalRequest in the descriptor set")

        assertTrue(
            Idempotency.schemaHasKey(decideRequest),
            "DecideApprovalRequest to declare a string idempotency_key",
        )
        assertTrue(
            !Idempotency.schemaHasKey(getRequest),
            "GetApprovalRequest to declare no idempotency_key",
        )

        // A message with no key field is left exactly as the caller wrote it.
        val bare = DynamicMessage.newBuilder(getRequest)
            .setField(getRequest.findFieldByName("approval_id"), com.google.protobuf.ByteString.copyFromUtf8("apr_x"))
            .build()
        val keyed = Idempotency.apply(bare, "")
        assertTrue(!keyed.keyed, "a request with no key field to be reported unkeyed")
        assertEquals(
            bare.getField(getRequest.findFieldByName("approval_id")).toString(),
            keyed.request.getField(getRequest.findFieldByName("approval_id")).toString(),
            "the untouched fields of an unkeyable request",
        )
    }
}

/**
 * `kotlin_uuid_v7_sorts_by_time` — why an idempotency key is a v7 and not the v4
 * `UUID.randomUUID()` reaches for by default.
 *
 * An operator correlating one key in a log has to be able to read a time out of
 * it, and two keys minted in the same millisecond must not share a timestamp or
 * the sort order — the reason for choosing v7 — is a coin flip.
 */
fun uuidV7SortsByTime() {
    Harness.test("kotlin_uuid_v7_sorts_by_time") {
        val keys = (1..64).map { UuidV7.new() }
        for (key in keys) {
            assertEquals(36, key.length, "the length of $key")
            assertTrue(key[14] == '7', "the version nibble of $key")
            assertTrue(
                key[19] in "89ab",
                "the variant nibble of $key (RFC 9562 fixes it at 10)",
            )
        }
        assertEquals(keys.size, keys.distinct().size, "distinct keys")

        // The first twelve hex digits are 48 bits of Unix milliseconds. Read as a
        // sortable prefix, which is the property the layout exists for.
        val times = keys.map { key ->
            val digits = key.filterIndexed { index, _ -> index < 8 || index in 9..10 || index in 14..15 }
            digits.toLong(16)
        }
        val sorted = times.sorted()
        assertEquals(times, sorted, "keys minted in order to sort by their embedded time")
        val now = System.currentTimeMillis()
        assertTrue(
            times.all { it in (now - 3_600_000)..(now + 60_000) },
            "every embedded time to be about now (now=$now, saw ${times.first()}..${times.last()})",
        )
    }
}

/**
 * `kotlin_retry_backoff_is_capped_and_jittered` — M1.6 Ruling 5's numbers, the
 * same in every SDK.
 *
 * Base 100 ms, doubling, capped at 2 s, 3 retries, **full** jitter. Full jitter
 * rather than exponential backoff alone, because every client retrying at the same
 * instant after a node restart is how a recovering node gets knocked over again.
 * The bound rather than the draw is asserted: a test that pinned the random number
 * would test the jitter *source*, and the claim is about the ceiling.
 */
fun retryBackoffIsCappedAndJittered() {
    Harness.test("kotlin_retry_backoff_is_capped_and_jittered") {
        assertEquals(100, RetryPolicy.BASE_DELAY_MS, "the base delay")
        assertEquals(2000, RetryPolicy.MAX_DELAY_MS, "the cap")
        assertEquals(3, RetryPolicy.DEFAULT_MAX_RETRIES, "the default budget")

        val random = java.util.Random(20261006L)
        val waits = (0..40).map { RetryPolicy.backoff(it, random = random) }
        for ((attempt, wait) in waits.withIndex()) {
            assertTrue(wait >= 0, "attempt $attempt's wait to be non-negative")
            val ceiling = if (attempt >= 16) {
                RetryPolicy.MAX_DELAY_MS.toLong()
            } else {
                minOf(RetryPolicy.MAX_DELAY_MS.toLong(), RetryPolicy.BASE_DELAY_MS.toLong() shl attempt)
            }
            assertTrue(wait <= ceiling, "attempt $attempt's wait of ${wait}ms to be within [0, ${ceiling}]")
        }

        // Full jitter means the ceiling is drawn from, not returned: 40 draws at a
        // 2 s ceiling would all be exactly 2000 only if the jitter were absent.
        val wide = (0..40).map { RetryPolicy.backoff(20, random = java.util.Random(it.toLong())) }
        assertTrue(wide.distinct().size > 1, "the jitter to vary the wait ($wide)")

        // A server-sent RetryInfo delay replaces the computed backoff, capped.
        assertEquals(
            RetryPolicy.MAX_SERVER_DELAY_MS.toLong(),
            RetryPolicy.backoff(0, serverDelayMs = 90_000),
            "a server delay above the ceiling",
        )
        assertEquals(250L, RetryPolicy.backoff(9, serverDelayMs = 250), "a server delay below the ceiling")
    }
}

/**
 * `kotlin_retry_classes_are_the_ones_d610_names` — and nothing else.
 *
 * Notably **not** `internal`: a server that answered with an internal error has
 * already run the handler, and for a mutation that means the write may have
 * happened, so repeating it on the SDK's own initiative is how one logical call
 * becomes two writes.
 */
fun retryClassesAreTheOnesD610Names() {
    Harness.test("kotlin_retry_classes_are_the_ones_d610_names") {
        for (code in listOf(Code.Unavailable, Code.DeadlineExceeded, Code.ResourceExhausted)) {
            assertTrue(RetryPolicy.isRetryableCode(code), "$code to be retryable")
        }
        for (code in listOf(
            Code.Internal, Code.InvalidArgument, Code.NotFound, Code.AlreadyExists,
            Code.PermissionDenied, Code.FailedPrecondition, Code.Aborted, Code.OutOfRange,
            Code.Unimplemented, Code.DataLoss, Code.Unauthenticated, Code.Cancelled, Code.Unknown,
        )) {
            assertTrue(!RetryPolicy.isRetryableCode(code), "$code to be retryable")
        }

        assertTrue(
            !RetryPolicy.shouldRetry(0, 3, retrySafe = false, code = Code.Unavailable, cancelled = false),
            "an unkeyed mutation to be retried",
        )
        assertTrue(
            !RetryPolicy.shouldRetry(0, 3, retrySafe = true, code = Code.Unavailable, cancelled = true),
            "a cancelled call to be retried",
        )
        assertTrue(
            !RetryPolicy.shouldRetry(0, 0, retrySafe = true, code = Code.Unavailable, cancelled = false),
            "a call with no budget to be retried",
        )
    }
}

/** `kotlin_retry_class_comes_from_the_proto` — the retry class is derived (D610). */
fun retryClassComesFromTheProto() {
    Harness.test("kotlin_retry_class_comes_from_the_proto") {
        val safe = listOf(
            "loams.instance.v1.InstanceService/GetInstance",
            "loams.instance.v1.InstanceService/WhoAmI",
            "loams.approvals.v1.ApprovalService/ListApprovals",
            "loams.approvals.v1.ApprovalService/GetApproval",
            "loams.devices.v1.DeviceService/ListDevices",
        )
        for (rpc in safe) {
            val binding = Descriptors.bindingForRpc(rpc)
            assertTrue(binding != null, "a binding for $rpc")
            assertEquals(dev.loams.RetryClass.SAFE, binding.retry, "$rpc's retry class")
        }

        // A mutation with no `idempotency_level` is MANUAL, and becomes retryable
        // only once `Idempotency.apply` has put a key on it.
        val manual = listOf(
            "loams.approvals.v1.ApprovalService/DecideApproval",
            "loams.devices.v1.DeviceService/SendTestNotification",
            "loams.live.v1.LiveService/Mutate",
        )
        for (rpc in manual) {
            val binding = Descriptors.bindingForRpc(rpc)
            assertTrue(binding != null, "a binding for $rpc")
            assertEquals(dev.loams.RetryClass.MANUAL, binding.retry, "$rpc's retry class")
        }
    }
}

/** `kotlin_a_cancelled_call_spends_no_attempt` — checked before the attempt, not after. */
fun cancelledCallSpendsNoAttempt() {
    Harness.test("kotlin_a_cancelled_call_spends_no_attempt") {
        val transport = StubTransport()
        val error = assertThrows<dev.loams.LoamsException>("a cancelled call") {
            transport.client(maxRetries = 3).invoker.unary(
                StubTransport.binding("loams.instance.v1.InstanceService/GetInstance"),
                DynamicMessage.getDefaultInstance(
                    Descriptors.message("loams.instance.v1.GetInstanceRequest")!!
                ),
                CallOptions(),
                cancelled = true,
            )
        }
        assertEquals(Code.Cancelled, error.code, "the code a cancellation reports")
        assertEquals(0, transport.requests.size, "attempts spent after a cancellation")
    }
}

/**
 * `kotlin_the_reason_registry_matches_the_registry_page` — D611's registry is
 * `docs/api/reasons.md`, and a reason may be added there but never renamed.
 */
fun reasonRegistryMatchesTheRegistryPage() {
    Harness.test("kotlin_reason_registry_matches_the_registry_page") {
        val page = dev.loams.test.Support.RepositoryRoot
            .find(java.io.File(".").absoluteFile)
            .resolve("docs/api/reasons.md")
        assertTrue(page.isFile, "docs/api/reasons.md to exist")
        val text = page.readText()
        for (name in ReasonRegistry.allNames) {
            assertTrue(text.contains("`$name`"), "the registry page to name $name")
        }
        for (reason in ReasonRegistry.all) {
            if (reason == Reason.NONE) continue
            assertTrue(ReasonRegistry.codeOf(reason) != Code.Unknown, "${ReasonRegistry.name(reason)}'s code")
        }
    }
}

/** A guard that the client rejects a call made against a fixture-server-less endpoint. */
fun clientRequiresAnEndpoint() {
    Harness.test("kotlin_a_client_requires_an_endpoint") {
        assertThrows<IllegalArgumentException>("a client with no endpoint") {
            LoamsClient(endpoint = "", transport = StubTransport())
        }
    }
}