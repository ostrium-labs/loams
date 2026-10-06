package dev.loams

import com.google.protobuf.Descriptors.Descriptor
import com.google.protobuf.DynamicMessage

/**
 * A per-call override. Everything is optional: the client's defaults apply.
 *
 * @property maxRetries the retries after the first attempt for this call. Zero disables them.
 * @property retrySafe overrides the call's retry class for this call only. False on a
 *   read stops the SDK retrying it; true on a mutation retries something the proto says
 *   is not safe to repeat, which is only correct when the call carries an idempotency key.
 * @property idempotencyKey the caller's own idempotency key, making the retry theirs
 *   rather than the SDK's. Omit it and the runtime mints a UUIDv7 per logical call and
 *   reuses it on every retry (R3).
 * @property mintIdempotencyKey whether the SDK may mint a key for this call at all.
 *   Null is the default, which is to mint (D610, R3).
 * @property headers headers for this call. `Authorization` is set by the runtime and wins.
 */
data class CallOptions(
    val maxRetries: Int? = null,
    val retrySafe: Boolean? = null,
    val idempotencyKey: String? = null,
    val mintIdempotencyKey: Boolean? = null,
    val headers: Map<String, String> = emptyMap(),
    /**
     * Whether the caller has already cancelled this call.
     *
     * A parameter rather than a `CancellationToken` because this SDK is **synchronous**
     * — see `DEPENDENCIES.md` for why — so there is nothing to interrupt a blocking
     * call with. It is here for the same reason the C# SDK has one: a caller that has
     * decided to give up says so, and the retry loop then spends no attempt on a call
     * whose answer nobody wants. A caller that wants a real interrupt cancels the
     * thread, which surfaces as a `LoamsException` with [Code.CANCELLED].
     */
    val cancelled: Boolean = false,
) {
    companion object {
        /** An empty set of overrides, which is what a caller who asked for nothing gets. */
        val NONE: CallOptions = CallOptions()
    }
}

/**
 * One attempt of a call, with everything the sender needs to make it.
 *
 * @property request the message, with the idempotency key already set. It is the **same
 *   value** on every attempt, which is the whole of R3: a key regenerated per attempt
 *   turns one write into two.
 * @property number the zero-based attempt number.
 * @property refreshed true once R1's single refresh has happened.
 */
data class Attempt(
    val request: DynamicMessage,
    val number: Int,
    val refreshed: Boolean,
)

/** Everything the retry loop needs, so the policy is testable without a transport. */
data class RetryPlan(
    /**
     * Whether the call may be retried on the SDK's own initiative: [RetryClass.SAFE]
     * for a read, or true once the call has been **keyed** — a mutation becomes
     * retryable when it carries an idempotency key, which is what makes the repeat
     * safe.
     */
    val retrySafe: Boolean,
    /** The retries after the first attempt. */
    val maxRetries: Int,
    /** Whether the client's token source can refresh at all. */
    val canRefresh: Boolean = false,
    /** The client's refresh, or null for a client with no source. */
    val refresh: (() -> Unit)? = null,
)

/**
 * The retry loop (R1, R2, R3).
 *
 * ## The same message on every attempt
 *
 * R3 is enforced structurally rather than by convention: the keyed message is computed
 * **before** the loop and the sender is handed the same [Attempt.request] value every
 * time. A sender that built its own request would be one refactor away from
 * regenerating the key per attempt, which is the exact failure the corpus's
 * `mock_state_idempotent_decide` records.
 */
object RetryLoop {
    /**
     * Runs [send] until it answers or the plan says stop.
     *
     * @param rpc the RPC, for the failures' messages.
     * @param request the request message. Passed in rather than taken from the sender,
     *   because R3 requires the **same** message on every attempt and a sender that
     *   built its own would be one refactor away from regenerating the key.
     * @param plan the retry class, the budget and the refresh.
     * @param send one attempt.
     * @param cancelled whether the caller has already given up.
     * @param sleep waits [RetryPolicy.backoff] milliseconds, overridable so a test can
     *   keep a retry test fast. It is a parameter rather than a hardcoded
     *   `Thread.sleep` precisely so that it is visible that something else does the
     *   waiting.
     * @throws LoamsException whatever the last attempt threw, mapped.
     */
    fun <T> run(
        rpc: String,
        request: DynamicMessage,
        plan: RetryPlan,
        cancelled: Boolean = false,
        sleep: (Long) -> Unit = { Thread.sleep(it) },
        send: (Attempt) -> T,
    ): T {
        var refreshed = false
        var attemptNumber = 0

        while (true) {
            // Checked **before** the attempt rather than after the failure: a caller who
            // cancelled between attempts must not spend one.
            if (cancelled) {
                throw LoamsException(
                    code = Code.CANCELLED,
                    rpc = rpc,
                    reason = Reason.NONE,
                    message = "$rpc: the caller cancelled the call before an attempt was spent",
                )
            }

            try {
                return send(Attempt(request, attemptNumber, refreshed))
            } catch (thrown: Throwable) {
                val mapped = ErrorMapper.fromException(thrown, rpc)

                // R1: a rejection whose reason is `token_expired` gets exactly one
                // refresh and one retry. A second expiry is reported rather than looped
                // on — a loop is the obvious implementation and it turns a refusal into
                // a hang.
                if (mapped is TokenExpiredException && !refreshed && plan.canRefresh && plan.refresh != null) {
                    try {
                        plan.refresh.invoke()
                    } catch (refreshFailure: Throwable) {
                        // A refresh that itself failed is the caller's answer: it is a
                        // real failure with its own cause, and reporting the original
                        // expiry would hide why nothing improved.
                        throw ErrorMapper.fromException(refreshFailure, rpc)
                    }
                    refreshed = true
                    // The refresh retry is not charged to the budget: the same logical
                    // call, so the attempt number does not advance.
                    continue
                }

                if (cancelled) {
                    throw gaveUp(mapped)
                }

                if (!RetryPolicy.shouldRetry(
                        attemptNumber, plan.maxRetries, plan.retrySafe, mapped.code, cancelled
                    )
                ) {
                    throw mapped
                }

                val wait = RetryPolicy.backoff(attemptNumber)
                sleep(wait)
                attemptNumber++
            }
        }
    }

    /**
     * Records that the caller's cancellation ended a call while a retryable failure was
     * still outstanding.
     *
     * It keeps the type, code and reason the server sent and says the caller is why it
     * stopped. The message is longer rather than the type different: Kotlin's `catch`
     * matches on the class, so a caller that caught [UnavailableException] would stop
     * catching it if a cancellation arrived as a different class. The original stays
     * reachable as [LoamsException.cause].
     */
    private fun gaveUp(mapped: LoamsException): LoamsException {
        val note = "${mapped.message} (the caller gave up before the retry)"
        return when (mapped) {
            is FeatureNotInVariantException ->
                FeatureNotInVariantException(mapped.rpc, mapped.metadata, mapped.hint, mapped.detail, mapped.httpStatus, note)
            is TokenExpiredException ->
                TokenExpiredException(mapped.rpc, mapped.metadata, mapped.hint, mapped.detail, mapped.httpStatus, note)
            is NotFoundException ->
                NotFoundException(mapped.rpc, mapped.reason, mapped.unknownReason, mapped.metadata, mapped.hint, mapped.detail, mapped.httpStatus, note)
            is InvalidArgumentException ->
                InvalidArgumentException(mapped.rpc, mapped.reason, mapped.unknownReason, mapped.metadata, mapped.hint, mapped.detail, mapped.httpStatus, note)
            is AlreadyExistsException ->
                AlreadyExistsException(mapped.rpc, mapped.reason, mapped.unknownReason, mapped.metadata, mapped.hint, mapped.detail, mapped.httpStatus, note)
            is PermissionDeniedException ->
                PermissionDeniedException(mapped.rpc, mapped.reason, mapped.unknownReason, mapped.metadata, mapped.hint, mapped.detail, mapped.httpStatus, note)
            is UnauthenticatedException ->
                UnauthenticatedException(mapped.rpc, mapped.reason, mapped.unknownReason, mapped.metadata, mapped.hint, mapped.detail, mapped.httpStatus, note)
            is FailedPreconditionException ->
                FailedPreconditionException(mapped.rpc, mapped.reason, mapped.unknownReason, mapped.metadata, mapped.hint, mapped.detail, mapped.httpStatus, note)
            is ResourceExhaustedException ->
                ResourceExhaustedException(mapped.rpc, mapped.reason, mapped.unknownReason, mapped.metadata, mapped.hint, mapped.detail, mapped.httpStatus, note)
            is UnavailableException ->
                UnavailableException(mapped.rpc, mapped.reason, mapped.unknownReason, mapped.metadata, mapped.hint, mapped.detail, mapped.httpStatus, note)
            is DeadlineExceededException ->
                DeadlineExceededException(mapped.rpc, mapped.reason, mapped.unknownReason, mapped.metadata, mapped.hint, mapped.detail, mapped.httpStatus, note)
            is AbortedException ->
                AbortedException(mapped.rpc, mapped.reason, mapped.unknownReason, mapped.metadata, mapped.hint, mapped.detail, mapped.httpStatus, note)
            is InternalException ->
                InternalException(mapped.rpc, mapped.reason, mapped.unknownReason, mapped.metadata, mapped.hint, mapped.detail, mapped.httpStatus, note, mapped.cause)
            is UnimplementedException ->
                UnimplementedException(mapped.rpc, mapped.reason, mapped.unknownReason, mapped.metadata, mapped.hint, mapped.detail, mapped.httpStatus, note)
            else ->
                LoamsException(
                    mapped.code, mapped.rpc, mapped.reason, mapped.unknownReason, mapped.metadata,
                    mapped.hint, mapped.detail, mapped.httpStatus, note, mapped.cause,
                )
        }
    }
}

/**
 * A request as it goes over the wire.
 *
 * @property rpc the full RPC path, `package.Service/Method`.
 * @property method the HTTP method. Always `POST`: Connect unary is a POST and the
 *   framing protocols are too, and a GET would need the idempotency level's blessing
 *   for a body.
 * @property path the request path, `/package.Service/Method`.
 * @property contentType the encoding this call is made in.
 * @property headers everything the runtime attached, plus the caller's own.
 * @property body the encoded request: the message itself, or one framed message.
 */
data class TransportRequest(
    val rpc: String,
    val method: String,
    val path: String,
    val contentType: String,
    val headers: Map<String, String>,
    val body: ByteArray,
) {
    override fun equals(other: Any?): Boolean = this === other || (
        other is TransportRequest && rpc == other.rpc && method == other.method && path == other.path &&
            contentType == other.contentType && headers == other.headers && body.contentEquals(other.body)
        )

    override fun hashCode(): Int {
        var result = rpc.hashCode()
        result = 31 * result + method.hashCode()
        result = 31 * result + path.hashCode()
        result = 31 * result + contentType.hashCode()
        result = 31 * result + headers.hashCode()
        result = 31 * result + body.contentHashCode()
        return result
    }
}

/**
 * What came back over HTTP.
 *
 * @property status the HTTP status. **Not** proof of success: R10's sharpest edge is
 *   that a refusal is often not an HTTP status, so every caller parses [body] before
 *   deciding.
 * @property headers the response headers, last value winning for a repeated name.
 * @property contentType the encoding the server answered in.
 * @property body the raw response body.
 */
data class TransportResponse(
    val status: Int,
    val headers: Map<String, String>,
    val contentType: String,
    val body: ByteArray,
) {
    override fun equals(other: Any?): Boolean = this === other || (
        other is TransportResponse && status == other.status && headers == other.headers &&
            contentType == other.contentType && body.contentEquals(other.body)
        )

    override fun hashCode(): Int {
        var result = status
        result = 31 * result + headers.hashCode()
        result = 31 * result + contentType.hashCode()
        result = 31 * result + body.contentHashCode()
        return result
    }
}

/**
 * The HTTP layer the runtime sends over.
 *
 * An interface because `java.net.http.HttpClient` is a reasonable default and an
 * **unreasonable requirement**: an Android app already has OkHttp and a coroutine
 * dispatcher, and a Kotlin SDK that forces `java.net.http` onto it would either fail on
 * API levels that lack it or double its own connection pool. So the shipped
 * implementation is one class, and substituting it is an interface implementation
 * rather than a fork. See `DEPENDENCIES.md`.
 */
interface Transport {
    /** Sends one request and returns what came back. Blocking, by design. */
    fun send(request: TransportRequest): TransportResponse
}

/**
 * A per-call consistency token store (D609, R4).
 *
 * ## Why it is inert today
 *
 * `manifest.json` says so about R4 in as many words: **no RPC carries a
 * `consistency_token`**, and the token's encoding is not in the protos yet. R4 is pinned
 * against a stub, deliberately — "a silently-merged token reads stale data, which is
 * worse than a failure".
 *
 * So this class does the one thing that is safe with no token in sight and refuses the
 * rest: it holds nothing, merges nothing, and [merge] returns null. It exists because
 * the *seam* has to be there before the field does — a session store introduced at the
 * same moment as the first token is a store whose merge rules have never been reviewed
 * against a real token.
 */
class ConsistencySession {
    /** The header a returned token is carried in. */
    val header: String get() = "loams-consistency-token"

    /** The token to attach to a later read, or null when there is none. */
    fun token(): String? = null

    /**
     * Folds a returned token in, or returns null when there was nothing to merge.
     *
     * Never throws: no RPC returns one yet, and a store that refused a token no RPC
     * could send would be a failure the SDK could raise on its own.
     */
    fun merge(token: String?): String? = null
}