package dev.loams

import com.google.protobuf.Descriptors.FieldDescriptor
import com.google.protobuf.DynamicMessage

/**
 * The invoker: the call path a facade method delegates to (design §44 §7.4; runtime
 * contract R1–R4).
 *
 * It is the one place a binding and a request message become an RPC, and it does four
 * things the derived facade cannot:
 *
 *  - attaches the bearer from the client's token source, **per attempt**, because R1's
 *    refresh has to change it between two attempts of one logical call;
 *  - hands the retry class to [RetryLoop], which applies it with M1.6 Ruling 5's backoff
 *    numbers and refreshes the token once on `token_expired`;
 *  - gives a mutating call an idempotency key **once per logical call** and reuses it on
 *    every retry, so a retried write is the same write (R3);
 *  - turns whatever comes back into the typed [LoamsException] hierarchy, so a caller
 *    branches on a `reason` and never on a message.
 *
 * # The same message on every attempt
 *
 * R3 is enforced structurally rather than by convention: the keyed message is computed
 * **before** the loop and the sender is handed the same [Attempt.request] value every
 * time. A sender that built its own request would be one refactor away from regenerating
 * the key per attempt, which is the exact failure the corpus's
 * `mock_state_idempotent_decide` records.
 */
class CallInvoker(
    /** The transport calls go over. */
    val transport: Transport,
    /** The client's credentials, or null for a client with none — which `GetInstance` does not need. */
    private val tokenSource: TokenSource?,
    /** The client's default retry budget; a call overrides it. */
    val maxRetries: Int,
    /** The protocol this client speaks. */
    val protocol: Protocol,
    /** The codec this client sends. */
    val codec: Codec,
    /** The consistency-token session, or null. Inert today; see [ConsistencySession]. */
    private val session: ConsistencySession? = null,
) {
    /** This client's protocol, for a caller that wants to branch on it. */
    val wireProtocol: Protocol get() = protocol

    /**
     * Makes one unary call with the whole runtime contract applied.
     *
     * @param binding the RPC, derived from the committed descriptor set.
     * @param request the request message. Never mutated — see [Idempotency.apply].
     * @param options per-call overrides, or null for the client's defaults.
     * @throws ErrorMapper.internal the binding is a server stream.
     * @throws LoamsException whatever the call failed with, mapped.
     */
    fun unary(
        binding: CallBinding,
        request: DynamicMessage,
        options: CallOptions? = null,
    ): DynamicMessage {
        if (binding.streaming != Streaming.UNARY) {
            throw ErrorMapper.internal(
                binding.rpc,
                "${binding.method} is a server stream; call serverStream for it",
            )
        }

        val settings = options ?: CallOptions.NONE

        // R3: keyed **once**, before the first attempt, and the same message goes to
        // every attempt. A binding whose schema declares no `idempotency_key` is left
        // exactly as the caller wrote it, and a caller who set `mintIdempotencyKey =
        // false` has already decided this call goes out unkeyed (D655).
        val keyed = Idempotency.apply(request, settings.idempotencyKey ?: "", settings.mintIdempotencyKey ?: true)
        val plan = planFor(binding, settings, keyed.keyed)

        val response = RetryLoop.run(
            rpc = binding.rpc,
            request = keyed.request,
            plan = plan,
            cancelled = settings.cancelled,
        ) { attempt ->
            sendUnary(binding, attempt, settings)
        }

        recordConsistency(settings, response)
        return response
    }

    /**
     * One server stream, and the counts a caller needs about it.
     *
     * @param resume how the stream reconnects, or null for one that does not.
     *
     * The handle is returned **eagerly** and the frames are read as the caller iterates,
     * so a stream that fails on its first frame reports the failure from inside the
     * `for` loop rather than from the call that opened it — which is where a caller has
     * a `try` around it.
     */
    fun serverStream(
        binding: CallBinding,
        request: DynamicMessage,
        options: CallOptions? = null,
        resume: StreamResume? = null,
    ): ServerStreamHandle {
        if (binding.streaming != Streaming.SERVER) {
            throw ErrorMapper.internal(binding.rpc, "${binding.method} is unary; call unary for it")
        }
        val settings = options ?: CallOptions.NONE
        val reader = ServerStreamReader(this, binding, request, settings, resume)
        return reader.open()
    }

    /** Sends one attempt of a unary call and decodes the answer. */
    private fun sendUnary(binding: CallBinding, attempt: Attempt, settings: CallOptions): DynamicMessage {
        val body = encodeRequest(binding, attempt.request, false)
        val request = TransportRequest(
            rpc = binding.rpc,
            method = "POST",
            path = "/" + binding.rpc,
            contentType = ContentTypes.forCall(protocol, codec, Streaming.UNARY),
            headers = headers(settings),
            body = body,
        )

        val response = transport.send(request)
        if (readUnaryFailure(response, binding.rpc) != null) {
            throw ErrorMapper.map(readUnaryFailure(response, binding.rpc)!!, binding.rpc)
        }
        return MessageCodec.deserialize(binding.response, response.body, codec)
    }

    /**
     * Encodes a request, framed when the encoding frames.
     *
     * Framing is the asymmetry the corpus records byte for byte: `live_watch`'s request
     * is `00 00 00 00 00` — a 5-byte envelope around an empty message — while
     * `instance_get_instance_json`'s is `{}` with no envelope at all. Getting this
     * backwards fails the comparison against the recording for a reason that has
     * nothing to do with conformance, so it is asked of the **content type** rather than
     * of the streaming flag.
     */
    internal fun encodeRequest(binding: CallBinding, request: DynamicMessage, streaming: Boolean): ByteArray {
        val contentType = ContentTypes.forCall(protocol, codec, if (streaming) Streaming.SERVER else Streaming.UNARY)
        val payload = MessageCodec.serialize(request, codec)
        return if (ContentTypes.isFramed(contentType)) Envelopes.wrap(payload) else payload
    }

    /**
     * The headers one attempt goes out with, including the bearer fetched **per
     * attempt**.
     *
     * Per attempt rather than per call, and that is the whole of R1's mechanism: the
     * refresh changes the token, and the retry has to carry the new one. A bearer
     * computed once per call would be the stale one on every retry after a refresh, and
     * the failure would look like "the refresh did not work" rather than "the refresh was
     * never used".
     *
     * The caller's own `Authorization` is **dropped**, not merged: a caller-supplied
     * bearer is one the runtime cannot refresh, and honouring it would make R1
     * unreachable for the call that set it. Matched case-insensitively, because HTTP
     * header names are.
     */
    internal fun headers(settings: CallOptions): Map<String, String> {
        val headers = LinkedHashMap<String, String>()
        for ((name, value) in settings.headers) {
            if (name.equals("Authorization", ignoreCase = true)) {
                continue
            }
            headers[name] = value
        }
        if (tokenSource != null) {
            val token = tokenSource.token()
            if (token.isNotEmpty()) {
                headers["Authorization"] = token
            }
        }
        return headers
    }

    /** The retry plan for one call. */
    private fun planFor(binding: CallBinding, settings: CallOptions, keyed: Boolean): RetryPlan {
        val retries = settings.maxRetries ?: maxRetries
        val budget = if (retries < 0) 0 else retries

        // D610: a `safe` call always retries; a mutation retries once it carries an
        // idempotency key, which `Idempotency.apply` has just decided.
        var retrySafe = binding.retry == RetryClass.SAFE || keyed
        if (settings.retrySafe != null) {
            retrySafe = settings.retrySafe
        }

        // The refresh is taken as a reference rather than as the source, so the plan
        // holds a value that does not capture a nullable source and has to be
        // re-checked at every use.
        return if (tokenSource == null) {
            RetryPlan(retrySafe, budget)
        } else {
            RetryPlan(retrySafe, budget, tokenSource.canRefresh) { tokenSource.refresh() }
        }
    }

    /**
     * Folds a response's consistency token into the session, when the caller opted in.
     *
     * A merge failure is swallowed **on purpose**: no RPC carries a `consistency_token`
     * yet (R4), and when one does the store may legitimately be asked to merge two it
     * cannot. The call succeeded; refusing it here would make a caller that retries on
     * the error perform the write twice.
     */
    private fun recordConsistency(settings: CallOptions, response: DynamicMessage) {
        if (session == null) {
            return
        }
        val field = response.descriptorForType.findFieldByName("consistency_token") ?: return
        val token = response.getField(field)?.toString() ?: return
        if (token.isEmpty()) {
            return
        }
        session.merge(token)
    }

    internal companion object {
        /**
         * Whether a unary response is a refusal, and which one.
         *
         * A Connect unary refusal is an HTTP 4xx or 5xx with a JSON error body; a gRPC-Web
         * unary refusal is an HTTP 200 whose **body** is a trailers frame. Both are read
         * here, and neither is decided by the status alone — `isRetryableCode` must never
         * treat a bare 200 as proof of anything.
         */
        fun readUnaryFailure(response: TransportResponse, rpc: String): WireFailure? {
            val contentType = response.contentType
            val grpcWeb = ContentTypes.isGrpcWeb(contentType)

            if (grpcWeb) {
                // The body is framed even for a unary call: a message frame and then a
                // trailers frame.
                val frames = try {
                    Envelopes.split(response.body)
                } catch (notFramed: IllegalArgumentException) {
                    // A body that is not framed at all is a server that answered with
                    // something else entirely — a proxy, or an error page. The status
                    // still carries a class, so a 4xx is reported as itself rather than
                    // as a framing failure; a 2xx with an unframed body is reported as
                    // `unknown`, because nothing in it says what happened.
                    return if (response.status >= 400) {
                        WireFailure(
                            Code.fromHttpStatus(response.status),
                            response.body.toString(Charsets.UTF_8).trim().take(512),
                            null,
                            response.status,
                            rpc,
                        )
                    } else {
                        WireFailure(
                            Code.UNKNOWN,
                            "the gRPC-Web answer was ${response.body.size} bytes that are not a framed body",
                            null,
                            response.status,
                            rpc,
                        )
                    }
                }
                for (frame in frames) {
                    if (frame.flags and EnvelopeFlags.TRAILERS != 0) {
                        return WireReader.grpcWebFailure(Envelopes.parseTrailers(frame.payload), response.status, rpc)
                    }
                }
                return null
            }

            // Connect unary: an HTTP 4xx or 5xx is a refusal, and the body carries the
            // reason. A 2xx is an answer, whatever else it might be.
            if (response.status >= 400) {
                return WireReader.unaryFailure(response.status, response.body.toString(Charsets.UTF_8), rpc)
            }
            return null
        }
    }
}

/**
 * Reads one server stream's frames and hands the caller a sequence of messages,
 * reconnecting from the cursor when the caller asked for it (R7).
 *
 * Not a public type: a caller sees a [ServerStreamHandle], which is an ordinary
 * `Sequence`. Everything stateful — the cursor, the reconnect count, the
 * "yield without resuming" rule — is here so the facade method that returns the handle
 * stays three lines long.
 */
internal class ServerStreamReader(
    private val invoker: CallInvoker,
    private val binding: CallBinding,
    private val request: DynamicMessage,
    private val settings: CallOptions,
    private val resume: StreamResume?,
) {
    /** How many times this stream re-opened. Useful in a test and in a log line. */
    var reconnects: Int = 0
        private set

    /**
     * How many heartbeats the server sent. Counted rather than yielded: a heartbeat is
     * liveness, and a stream that yields heartbeats to a UI is a stream that renders
     * empty rows every fifteen seconds.
     */
    var heartbeats: Int = 0
        private set

    /**
     * What arrived on the wire, in order, one entry per frame.
     *
     * The sequence itself hides two things a caller has to be able to see: the
     * heartbeats, which are liveness rather than data, and the order the frames arrived
     * in. This is where both are visible, named in the server's own vocabulary — the
     * corpus's `mock_state_stream_heartbeat` records `frameKinds: ["snapshot",
     * "heartbeat"]`, and this is what produces that list.
     */
    val frameKinds: MutableList<String> = mutableListOf()

    /** Opens the stream and returns the handle. The frames are read as the caller iterates. */
    fun open(): ServerStreamHandle {
        // A lazy sequence rather than an eager read, so a stream that fails on its first
        // frame reports the failure from inside the caller's `for` loop — which is where
        // a caller has a `try` around it — rather than from the call that opened it.
        val messages = lazyMessages()
        return ServerStreamHandle(messages, { heartbeats }, { reconnects }, frameKinds)
    }

    private fun lazyMessages(): Sequence<DynamicMessage> = sequence {
        var cursor = StreamCursor.NONE
        var yielded = false

        // A policy that does not state a budget gets the client's, and a negative one is
        // the "unset" marker rather than "never reconnect".
        val budget = when {
            resume == null -> invoker.maxRetries
            resume.maxRetries >= 0 -> resume.maxRetries
            else -> invoker.maxRetries
        }

        while (true) {
            // R7's two halves interact: nothing yielded yet means resume is safe
            // (re-opening from scratch would duplicate nothing, because nothing was
            // seen), and something yielded means the re-open **must** carry a cursor.
            val openRequest = if (resume == null || !yielded) request else resume.resume(cursor, request)

            val read = try {
                open(openRequest)
            } catch (thrown: Throwable) {
                val mapped = ErrorMapper.fromException(thrown, binding.rpc)
                if (shouldReconnect(budget, mapped.code)) {
                    reconnects++
                    continue
                }
                throw mapped
            }

            for (message in read.messages) {
                if (isHeartbeat(message)) {
                    heartbeats++
                    frameKinds.add("heartbeat")
                    continue
                }
                frameKinds.add(kindOf(message))

                // The cursor is recorded as each message is yielded, so a reconnect
                // resumes from the last one the caller *has*, not the last one the server
                // sent: a frame that arrived and was not yielded is a frame the caller
                // never applied.
                if (resume != null) {
                    val next = resume.cursorOf(message)
                    if (!next.isEmpty) {
                        cursor = next
                    }
                }

                yielded = true
                yield(message)
            }

            val failure = read.failure
            if (failure == null) {
                return@sequence
            }

            val mappedFailure = ErrorMapper.map(failure, binding.rpc)
            if (shouldReconnect(budget, mappedFailure.code)) {
                reconnects++
                continue
            }
            throw mappedFailure
        }
    }

    /**
     * Whether a stream failure is one a reconnect may absorb.
     *
     * Four conditions, and each is load-bearing:
     *
     *  - a policy exists — without one there is nowhere to put the cursor, and a
     *    re-open would duplicate everything;
     *  - something was yielded — nothing yielded means re-opening from scratch loses
     *    nothing, which is the safe case and is why the rule is "yielded" rather than
     *    "attempted";
     *  - the code is one the retry class covers, so an `unimplemented` stream is
     *    **reported** rather than spun on;
     *  - the caller has not cancelled.
     */
    private fun shouldReconnect(budget: Int, code: Code): Boolean =
        resume != null && reconnects < budget && RetryPolicy.isRetryableCode(code) && !settings.cancelled

    /** Opens the stream once and reads its whole body. */
    private fun open(openRequest: DynamicMessage): StreamRead {
        val body = invoker.encodeRequest(binding, openRequest, streaming = true)
        val contentType = ContentTypes.forCall(invoker.protocol, invoker.codec, Streaming.SERVER)
        val response = invoker.transport.send(
            TransportRequest(
                rpc = binding.rpc,
                method = "POST",
                path = "/" + binding.rpc,
                contentType = contentType,
                headers = invoker.headers(settings),
                body = body,
            )
        )

        if (invoker.protocol == Protocol.GRPC_WEB) {
            // gRPC-Web unary and streaming share the framing, so the whole body is the
            // frames and the refusal is the trailers frame among them.
            return WireReader.readStream(response.body, response.status, Protocol.GRPC_WEB, binding.rpc, invoker.codec)
        }

        // Connect streaming: an HTTP error status carries a Connect error envelope in the
        // body rather than frames, and it is read as one.
        if (response.status >= 400) {
            val failure = WireReader.unaryFailure(response.status, response.body.toString(Charsets.UTF_8), binding.rpc)
            return StreamRead(emptyList(), failure)
        }
        return WireReader.readStream(response.body, response.status, Protocol.CONNECT, binding.rpc, invoker.codec)
    }

    /**
     * Whether a frame is a heartbeat, read from the message rather than from its size.
     *
     * A heartbeat is the `heartbeat` case of the response's oneof, so a stream that
     * declares one has exactly one way to say "nothing changed, I am still here" — and a
     * message that carries a `heartbeat` field and nothing else is the liveness signal,
     * not a row.
     *
     * The check is over the oneof cases rather than over the whole message, because a
     * response that also carries a cursor (which `WatchApprovalsResponse` does, and the
     * corpus's heartbeat frame does too) is still a heartbeat: the cursor is the
     * position, not the data. Anything whose set case is **not** `heartbeat` is yielded,
     * so `snapshot`, `upsert` and `remove` all reach the caller — and R7's `remove` case
     * is the one an SDK that only handles upserts gets wrong, leaving a decided approval
     * on screen forever.
     */
    private fun isHeartbeat(message: DynamicMessage): Boolean {
        for (oneof in message.descriptorForType.oneofs) {
            for (field in oneof.fields) {
                if (field.name == "heartbeat" && message.hasField(field)) {
                    return true
                }
            }
        }
        return false
    }

    /**
     * The oneof case a message set, in the server's own vocabulary, or the message's own
     * name when it has no oneof.
     */
    private fun kindOf(message: DynamicMessage): String {
        for (oneof in message.descriptorForType.oneofs) {
            for (field in oneof.fields) {
                if (message.hasField(field)) {
                    return field.jsonName
                }
            }
        }
        return message.descriptorForType.name
    }
}

/** One module's calls, for a caller iterating the derived facade. */
val ModuleBinding.callNames: List<String> get() = calls.map { it.facadeName }

/** The items field and the token field of a paged call, or null when it does not page. */
val CallBinding.paginationPair: Pair<FieldDescriptor, FieldDescriptor>? get() = paginationFields()