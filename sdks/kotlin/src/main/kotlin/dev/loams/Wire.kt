package dev.loams

/**
 * The wire: encodings, framing, and the parts of a response the SDK has to read
 * before it can say anything about a call (design §44 §4, R10).
 *
 * One transport serves the Connect protocol and gRPC-Web (D600), so the only
 * question a Kotlin client has is which of those it would rather speak. Both are
 * spoken here over `java.net.http.HttpClient`, and the choice is two enums: a
 * [Protocol] and a [Codec]. **There is no gRPC dependency** — see `DEPENDENCIES.md`
 * — because Connect over HTTP/1.1 is what the corpus records and gRPC's HTTP/2-only
 * transport would be a second code path for the same wire.
 *
 * # What is shared and what is not
 *
 * Framing is shared: Connect streaming and gRPC-Web both put a 5-byte header in
 * front of every message, and the header means the same thing in both. So [Envelope]
 * is one type and one reader, and the difference between the two protocols is
 * entirely in how the **end** of a response arrives —
 *
 *  - Connect streaming: a final frame with the end-of-stream flag, whose payload is
 *    a JSON `EndStreamResponse` whether the codec is proto or JSON (this is
 *    specified, and the corpus depends on it — `live_watch` is a `+proto` stream
 *    whose end frame is JSON);
 *  - gRPC-Web: a trailers-only frame with the high flag bit, carrying `grpc-status`,
 *    `grpc-message` and `grpc-status-details-bin`.
 *
 * Everything else — the error envelope, `ErrorInfo`, the codes — is shared too, and
 * lives in `Errors.kt`.
 *
 * # Where a failure can hide
 *
 * The clause this file exists for is R10, and its sharpest edge is that **a refusal
 * is often not an HTTP status**. All three of these are refusals a status-code-only
 * client reads as success:
 *
 *  - Connect unary: HTTP 501 with a JSON error body.
 *  - gRPC-Web: HTTP 200, `grpc-status: 12` in a trailers frame.
 *  - Connect streaming: HTTP 200, an error inside the end-of-stream frame.
 *
 * So every path below parses the body before it decides anything, and none of them
 * treats a bare 200 as proof of anything.
 */

/** The wire protocol a client speaks. Both are served on one port. */
enum class Protocol {
    /**
     * The Connect protocol. The default: its unary form is an HTTP POST with a JSON
     * or protobuf body — the same bytes `curl` sends (design §44 §4) — and its
     * server streaming is a plain chunked HTTP response that works over HTTP/1.1.
     */
    CONNECT,

    /**
     * gRPC-Web: gRPC's framing with the trailers moved into the body, which is what
     * makes it work over HTTP/1.1 and from a browser with no proxy (R10).
     */
    GRPC_WEB,
}

/** The message encoding on the wire. */
enum class Codec {
    /**
     * Binary protobuf. What an SDK sends by default, and what the corpus calls the
     * encoding "an SDK sends by default".
     */
    PROTO,

    /**
     * The proto3 JSON mapping, which is what `curl` sends and what a browser SDK asks
     * for. Also what an error body always is, whatever the codec.
     */
    JSON,
}

/**
 * The content types this SDK sends and accepts (R10, and
 * `sdks/conformance/encodings.mjs` for the families the corpus keys on).
 *
 * They are spelled out rather than composed from a base and a suffix, because the
 * corpus is keyed on the exact strings and a helper that composes them would be one
 * more thing to keep in step with `fixture-server.mjs`'s `family()`. Note the
 * asymmetry the protocols themselves have: Connect **unary** uses `application/json`
 * and `application/proto`, while Connect **streaming** uses
 * `application/connect+json` and `application/connect+proto`. gRPC-Web uses one type
 * for both.
 */
object ContentTypes {
    /** Connect unary, JSON — what `curl` sends. */
    const val CONNECT_UNARY_JSON: String = "application/json"

    /** Connect unary, binary protobuf. */
    const val CONNECT_UNARY_PROTO: String = "application/proto"

    /** Connect server-streaming, JSON. */
    const val CONNECT_STREAM_JSON: String = "application/connect+json"

    /** Connect server-streaming, binary protobuf. */
    const val CONNECT_STREAM_PROTO: String = "application/connect+proto"

    /** gRPC-Web, binary protobuf. */
    const val GRPC_PROTO: String = "application/grpc-web+proto"

    /** gRPC-Web, JSON. */
    const val GRPC_JSON: String = "application/grpc-web+json"

    /** Every content type, for the family's own sake. */
    val ALL: List<String> = listOf(
        CONNECT_UNARY_JSON, CONNECT_UNARY_PROTO, CONNECT_STREAM_JSON,
        CONNECT_STREAM_PROTO, GRPC_PROTO, GRPC_JSON,
    )

    /** The `content-type` a (protocol, codec, streaming) triple sends. */
    fun forCall(protocol: Protocol, codec: Codec, streaming: Streaming): String = when (protocol) {
        Protocol.CONNECT ->
            if (streaming == Streaming.SERVER) {
                if (codec == Codec.JSON) CONNECT_STREAM_JSON else CONNECT_STREAM_PROTO
            } else {
                if (codec == Codec.JSON) CONNECT_UNARY_JSON else CONNECT_UNARY_PROTO
            }
        Protocol.GRPC_WEB -> if (codec == Codec.JSON) GRPC_JSON else GRPC_PROTO
    }

    /**
     * The **family** of a content type: `json`, `proto`, `grpc_web`,
     * `grpc_web_json`, `connect` or `connect_json`.
     *
     * The fixture corpus is keyed on the family rather than the exact string, so this
     * is what `fixture-server.mjs` matches on and what a test compares. Mirrors
     * `sdks/conformance/encodings.mjs`'s `family()`, including its ordering: an exact
     * `grpc-web+json` before a prefix `grpc-web`, and an exact `connect+json` before a
     * prefix `connect`, because collapsing those two would make six `WatchApprovals`
     * fixtures share one recorded response.
     */
    fun family(contentType: String?): String {
        val type = (contentType ?: "").substringBefore(';').trim()
        return when {
            type == GRPC_JSON -> "grpc_web_json"
            type.startsWith("application/grpc-web") -> "grpc_web"
            type == CONNECT_STREAM_JSON -> "connect_json"
            type.startsWith("application/connect") -> "connect"
            type == CONNECT_UNARY_JSON -> "json"
            type == CONNECT_UNARY_PROTO -> "proto"
            type.isEmpty() -> "none"
            else -> type
        }
    }

    /**
     * Whether an encoding frames messages in the 5-byte envelope.
     *
     * Connect streaming and both gRPC-Web encodings do; `application/json` and
     * `application/proto` do not. This is what makes a stream's request and a unary's
     * differ in the recorded bytes — `live_watch`'s request is `00 00 00 00 00` and
     * `instance_get_instance_json`'s is `{}` — and getting it backwards fails the byte
     * comparison against the recording.
     */
    fun isFramed(contentType: String): Boolean {
        val type = contentType.substringBefore(';').trim()
        return type.startsWith("application/connect") || type.startsWith("application/grpc-web")
    }

    /** Whether an encoding carries the **proto3 JSON mapping** rather than protobuf binary. */
    fun isJson(contentType: String): Boolean {
        val type = contentType.substringBefore(';').trim()
        return type == CONNECT_UNARY_JSON || type == CONNECT_STREAM_JSON || type == GRPC_JSON
    }

    /** Whether an encoding is a gRPC-Web one, whose trailers carry the status. */
    fun isGrpcWeb(contentType: String): Boolean = contentType.substringBefore(';').trim().startsWith("application/grpc-web")
}

/** The flags in an envelope's first byte. */
object EnvelopeFlags {
    /** An ordinary message frame. What a caller sees as data. */
    const val NONE: Int = 0

    /**
     * The payload is compressed.
     *
     * Nothing in this SDK compresses a request and nothing in the corpus compresses a
     * response, so a compressed frame is a server that ignored the request. It is
     * reported rather than silently mis-parsed.
     */
    const val COMPRESSED: Int = 0b0000_0001

    /**
     * The end-of-stream frame. Its payload is a JSON `EndStreamResponse` whatever the
     * codec is — the protocol defines it that way, and the corpus's `live_watch` case
     * is exactly a proto stream whose end frame is JSON.
     */
    const val END_OF_STREAM: Int = 0b0000_0010

    /**
     * The trailers frame, gRPC-Web only. The high bit, so it cannot collide with a
     * Connect flag.
     */
    const val TRAILERS: Int = 0b1000_0000

    /** The flags byte as a value, for a test that names a recorded frame's flags. */
    fun of(flags: Int): Int = flags
}

/** One decoded frame: its flags and its payload. */
data class Envelope(val flags: Int, val payload: ByteArray) {
    override fun equals(other: Any?): Boolean =
        this === other || (other is Envelope && flags == other.flags && payload.contentEquals(other.payload))

    override fun hashCode(): Int = flags * 31 + payload.contentHashCode()
}

/**
 * Reads and writes the 5-byte envelope. Both directions, and no state: a caller owns
 * the stream and decides when the next frame starts.
 */
object Envelopes {
    /** The header's own size, so a reader can skip it. */
    const val HEADER_LENGTH: Int = 5

    /** Frames a payload as one message. */
    fun wrap(payload: ByteArray, flags: Int = EnvelopeFlags.NONE): ByteArray {
        val out = ByteArrayOutput(HEADER_LENGTH + payload.size)
        out.write(flags)
        out.writeUInt32(payload.size)
        out.write(payload)
        return out.toByteArray()
    }

    /** Concatenates several frames into one buffer. */
    fun wrapAll(frames: List<Pair<Int, ByteArray>>): ByteArray {
        val out = ByteArrayOutput()
        for ((flags, payload) in frames) {
            out.write(wrap(payload, flags))
        }
        return out.toByteArray()
    }

    /**
     * Splits a body into its frames.
     *
     * A truncated tail is **reported** rather than dropped: a stream that lost its
     * last frame is a stream whose last message is unknown, and answering as if it had
     * ended cleanly would report a success the caller never got. The same for a body
     * that is empty — a framed body always has at least one frame, so an empty one is a
     * server that sent nothing, which is not a stream that cleanly ended.
     *
     * @throws IllegalArgumentException the body ends mid-frame, or is empty where a
     *   framed body cannot be.
     */
    fun split(body: ByteArray): List<Envelope> {
        if (body.isEmpty()) {
            throw IllegalArgumentException("the framed body is empty, so it carries no frame at all")
        }
        val frames = mutableListOf<Envelope>()
        var at = 0
        while (at + HEADER_LENGTH <= body.size) {
            val flags = body[at].toInt() and 0xFF
            val length = readUInt32(body, at + 1)
            if (length < 0 || at + HEADER_LENGTH + length > body.size) {
                throw IllegalArgumentException(
                    "the stream's frame at offset $at declares $length bytes and only " +
                        "${body.size - at - HEADER_LENGTH} are left"
                )
            }
            frames.add(Envelope(flags, body.copyOfRange(at + HEADER_LENGTH, at + HEADER_LENGTH + length)))
            at += HEADER_LENGTH + length
        }
        if (at != body.size) {
            throw IllegalArgumentException(
                "the stream ended with ${body.size - at} bytes that are not a whole frame"
            )
        }
        return frames
    }

    /** Reads a trailers frame's text into a map, last value wins. */
    fun parseTrailers(payload: ByteArray): Map<String, String> {
        val trailers = LinkedHashMap<String, String>()
        for (line in String(payload, Charsets.UTF_8).split("\r\n")) {
            if (line.isEmpty()) continue
            val colon = line.indexOf(':')
            // A line with no colon is not a trailer. Skipping it is right: the frame is
            // text, and refusing to read a frame because of one line would turn a
            // malformed trailer into a lost status.
            if (colon > 0) {
                trailers[line.substring(0, colon).trim()] = line.substring(colon + 1).trim()
            }
        }
        return trailers
    }

    private fun readUInt32(bytes: ByteArray, at: Int): Int =
        ((bytes[at].toInt() and 0xFF) shl 24) or
            ((bytes[at + 1].toInt() and 0xFF) shl 16) or
            ((bytes[at + 2].toInt() and 0xFF) shl 8) or
            (bytes[at + 3].toInt() and 0xFF)
}

/**
 * A Connect protocol error body: what a unary refusal and a stream's end frame both
 * carry.
 *
 * This is the shape the corpus's Connect encodings produce
 * (`{"code","message","details":[{"type","value"}]}`) and the same one `connect-kotlin`
 * and `connect-go` write. `value` is base64 of the serialized detail and `type` is the
 * detail's type name — both present, because an SDK that reads `details[0].type` to
 * decide what it is holding has to find the type there.
 */
data class ConnectErrorBody(
    /** The Connect code, as its `snake_case` wire name. */
    val code: String,
    /** The human-readable message, which no branch in this SDK reads. */
    val message: String,
    /** The detail list, as it arrived. */
    val details: List<ConnectDetail>,
)

/** One detail of a Connect error body. */
data class ConnectDetail(
    /** The detail's type name, for example `loams.errors.v1.ErrorInfo`. */
    val type: String,
    /** Base64 of the serialized detail. */
    val value: String,
)

/** The `EndStreamResponse` a Connect stream's last frame carries. */
data class EndStreamResponse(val error: ConnectErrorBody?)

/** What one stream body turned out to hold. */
data class StreamRead(
    /** The data frames, in order, decoded. */
    val messages: List<com.google.protobuf.DynamicMessage>,
    /** The failure the stream ended in, or null when it ended cleanly. */
    val failure: WireFailure?,
)

/**
 * Turns the shapes above into the [WireFailure] the mapper reads.
 *
 * Split out from the transport so the parsing is testable without a socket, and so
 * there is exactly one place that knows a refusal can arrive three ways.
 */
object WireReader {
    /**
     * Parses a Connect unary error body (HTTP 4xx or 5xx with a JSON body).
     *
     * A body that is not a Connect error envelope becomes a [Code.UNKNOWN] failure
     * carrying the raw text, rather than an exception: a proxy that answers
     * `502 Bad Gateway` with an HTML page is a failure the caller must see as a
     * failure, and its code really is unknown.
     */
    fun unaryFailure(httpStatus: Int, body: String, rpc: String): WireFailure {
        val parsed = tryParseConnectErrorBody(body)
            ?: return WireFailure(Code.UNKNOWN, trim(body), null, httpStatus, rpc)
        return WireFailure(
            Code.fromWire(parsed.code),
            parsed.message,
            ErrorInfoCodec.fromDetails(parsed.details),
            httpStatus,
            rpc,
        )
    }

    /** Parses a Connect error body, or returns null when it is not one. */
    fun tryParseConnectErrorBody(body: String): ConnectErrorBody? {
        val root = Json.parse(body)?.obj ?: return null
        // `code` is the one member that makes this a Connect envelope. Its absence is
        // what distinguishes a proxy's HTML page from a refusal, and requiring it is
        // what keeps a JSON body that happens to have a `message` from being read as
        // one.
        val code = root.member("code") ?: return null
        return ConnectErrorBody(code, root.member("message") ?: "", readDetails(root["details"]))
    }

    /**
     * Parses the end-of-stream frame, whose `{"error": {…}}` nests the envelope one
     * level down from a unary error body.
     *
     * An empty payload, or one with no `error` member, is a **clean end**: the
     * protocol allows a server to close a stream with no error at all, and the
     * corpus's recorded streams are bounded prefixes a server closed that way. What
     * it does not allow is an error the SDK cannot read, so a payload that names a
     * code but whose details do not parse still yields a typed failure with that
     * code.
     */
    fun parseEndStreamFrame(payload: ByteArray): EndStreamResponse {
        if (payload.isEmpty()) {
            return EndStreamResponse(null)
        }
        val root = try {
            Json.parse(String(payload, Charsets.UTF_8))?.obj
        } catch (_: Exception) {
            null
        } ?: return EndStreamResponse(null)
        val error = root["error"]?.obj ?: return EndStreamResponse(null)
        val code = error.member("code") ?: return EndStreamResponse(null)
        return EndStreamResponse(
            ConnectErrorBody(code, error.member("message") ?: "", readDetails(error["details"]))
        )
    }

    /** The failure a Connect stream's end-of-stream frame describes, or null. */
    fun streamFailure(payload: ByteArray, httpStatus: Int, rpc: String): WireFailure? {
        val end = parseEndStreamFrame(payload)
        val error = end.error ?: return null
        return WireFailure(
            Code.fromWire(error.code),
            error.message,
            ErrorInfoCodec.fromDetails(error.details),
            httpStatus,
            rpc,
        )
    }

    /**
     * The failure a gRPC-Web trailers frame describes, or null when it says the call
     * succeeded.
     *
     * A trailers frame with no `grpc-status` is treated as success, which is what
     * gRPC-Web itself does: the absence of a status on a 200 is a status of zero. A
     * frame whose status is non-zero and whose details do not decode still produces a
     * typed failure with the code and the message — the class is the part a caller can
     * always act on, and losing it would turn a refusal into an unknown. That is also
     * why `grpc-status-details-bin` goes through [Base64.decode]: a value this decoder
     * refuses costs the reason, never the failure.
     */
    fun grpcWebFailure(trailers: Map<String, String>, httpStatus: Int, rpc: String): WireFailure? {
        val raw = trailers["grpc-status"] ?: return null
        val status = raw.toIntOrNull()
            ?: return WireFailure(Code.UNKNOWN, raw, null, httpStatus, rpc)
        if (status == 0) {
            return null
        }
        val message = trailers["grpc-message"]?.let { percentDecode(it) } ?: ""
        val encoded = trailers["grpc-status-details-bin"]
        val detail = if (!encoded.isNullOrEmpty()) {
            ErrorInfoCodec.fromStatusBytes(Base64.decode(encoded) ?: ByteArray(0))
        } else {
            null
        }
        return WireFailure(Code.fromNumber(status), message, detail, httpStatus, rpc)
    }

    /**
     * Reads a whole stream body: the data frames, plus the failure the body ended in
     * if it ended in one.
     *
     * @param codec how to decode the **data** frames. It is a parameter rather than
     *   derived from the body because a framed body says nothing about its own codec:
     *   `live_watch` is a `+proto` stream whose end frame is JSON, so "look at the
     *   content type" would answer about the end frame rather than the messages, and
     *   a global keyed on the RPC would be shared mutable state across every client
     *   in the process. The invoker passes the codec it is configured for, which is
     *   the same answer it used to encode the request.
     *
     * A frame this SDK cannot read — a compressed one, or a body that is not framed at
     * all — is **reported** rather than passed on as if it were data: a caller that
     * rendered an unreadable frame would show a row nobody sent.
     */
    fun readStream(
        body: ByteArray,
        httpStatus: Int,
        protocol: Protocol,
        rpc: String,
        codec: Codec = Codec.PROTO,
    ): StreamRead {
        val frames = Envelopes.split(body)
        val messages = mutableListOf<com.google.protobuf.DynamicMessage>()
        var failure: WireFailure? = null

        for (frame in frames) {
            when {
                frame.flags and EnvelopeFlags.TRAILERS != 0 ->
                    failure = grpcWebFailure(Envelopes.parseTrailers(frame.payload), httpStatus, rpc) ?: failure

                frame.flags and EnvelopeFlags.END_OF_STREAM != 0 ->
                    failure = streamFailure(frame.payload, httpStatus, rpc) ?: failure

                frame.flags and EnvelopeFlags.COMPRESSED != 0 ->
                    throw ErrorMapper.internal(
                        rpc,
                        "the server sent a compressed frame; this SDK sends no compression and cannot read one",
                    )

                else -> messages.add(MessageCodec.deserialize(bindingFor(rpc), frame.payload, codec))
            }
        }
        // `protocol` is part of the signature because the two protocols differ in where
        // the refusal arrives, and a caller reading this wants the answer to be the one
        // for the protocol it asked about. The frame flags already say which it was, so
        // it does not change the parse — it is taken and reported rather than ignored,
        // which would be a parameter that lies about being read.
        if (protocol == Protocol.GRPC_WEB && messages.isEmpty() && failure == null) {
            throw ErrorMapper.internal(
                rpc,
                "the gRPC-Web body carried neither a message frame nor a trailers frame, so the call's outcome " +
                    "is unknown and reporting success would be a claim nobody can check",
            )
        }
        return StreamRead(messages, failure)
    }

    /**
     * The response descriptor an RPC's frames decode into.
     *
     * Resolved per body rather than passed in, because the frame decoder is reached from
     * three places — the streaming reader, a unary gRPC-Web call, and the harness — and
     * threading a descriptor through all three would put a `Descriptor` in a signature
     * where a caller could pass the wrong one. A message decoded into the wrong type is
     * a plausible silent corruption; a message that cannot be decoded at all is not.
     */
    private fun bindingFor(rpc: String): com.google.protobuf.Descriptors.Descriptor =
        requireNotNull(Descriptors.bindingForRpc(rpc)) {
            "the committed descriptor set declares no RPC '$rpc', so its frames cannot be decoded"
        }.response

    /** Reads a `details` array out of a Connect envelope. */
    private fun readDetails(value: JsonValue?): List<ConnectDetail> {
        val items = value?.array ?: return emptyList()
        return items.mapNotNull { item ->
            val node = item.obj ?: return@mapNotNull null
            ConnectDetail(node.member("type") ?: "", node.member("value") ?: "")
        }
    }

    private fun trim(body: String): String {
        val text = body.trim()
        // 512 characters: enough for a JSON envelope to be recognisable in a log, short
        // enough that a 4 MB HTML page does not land in an exception message.
        return if (text.length <= 512) text else text.substring(0, 512) + "…"
    }

    /**
     * Percent-decodes a `grpc-message`.
     *
     * gRPC percent-encodes anything outside printable ASCII, and encodes a space as
     * `%20` rather than `+` — which is the difference between this and a form decoder,
     * and the reason a `+` in a message stays a `+`.
     */
    private fun percentDecode(value: String): String {
        if (!value.contains('%')) {
            return value
        }
        val bytes = ByteArrayOutput()
        var at = 0
        while (at < value.length) {
            val c = value[at]
            if (c == '%' && at + 2 < value.length) {
                val hex = value.substring(at + 1, at + 3)
                val decoded = hex.toIntOrNull(16)
                if (decoded != null) {
                    bytes.write(decoded)
                    at += 3
                    continue
                }
            }
            // Anything not percent-encoded arrives as UTF-8 bytes, so it is written as
            // its own encoding rather than truncated to a byte per char.
            for (byte in c.toString().toByteArray(Charsets.UTF_8)) {
                bytes.write(byte.toInt() and 0xFF)
            }
            at++
        }
        return String(bytes.toByteArray(), Charsets.UTF_8)
    }
}