package dev.loams.test

import com.google.protobuf.ByteString
import com.google.protobuf.DynamicMessage
import dev.loams.CallOptions
import dev.loams.Code
import dev.loams.ContentTypes
import dev.loams.Descriptors
import dev.loams.EnvelopeFlags
import dev.loams.Envelopes
import dev.loams.MessageCodec
import dev.loams.Codec
import dev.loams.StreamCursor
import dev.loams.StreamResume
import dev.loams.Streaming
import dev.loams.test.Support.StubTransport

/**
 * `kotlin_stream_resume_with_cursor` (R7).
 *
 * A stream is not "retried like a unary call". A unary call is idempotent or it is
 * not, so the retry decision is a property of the call. A stream is different: the
 * server hands out cursors, and a client that reconnects from the beginning
 * re-yields everything the caller already saw, while a client that reconnects from
 * nothing misses everything that changed while it was away. **Both are worse than an
 * error** — one duplicates, the other silently misses — so a stream reconnects from
 * the last cursor the caller applied and carries on.
 *
 * Two halves, and they interact:
 *
 *  - **nothing yielded yet means resume is safe** — re-opening from scratch would
 *    duplicate nothing, because nothing was seen;
 *  - **something yielded means the re-open must carry a cursor** — once the caller
 *    holds a position, replaying from the start duplicates and resuming from nothing
 *    misses.
 *
 * # Heartbeats are liveness, not data
 *
 * The server sends an empty frame on a timer so proxies do not idle the connection
 * out. Yielding those to a `for` loop would render an empty row every fifteen
 * seconds, so a heartbeat is counted and not yielded. The corpus's
 * `mock_state_stream_heartbeat` is the recording of the distinction: **two frames,
 * one message**.
 */
object StreamResumeTest {
    const val NAME = "kotlin_stream_resume_with_cursor"

    private val watch = StubTransport.binding("loams.approvals.v1.ApprovalService/WatchApprovals")

    /**
     * A `WatchApprovalsResponse` carrying one `oneof` case, built from the schema.
     *
     * Which case and which cursor come from the arguments, so no message shape is
     * written out by hand here — the point of a descriptor-driven SDK is that a
     * fixture's response is built the way the server built it.
     */
    private fun response(case: String, cursor: String = "", snapshotReset: Boolean = false): ByteArray {
        val descriptor = watch.response
        val builder = DynamicMessage.newBuilder(descriptor)
        builder.setField(descriptor.findFieldByName("cursor"), ByteString.copyFromUtf8(cursor))
        builder.setField(descriptor.findFieldByName("snapshot_reset"), snapshotReset)

        val caseField = descriptor.fields.firstOrNull { field ->
            !field.isRepeated && field.containingOneof != null && field.type ==
                com.google.protobuf.Descriptors.FieldDescriptor.Type.MESSAGE
        }
        requireNotNull(caseField) { "WatchApprovalsResponse declares no message case in a oneof" }
        val caseDescriptor = caseField.messageType
        val payload = when (case) {
            "snapshot" -> {
                val approval = caseDescriptor.findFieldByName("snapshot")
                if (approval != null) DynamicMessage.newBuilder(approval.messageType).build() else null
            }
            "upsert" -> {
                val approval = caseDescriptor.findFieldByName("upsert")
                if (approval != null) {
                    val one = approval.messageType
                    DynamicMessage.newBuilder(one)
                        .setField(one.findFieldByName("approval_id"), ByteString.copyFromUtf8("apr_new"))
                        .build()
                } else null
            }
            else -> DynamicMessage.newBuilder(caseDescriptor.findFieldByName(case).messageType).build()
        }
        requireNotNull(payload) { "WatchApprovalsResponse has no '$case' case" }
        builder.setField(caseField, payload)
        return MessageCodec.serialize(builder.build(), Codec.PROTO)
    }

    /** The `resume_cursor` on a recorded request, or null when it carries none. */
    private fun resumeCursorOn(transport: StubTransport, at: Int): String? =
        transport.requests.getOrNull(at)?.let { request ->
            val message = MessageCodec.deserialize(watch.request, request.body, Codec.PROTO)
            (message.getField(watch.request.findFieldByName("resume_cursor")) as? String)?.takeIf { it.isNotEmpty() }
        }

    /** A resume policy over the real request and response types. */
    private fun policy(maxRetries: Int = 1) = object : StreamResume {
        override val maxRetries: Int = maxRetries

        override fun cursorOf(message: DynamicMessage): StreamCursor {
            val cursor = message.getField(message.descriptorForType.findFieldByName("cursor")) as? String
            return StreamCursor(cursor ?: "")
        }

        override fun resume(cursor: StreamCursor, request: DynamicMessage): DynamicMessage {
            val field = request.descriptorForType.findFieldByName("resume_cursor")
                ?: return request
            return DynamicMessage.newBuilder(request).setField(field, cursor.value).build()
        }
    }

    fun register() {
        Harness.test("$NAME a heartbeat is counted and not yielded") {
            val transport = StubTransport()
            transport.answerStream(response("snapshot", cursor = "c0"), response("heartbeat"))

            val handle = transport.client().invoker.serverStream(
                watch, DynamicMessage.getDefaultInstance(watch.request), CallOptions(), policy(),
            )
            val messages = handle.messages.toList()
            assertEquals(1, messages.size, "messages the caller saw")
            assertEquals(2, handle.frameKinds.size, "frames on the wire")
            assertEquals(listOf("snapshot", "heartbeat"), handle.frameKinds, "the frame kinds, in order")
            assertEquals(1, handle.heartbeats, "heartbeats counted")
        }

        Harness.test("$NAME a stream is described by the descriptor, not by a name") {
            assertEquals(Streaming.SERVER, watch.streaming, "WatchApprovals's streaming")
            assertEquals(
                "loams.approvals.v1.WatchApprovalsResponse",
                watch.response.fullName,
                "WatchApprovals's response type",
            )
            val list = Descriptors.bindingForRpc("loams.approvals.v1.ApprovalService/ListApprovals")
            assertEquals(Streaming.UNARY, list!!.streaming, "ListApprovals's streaming")
        }

        Harness.test("$NAME a retryable failure after a message resumes from that message's cursor") {
            val transport = StubTransport()
            transport.answerStream(response("snapshot", cursor = "c0"), response("upsert", cursor = "c1"))
            transport.answerConnectError(503, "unavailable", "the stream broke")

            val handle = transport.client().invoker.serverStream(
                watch, DynamicMessage.getDefaultInstance(watch.request), CallOptions(), policy(),
            )
            val messages = handle.messages.toList()
            assertEquals(2, messages.size, "messages before the break")
            assertEquals(1, handle.reconnects, "reconnects")

            // The re-open carried the cursor of the message the caller actually saw.
            // `c1` is the upsert's cursor; a client that resumed from the start would
            // have re-sent an empty `resume_cursor` and duplicated the snapshot.
            assertEquals(2, transport.requests.size, "requests made")
            assertEquals("c1", resumeCursorOn(transport, 1), "the cursor the re-open carried")
        }

        Harness.test("$NAME a failure before any message resumes from nothing, because nothing duplicated") {
            val transport = StubTransport()
            transport.answerConnectError(503, "unavailable", "the stream broke")
            transport.answerStream(response("snapshot", cursor = "c0"))

            val handle = transport.client().invoker.serverStream(
                watch, DynamicMessage.getDefaultInstance(watch.request), CallOptions(), policy(),
            )
            assertEquals(1, handle.messages.toList().size, "messages after the reconnect")
            assertEquals(1, handle.reconnects, "reconnects")
            assertEquals(null, resumeCursorOn(transport, 1), "the cursor of a re-open with nothing yielded")
        }

        Harness.test("$NAME a failure the retry class does not cover is reported, not spun on") {
            val transport = StubTransport()
            transport.answerConnectError(501, "unimplemented", "not in this variant")
            transport.answerStream(response("snapshot", cursor = "c0"))

            val error = assertThrows<dev.loams.LoamsException>("an unimplemented stream") {
                transport.client().invoker.serverStream(
                    watch, DynamicMessage.getDefaultInstance(watch.request), CallOptions(), policy(),
                ).messages.toList()
            }
            assertEquals(Code.Unimplemented, error.code, "the code")
            assertEquals(1, transport.requests.size, "requests (an unimplemented stream is not re-opened)")
        }

        Harness.test("$NAME a refusal inside a 200's end frame is a refusal, not a clean end") {
            // `live_watch` is the recording: HTTP 200, and the refusal is the
            // end-of-stream frame's payload, which is JSON whatever the codec is.
            val detail = dev.loams.Base64.encode(
                dev.loams.ErrorInfoCodec.encode("feature_not_in_variant", mapOf("variant" to "standard"), "")
            )
            val end = """{"error":{"code":"unimplemented","message":"Watch is not in the standard variant",""" +
                """"details":[{"type":"loams.errors.v1.ErrorInfo","value":"$detail"}]}}"""
            val body = Envelopes.wrap(end.toByteArray(Charsets.UTF_8), EnvelopeFlags.END_OF_STREAM)

            val frames = dev.loams.WireReader.readStream(
                body, 200, dev.loams.Protocol.CONNECT, "loams.live.v1.LiveService/Watch",
            )
            assertEquals(0, frames.messages.size, "messages before the refusal")
            assertTrue(frames.failure != null, "a refusal in the end-of-stream frame")
            assertEquals(Code.Unimplemented, frames.failure!!.code, "the code")
            assertEquals("feature_not_in_variant", frames.failure.detail?.reason, "the reason")
            assertEquals("standard", frames.failure.detail?.metadata?.get("variant"), "the variant")
        }

        Harness.test("$NAME a gRPC-Web refusal arrives in a trailers frame, after the messages") {
            val detail = dev.loams.Base64.encode(
                dev.loams.ErrorInfoCodec.encode("not_implemented", emptyMap(), "")
            )
            val trailers = buildString {
                append("grpc-status: 12\r\n")
                append("grpc-message: not implemented\r\n")
                append("grpc-status-details-bin: $detail\r\n")
            }.toByteArray(Charsets.UTF_8)
            val body = Envelopes.wrap("hello".toByteArray()) +
                Envelopes.wrap(trailers, EnvelopeFlags.TRAILERS)

            val read = dev.loams.WireReader.readStream(
                body, 200, dev.loams.Protocol.GRPC_WEB, "loams.devices.v1.DeviceService/SendTestNotification",
            )
            assertEquals(1, read.messages.size, "messages before the trailers frame")
            assertTrue(read.failure != null, "a refusal in the trailers frame")
            assertEquals(Code.Unimplemented, read.failure!!.code, "the code")
            assertEquals("not_implemented", read.failure.detail?.reason, "the reason")
        }

        Harness.test("$NAME a compressed frame is reported rather than mis-parsed") {
            // Nothing in this SDK compresses a request and nothing in the corpus
            // compresses a response, so a compressed frame is a server that ignored
            // the request. Passing it on as data would be worse than saying so.
            val body = Envelopes.wrap("payload".toByteArray(), EnvelopeFlags.COMPRESSED)
            val error = assertThrows<dev.loams.LoamsException>("a compressed frame") {
                dev.loams.WireReader.readStream(body, 200, dev.loams.Protocol.CONNECT, "some.Rpc/Method").messages
            }
            assertEquals(Code.Internal, error.code, "the code a compressed frame reports")
        }

        Harness.test("$NAME a server stream over gRPC-Web is framed on the request") {
            val transport = StubTransport()
            transport.answerStream(response("snapshot", cursor = "c0"))
            transport.client(codec = Codec.PROTO, protocol = dev.loams.Protocol.GRPC_WEB).invoker.serverStream(
                watch, DynamicMessage.getDefaultInstance(watch.request), CallOptions(), policy(),
            ).messages.toList()
            val body = transport.requests.single().body
            // 5-byte envelope, flags 0, then the message.
            assertEquals(0, body[0].toInt(), "the envelope's flag byte")
            assertEquals(response("snapshot", cursor = "c0").size, body.size - 5, "the framed payload's length")
        }
    }
}

/**
 * `kotlin_a_stream_sends_a_framed_request` — the encoding rule, derived.
 *
 * Connect streaming and gRPC-Web both put a 5-byte header in front of every
 * message; `application/json` and `application/proto` do not. A client that framed
 * a unary request, or that sent a stream's request bare, fails the byte comparison
 * against the recording.
 */
fun streamRequestsAreFramed() {
    Harness.test("kotlin_a_stream_sends_a_framed_request") {
        val watch = StubTransport.binding("loams.approvals.v1.ApprovalService/WatchApprovals")
        val list = StubTransport.binding("loams.approvals.v1.ApprovalService/ListApprovals")

        val streamTransport = StubTransport()
        streamTransport.answerStream(ByteArray(0))
        streamTransport.client().invoker.serverStream(
            watch, DynamicMessage.getDefaultInstance(watch.request), CallOptions(), resume = null,
        ).messages.toList()
        assertEquals(ContentTypes.CONNECT_STREAM_PROTO, streamTransport.requests.single().contentType, "a Connect stream's content type")
        assertEquals(5, streamTransport.requests.single().body.size, "a framed empty message")

        val unaryTransport = StubTransport()
        unaryTransport.answerMessage(list.response, DynamicMessage.getDefaultInstance(list.response))
        unaryTransport.client().invoker.unary(list, DynamicMessage.getDefaultInstance(list.request), CallOptions())
        assertEquals(ContentTypes.CONNECT_UNARY_PROTO, unaryTransport.requests.single().contentType, "a Connect unary's content type")
        assertEquals(0, unaryTransport.requests.single().body.size, "an unframed empty message")
    }
}