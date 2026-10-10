package dev.loams

/**
 * The typed error hierarchy of design §44 §7.4, decision D611 (runtime contract
 * R8).
 *
 * A failed RPC carries a Connect code and one `loams.errors.v1.ErrorInfo` in its
 * details. The **code** gives the class — a taxonomy that does not change within an
 * API major version — and the **`reason`** is the stable branch. The message is for
 * a person and may change; nothing in this SDK branches on it.
 *
 * # Kotlin's branch is a type test
 *
 * Go reaches its hierarchy with `errors.As`, C# with a `when` filter, Java with
 * `instanceof`. Kotlin has the last of those:
 *
 * ```kotlin
 * try { client.instance().getInstance() }
 * catch (error: NotFoundException) { … }
 * catch (error: LoamsException) { … }   // catches every typed refusal
 * ```
 *
 * [LoamsException] is the base of the hierarchy **and** the thing every failure is,
 * so a `catch (LoamsException)` with no filter catches all of them — including a
 * failure from below the API.
 *
 * Three cases R8 requires to stay distinct, and which this hierarchy keeps distinct:
 *
 *  - a reason from a **newer** server, which this SDK's registry does not have: it is
 *    surfaced as text in [LoamsException.unknownReason] and flagged, not dropped;
 *  - a failure from **below the API** — a socket, a refused connection, a cancelled
 *    call — which carries no reason at all ([Reason.NONE]) and names no server code;
 *  - a [LoamsException] that has already been mapped, which is returned unchanged if
 *    mapped twice, so wrapping an SDK's own error never loses its reason.
 *
 * ## Why these are `Exception` and not a sealed hierarchy of data
 *
 * A sealed hierarchy cannot extend `Exception` and still be thrown across a
 * coroutine boundary with its type intact, and a caller on Android catches
 * `Exception` because that is what the platform throws. So the hierarchy is a
 * normal open class, and the branch a caller wants — the reason — is on the base.
 */
open class LoamsException(
    /** The Connect code, the canonical classification. */
    val code: Code,
    /** The RPC that failed, as `package.Service/Method`. */
    val rpc: String,
    /** The stable cause, or [Reason.NONE] for a failure from below the API. */
    val reason: Reason,
    /**
     * A reason off the wire that this SDK's registry does not have, meaning the
     * server is newer than the SDK (R8).
     *
     * Surfaced rather than dropped: losing it would leave a caller unable to tell
     * "not supported here" from "not supported at all". It is a `String` rather than a
     * [Reason] because no enum value can name it.
     */
    val unknownReason: String? = null,
    /** The structured context the server sent. Never secrets. */
    val metadata: Map<String, String> = emptyMap(),
    /** A short next step in the caller's locale, when the server sent one. */
    val hint: String = "",
    /** The raw `ErrorInfo` detail, when there was one. */
    val detail: ErrorInfoShape? = null,
    /** The HTTP status the failure arrived with, or null when it never spoke. */
    val httpStatus: Int? = null,
    /** What the failure says. Defaults to a rendering of the code and the reason. */
    message: String? = null,
    cause: Throwable? = null,
) : Exception(message ?: defaultMessage(code, rpc, reason, unknownReason), cause)

/**
 * A malformed field or an unparseable value: `invalid_argument`.
 *
 * The class carries no field of its own; the branch is what a caller wants and the
 * reason is on the base.
 */
class InvalidArgumentException(
    rpc: String,
    reason: Reason = Reason.INVALID_ARGUMENT,
    unknownReason: String? = null,
    metadata: Map<String, String> = emptyMap(),
    hint: String = "",
    detail: ErrorInfoShape? = null,
    httpStatus: Int? = null,
    message: String? = null,
) : LoamsException(Code.INVALID_ARGUMENT, rpc, reason, unknownReason, metadata, hint, detail, httpStatus, message)

/** The named resource does not exist: `not_found`. */
class NotFoundException(
    rpc: String,
    reason: Reason = Reason.NOT_FOUND,
    unknownReason: String? = null,
    metadata: Map<String, String> = emptyMap(),
    hint: String = "",
    detail: ErrorInfoShape? = null,
    httpStatus: Int? = null,
    message: String? = null,
) : LoamsException(Code.NOT_FOUND, rpc, reason, unknownReason, metadata, hint, detail, httpStatus, message)

/** The named resource already exists: `already_exists`. */
class AlreadyExistsException(
    rpc: String,
    reason: Reason = Reason.ALREADY_EXISTS,
    unknownReason: String? = null,
    metadata: Map<String, String> = emptyMap(),
    hint: String = "",
    detail: ErrorInfoShape? = null,
    httpStatus: Int? = null,
    message: String? = null,
) : LoamsException(Code.ALREADY_EXISTS, rpc, reason, unknownReason, metadata, hint, detail, httpStatus, message)

/** The caller's role may not make this call: `permission_denied`. */
class PermissionDeniedException(
    rpc: String,
    reason: Reason = Reason.PERMISSION_DENIED,
    unknownReason: String? = null,
    metadata: Map<String, String> = emptyMap(),
    hint: String = "",
    detail: ErrorInfoShape? = null,
    httpStatus: Int? = null,
    message: String? = null,
) : LoamsException(Code.PERMISSION_DENIED, rpc, reason, unknownReason, metadata, hint, detail, httpStatus, message)

/**
 * No credential, or one that cannot be used: `unauthenticated`.
 *
 * This is the class R1's refresh applies under; [TokenExpiredException] is a
 * narrower failure inside it and is distinguished by
 * [LoamsException.reason], not by the class alone.
 */
open class UnauthenticatedException(
    rpc: String,
    reason: Reason = Reason.UNAUTHENTICATED,
    unknownReason: String? = null,
    metadata: Map<String, String> = emptyMap(),
    hint: String = "",
    detail: ErrorInfoShape? = null,
    httpStatus: Int? = null,
    message: String? = null,
) : LoamsException(Code.UNAUTHENTICATED, rpc, reason, unknownReason, metadata, hint, detail, httpStatus, message)

/** The call's preconditions do not hold: `failed_precondition`. */
class FailedPreconditionException(
    rpc: String,
    reason: Reason = Reason.FAILED_PRECONDITION,
    unknownReason: String? = null,
    metadata: Map<String, String> = emptyMap(),
    hint: String = "",
    detail: ErrorInfoShape? = null,
    httpStatus: Int? = null,
    message: String? = null,
) : LoamsException(Code.FAILED_PRECONDITION, rpc, reason, unknownReason, metadata, hint, detail, httpStatus, message)

/** A backpressure or quota refusal: `resource_exhausted`. */
class ResourceExhaustedException(
    rpc: String,
    reason: Reason = Reason.RESOURCE_EXHAUSTED,
    unknownReason: String? = null,
    metadata: Map<String, String> = emptyMap(),
    hint: String = "",
    detail: ErrorInfoShape? = null,
    httpStatus: Int? = null,
    message: String? = null,
) : LoamsException(Code.RESOURCE_EXHAUSTED, rpc, reason, unknownReason, metadata, hint, detail, httpStatus, message)

/** A dependency is down, or this node cannot serve the read: `unavailable`. */
class UnavailableException(
    rpc: String,
    reason: Reason = Reason.UNAVAILABLE,
    unknownReason: String? = null,
    metadata: Map<String, String> = emptyMap(),
    hint: String = "",
    detail: ErrorInfoShape? = null,
    httpStatus: Int? = null,
    message: String? = null,
) : LoamsException(Code.UNAVAILABLE, rpc, reason, unknownReason, metadata, hint, detail, httpStatus, message)

/** The caller's deadline passed: `deadline_exceeded`. */
class DeadlineExceededException(
    rpc: String,
    reason: Reason = Reason.DEADLINE_EXCEEDED,
    unknownReason: String? = null,
    metadata: Map<String, String> = emptyMap(),
    hint: String = "",
    detail: ErrorInfoShape? = null,
    httpStatus: Int? = null,
    message: String? = null,
) : LoamsException(Code.DEADLINE_EXCEEDED, rpc, reason, unknownReason, metadata, hint, detail, httpStatus, message)

/** A concurrent write won; the caller retries: `aborted`. */
class AbortedException(
    rpc: String,
    reason: Reason = Reason.ABORTED,
    unknownReason: String? = null,
    metadata: Map<String, String> = emptyMap(),
    hint: String = "",
    detail: ErrorInfoShape? = null,
    httpStatus: Int? = null,
    message: String? = null,
) : LoamsException(Code.ABORTED, rpc, reason, unknownReason, metadata, hint, detail, httpStatus, message)

/** A bug. The message and the request id go to the log: `internal`. */
class InternalException(
    rpc: String,
    reason: Reason = Reason.INTERNAL,
    unknownReason: String? = null,
    metadata: Map<String, String> = emptyMap(),
    hint: String = "",
    detail: ErrorInfoShape? = null,
    httpStatus: Int? = null,
    message: String? = null,
    cause: Throwable? = null,
) : LoamsException(Code.INTERNAL, rpc, reason, unknownReason, metadata, hint, detail, httpStatus, message, cause)

/** The handler does not exist in this build: `unimplemented`. */
open class UnimplementedException(
    rpc: String,
    reason: Reason = Reason.NOT_IMPLEMENTED,
    unknownReason: String? = null,
    metadata: Map<String, String> = emptyMap(),
    hint: String = "",
    detail: ErrorInfoShape? = null,
    httpStatus: Int? = null,
    message: String? = null,
) : LoamsException(Code.UNIMPLEMENTED, rpc, reason, unknownReason, metadata, hint, detail, httpStatus, message)

/**
 * A package this build variant does not carry (design §44 §4, D600).
 *
 * The server answers `unimplemented` with `reason = feature_not_in_variant` and
 * names the variant in `metadata.variant`, which is what [variant] reads. A caller
 * usually never gets here: `client.system.guard()` feature-detects from
 * `GetInstance.services[]` before calling, so an unavailable module raises this same
 * type from the guard with no request spent. One `catch` therefore covers "the guard
 * said no" and "the server refused", which is the point of the guard raising the
 * same type.
 */
class FeatureNotInVariantException(
    rpc: String,
    metadata: Map<String, String> = emptyMap(),
    hint: String = "",
    detail: ErrorInfoShape? = null,
    httpStatus: Int? = null,
    message: String? = null,
) : UnimplementedException(rpc, Reason.FEATURE_NOT_IN_VARIANT, null, metadata, hint, detail, httpStatus, message) {
    /**
     * The build variant that was asked for, from `metadata.variant`.
     *
     * The guard raises this type from the catalogue, which does not carry a variant
     * name, so it is the empty string there rather than a guess.
     */
    val variant: String = metadata["variant"] ?: ""
}

/**
 * A token the server rejected as expired: `unauthenticated` with reason
 * `token_expired`.
 *
 * The runtime refreshes once and retries once (D608, R1); a second expiry reaches
 * the caller as this type.
 */
class TokenExpiredException(
    rpc: String,
    metadata: Map<String, String> = emptyMap(),
    hint: String = "",
    detail: ErrorInfoShape? = null,
    httpStatus: Int? = null,
    message: String? = null,
) : UnauthenticatedException(rpc, Reason.TOKEN_EXPIRED, null, metadata, hint, detail, httpStatus, message)

/**
 * What the error mapping needs from a failure that arrived over the wire: the code,
 * the message and the `ErrorInfo` detail, whatever protocol carried it.
 *
 * The transport fills this in; the mapping reads it.
 */
data class WireFailure(
    val code: Code,
    val message: String,
    val detail: ErrorInfoShape? = null,
    val httpStatus: Int? = null,
    val rpc: String? = null,
)

/**
 * Turns anything that came back — or threw — into the typed hierarchy (D611).
 */
object ErrorMapper {
    /**
     * Maps a wire failure into the class its code names, with `reason` and
     * `metadata` lifted out of the `ErrorInfo` detail.
     *
     * The detail is looked up **by its type**, never by position, so a service that
     * adds a detail of its own cannot move `reason` out from under a caller. A
     * detail whose bytes did not parse is reported as "no `ErrorInfo`" rather than
     * dropped silently, because the alternative is a Loams failure with no reason
     * and no hint, which is the one thing R8 says must not happen.
     */
    fun map(failure: WireFailure, rpc: String): LoamsException {
        val detail = failure.detail
        val metadata = detail?.metadata ?: emptyMap()
        val hint = detail?.hint ?: ""

        val known = if (detail != null && detail.reason.isNotEmpty()) ReasonRegistry.fromWire(detail.reason) else null
        val reason = known ?: Reason.NONE
        val unknown = if (known == null && detail != null && detail.reason.isNotEmpty()) detail.reason else null

        // The two reasons D611 gives their own class are decided **before** the
        // code-to-class table runs, because both are refinements of a code the table
        // would otherwise answer on its own.
        if (reason == Reason.FEATURE_NOT_IN_VARIANT) {
            return FeatureNotInVariantException(rpc, metadata, hint, detail, failure.httpStatus, failure.message)
        }
        if (failure.code == Code.UNAUTHENTICATED && reason == Reason.TOKEN_EXPIRED) {
            return TokenExpiredException(rpc, metadata, hint, detail, failure.httpStatus, failure.message)
        }

        return byCode(failure.code, rpc, reason, unknown, metadata, hint, detail, failure.httpStatus, failure.message)
    }

    private fun byCode(
        code: Code,
        rpc: String,
        reason: Reason,
        unknown: String?,
        metadata: Map<String, String>,
        hint: String,
        detail: ErrorInfoShape?,
        httpStatus: Int?,
        message: String,
    ): LoamsException = when (code) {
        Code.INVALID_ARGUMENT -> InvalidArgumentException(rpc, reason, unknown, metadata, hint, detail, httpStatus, message)
        Code.NOT_FOUND -> NotFoundException(rpc, reason, unknown, metadata, hint, detail, httpStatus, message)
        Code.ALREADY_EXISTS -> AlreadyExistsException(rpc, reason, unknown, metadata, hint, detail, httpStatus, message)
        Code.PERMISSION_DENIED -> PermissionDeniedException(rpc, reason, unknown, metadata, hint, detail, httpStatus, message)
        Code.UNAUTHENTICATED -> UnauthenticatedException(rpc, reason, unknown, metadata, hint, detail, httpStatus, message)
        Code.FAILED_PRECONDITION -> FailedPreconditionException(rpc, reason, unknown, metadata, hint, detail, httpStatus, message)
        Code.RESOURCE_EXHAUSTED -> ResourceExhaustedException(rpc, reason, unknown, metadata, hint, detail, httpStatus, message)
        Code.UNAVAILABLE -> UnavailableException(rpc, reason, unknown, metadata, hint, detail, httpStatus, message)
        Code.DEADLINE_EXCEEDED -> DeadlineExceededException(rpc, reason, unknown, metadata, hint, detail, httpStatus, message)
        Code.ABORTED -> AbortedException(rpc, reason, unknown, metadata, hint, detail, httpStatus, message)
        Code.INTERNAL -> InternalException(rpc, reason, unknown, metadata, hint, detail, httpStatus, message)
        Code.UNIMPLEMENTED -> UnimplementedException(rpc, reason, unknown, metadata, hint, detail, httpStatus, message)
        // Codes D611 does not name a class for — `cancelled`, `out_of_range`,
        // `data_loss` — fall through to `LoamsException` itself, which is why `code`
        // is still readable from any of them.
        else -> LoamsException(code, rpc, reason, unknown, metadata, hint, detail, httpStatus, message)
    }

    /**
     * Maps an exception that never spoke to the API: a socket, a refused connection,
     * a cancelled call, a bug in this SDK.
     *
     * A cancellation or a deadline keeps its own code rather than becoming
     * [Code.UNKNOWN], because a deadline that expired is a deadline and hiding it
     * behind `unknown` would lose the one failure a caller can always act on.
     * Everything else is [Code.UNKNOWN] with [Reason.NONE], which is exactly what
     * "a failure from below the API" means and is deliberately not any reason in the
     * registry.
     *
     * A [LoamsException] is returned **unchanged**, so mapping twice loses nothing.
     */
    fun fromException(exception: Throwable, rpc: String): LoamsException {
        if (exception is LoamsException) {
            return exception
        }
        val code = when (exception) {
            is InterruptedException -> Code.CANCELLED
            is java.util.concurrent.CancellationException -> Code.CANCELLED
            is java.net.http.HttpTimeoutException -> Code.DEADLINE_EXCEEDED
            is java.net.SocketTimeoutException -> Code.DEADLINE_EXCEEDED
            is java.net.http.HttpConnectTimeoutException -> Code.DEADLINE_EXCEEDED
            else -> Code.UNKNOWN
        }
        return LoamsException(
            code = code,
            rpc = rpc,
            reason = Reason.NONE,
            httpStatus = null,
            message = exception.message ?: exception.toString(),
            cause = exception,
        )
    }

    /** Whether a failure came from Loams rather than from below the API. */
    fun isLoamsError(exception: Throwable?): Boolean = exception is LoamsException

    /** The reason a failure carries, or [Reason.NONE] when it carries none. */
    fun reasonOf(exception: Throwable?): Reason = (exception as? LoamsException)?.reason ?: Reason.NONE

    /** The code a failure carries, or [Code.UNKNOWN]. */
    fun codeOf(exception: Throwable?): Code = (exception as? LoamsException)?.code ?: Code.UNKNOWN

    /**
     * An SDK-internal failure — a binding that does not resolve, a request whose
     * shape it cannot read.
     *
     * [Code.INTERNAL] because these are bugs in this package, not in the caller or
     * the server.
     */
    fun internal(rpc: String, message: String, cause: Throwable? = null): LoamsException =
        InternalException(rpc, Reason.INTERNAL, message = message, cause = cause)
}

/**
 * A rendering of a failure that names the code and the cause, for a log line and for
 * the `message` of a failure that arrived without one.
 */
private fun defaultMessage(code: Code, rpc: String, reason: Reason, unknownReason: String?): String {
    val cause = when {
        !unknownReason.isNullOrEmpty() -> unknownReason
        reason == Reason.NONE -> ""
        else -> ReasonRegistry.name(reason)
    }
    val prefix = if (rpc.isEmpty()) code.wireName else "$rpc: ${code.wireName}"
    return if (cause.isEmpty()) prefix else "$prefix ($cause)"
}