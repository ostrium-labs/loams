package dev.loams.test.Support

import dev.loams.CallBinding
import dev.loams.Codec
import dev.loams.ContentTypes
import dev.loams.Envelopes
import dev.loams.LoamsClient
import dev.loams.MessageCodec
import dev.loams.Protocol
import com.google.protobuf.DynamicMessage

/**
 * An in-process transport, for the clauses the corpus cannot reach.
 *
 * # Why some clauses need one
 *
 * `sdks/fixtures/manifest.json` says plainly which of R1–R10 have no fixture and
 * **cannot** have one from either server today: R2 (a retryable `unavailable`
 * needs a dependency to be down, and `RetryInfo` is on no proto at all), R4 (no
 * RPC carries a `consistency_token`), and R6 (no paged RPC exists on any server,
 * so `ListApprovals` declaring `page_size` and `next_page_token` and honouring
 * neither is the *recording* that pins the fact). Those three clauses are pinned
 * here, against a real `Transport` the SDK calls through its real `CallInvoker`,
 * rather than by asserting on the retry function's own return value — which would
 * be a claim about a function rather than about a call.
 *
 * It speaks the **same wire** the fixture server does — the same content types,
 * the same 5-byte envelope — so a stub that answered something the real server
 * could not would be testing a fiction.
 */
class StubTransport : dev.loams.Transport {
    /** One recorded request, as it went out. */
    data class Seen(
        val rpc: String,
        val path: String,
        val contentType: String,
        val headers: Map<String, String>,
        val body: ByteArray,
    )

    /** One scripted answer. */
    data class Answer(
        val status: Int,
        val contentType: String,
        val body: ByteArray,
        val headers: Map<String, String> = emptyMap(),
    )

    private val answers = mutableListOf<Answer>()
    private val seen = mutableListOf<Seen>()

    /** Every request the SDK made, in order. */
    val requests: List<Seen> get() = seen.toList()

    /** Queues one answer, consumed in order. */
    fun answer(status: Int, contentType: String, body: ByteArray, headers: Map<String, String> = emptyMap()): StubTransport {
        answers.add(Answer(status, contentType, body, headers))
        return this
    }

    /** Queues a successful unary answer carrying [message] in [codec]. */
    fun answerMessage(
        descriptor: com.google.protobuf.Descriptors.Descriptor,
        message: DynamicMessage,
        codec: Codec = Codec.PROTO,
        protocol: Protocol = Protocol.CONNECT,
    ): StubTransport = answer(
        status = 200,
        contentType = when {
            protocol == Protocol.GRPC_WEB && codec == Codec.JSON -> ContentTypes.GRPC_JSON
            protocol == Protocol.GRPC_WEB -> ContentTypes.GRPC_PROTO
            codec == Codec.JSON -> ContentTypes.CONNECT_UNARY_JSON
            else -> ContentTypes.CONNECT_UNARY_PROTO
        },
        body = MessageCodec.serialize(message, codec),
    )

    /** Queues a Connect error envelope: the shape a unary refusal carries. */
    fun answerConnectError(status: Int, code: String, message: String, details: String = ""): StubTransport {
        val detail = if (details.isEmpty()) "" else ""","details":[{"type":"loams.errors.v1.ErrorInfo","value":"$details"}]"""
        return answer(
            status = status,
            contentType = ContentTypes.CONNECT_UNARY_JSON,
            body = """{"code":"$code","message":"$message"$detail}""".toByteArray(Charsets.UTF_8),
        )
    }

    /** Queues a framed answer for a server stream. */
    fun answerStream(vararg payloads: ByteArray, endError: String = "{}"): StubTransport {
        val body = java.io.ByteArrayOutputStream()
        for (payload in payloads) {
            body.write(Envelopes.wrap(payload))
        }
        body.write(Envelopes.wrap(endError.toByteArray(Charsets.UTF_8), dev.loams.EnvelopeFlags.END_OF_STREAM))
        return answer(200, ContentTypes.CONNECT_STREAM_PROTO, body.toByteArray())
    }

    /** The single scripted answer, asserted to be the only one. */
    private fun take(): Answer {
        check(answers.isNotEmpty()) { "the stub has no answer queued for this request" }
        return answers.removeAt(0)
    }

    override fun send(request: dev.loams.TransportRequest): dev.loams.TransportResponse {
        seen.add(Seen(request.rpc, request.path, request.contentType, request.headers, request.body))
        val answer = take()
        return dev.loams.TransportResponse(answer.status, answer.headers, answer.contentType, answer.body)
    }

    /** A client bound to this transport, with no retries unless asked for. */
    fun client(maxRetries: Int = 0): LoamsClient = LoamsClient(
        endpoint = "stub://in-process",
        protocol = Protocol.CONNECT,
        codec = Codec.PROTO,
        maxRetries = maxRetries,
        transport = this,
    )

    companion object {
        /** The binding for an RPC in the committed descriptor set, or a clear failure. */
        fun binding(rpc: String): CallBinding = requireNotNull(dev.loams.Descriptors.bindingForRpc(rpc)) {
            "the committed descriptor set declares no RPC '$rpc'"
        }
    }
}