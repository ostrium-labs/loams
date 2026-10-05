package dev.loams;

import dev.loams.connect.ConnectError;
import dev.loams.connect.ConnectErrorDetail;
import dev.loams.connect.ConnectFailure;
import dev.loams.facade.Facade;
import dev.loams.facade.Reason;
import dev.loams.gen.loams.errors.v1.ErrorInfo;
import java.io.IOException;
import java.io.InterruptedIOException;
import java.net.http.HttpTimeoutException;
import java.util.Map;

/**
 * The mapper: whatever went wrong becomes the typed hierarchy of {@link LoamsException}
 * (design §44 §7.4, D611).
 *
 * <p>Three sources, and they are deliberately kept apart:
 *
 * <ul>
 *   <li>a {@link ConnectFailure} carrying a {@link ConnectError}, which becomes the class its
 *       code names, with {@code reason} and {@code metadata} lifted out of the {@code ErrorInfo}
 *       detail — looked up <b>by type</b>, never by position, so a service that adds a detail of
 *       its own cannot move {@code reason} out from under a caller;
 *   <li>a failure from <b>below the API</b> — a socket, a refused connection, an interrupt, an
 *       {@link HttpTimeoutException} — which carries no {@code reason} at all, because no service
 *       answered; and
 *   <li>a {@link LoamsException} already, which is returned <b>unchanged</b>, so mapping twice
 *       loses nothing and wrapping the SDK's own failure never drops its reason.
 * </ul>
 *
 * <p>A context failure keeps its own code rather than collapsing into {@link Code#UNKNOWN}: a
 * deadline that expired is a deadline, and hiding it behind {@code unknown} would lose the one
 * failure a caller can always act on.
 */
public final class Errors {

    private Errors() {}

    /** The reason an error carries, or {@code null} when it carries none. */
    public static Reason reasonOf(Throwable error) {
        if (error instanceof LoamsException loams) {
            return loams.reason();
        }
        return null;
    }

    /** The code an error carries, or {@link Code#UNKNOWN} for one that is not a Loams failure. */
    public static Code codeOf(Throwable error) {
        if (error instanceof LoamsException loams) {
            return loams.code();
        }
        if (error instanceof ConnectFailure failure && failure.error() != null) {
            return Code.fromWire(failure.error().code());
        }
        return Code.UNKNOWN;
    }

    /**
     * Whether an error came from Loams rather than from below the API.
     *
     * <p>The Java spelling of Go's {@code errors.As(err, &loamsErr)}: a caller who wants a
     * boolean rather than a type branch.
     */
    public static boolean isLoamsError(Throwable error) {
        return error instanceof LoamsException;
    }

    /**
     * Map anything to the typed hierarchy.
     *
     * @param error the failure
     * @param rpc the RPC that failed, as {@code package.Service/Method}
     */
    public static LoamsException toLoamsException(Throwable error, String rpc) {
        if (error == null) {
            return null;
        }
        if (error instanceof LoamsException already) {
            return already;
        }
        // An `UncheckedIOException` is a wrapper the call path puts around a checked failure so
        // it can cross a lambda, and it must be unwrapped **before** anything is classified. Left
        // in, a server refusal would be reported as a failure from below the API — with no reason
        // and no hint, which is the single outcome R8 forbids, and it would look like a socket
        // problem to whoever debugs it.
        if (error instanceof java.io.UncheckedIOException wrapper && wrapper.getCause() != null) {
            return toLoamsException(wrapper.getCause(), rpc);
        }
        if (error instanceof ConnectFailure failure) {
            if (failure.error() == null) {
                // Bytes that were not a Connect error: a proxy's page, a reset connection. The
                // cause carries the real answer and the code says "below the API".
                return belowApi(failure, rpc);
            }
            return fromWire(failure.error(), failure.rpc(), failure);
        }
        return belowApi(error, rpc);
    }

    private static LoamsException belowApi(Throwable error, String rpc) {
        Code code = Code.UNKNOWN;
        if (error instanceof HttpTimeoutException || unwrap(error) instanceof HttpTimeoutException) {
            code = Code.DEADLINE_EXCEEDED;
        } else if (error instanceof InterruptedException || error instanceof InterruptedIOException) {
            // An interrupt is the caller's own cancellation reaching the transport. Reporting it
            // as `canceled` is what lets a `catch (DeadlineExceededException)`-shaped caller
            // see the truth instead of an opaque `unknown`.
            code = Code.CANCELED;
        }
        return byCode(code, null, "", Map.of(), "", rpc, error);
    }

    private static Throwable unwrap(Throwable error) {
        Throwable cause = error.getCause();
        return cause == null ? error : cause;
    }

    /** Map a wire error onto the class its code names. */
    private static LoamsException fromWire(ConnectError error, String rpc, Throwable cause) {
        Code code = Code.fromWire(error.code());
        ErrorInfoShape info = errorInfoOf(error);
        Reason reason = null;
        String unknown = "";
        String metadata = "";
        Map<String, String> context = Map.of();
        String hint = "";
        if (info != null) {
            reason = Reason.fromWire(info.reason());
            if (reason == null && !info.reason().isEmpty()) {
                unknown = info.reason();
            }
            context = info.metadata();
            hint = info.hint();
            metadata = info.reason();
        }
        String rpcName = rpc == null || rpc.isEmpty() ? "" : rpc;

        if (reason == Reason.FEATURE_NOT_IN_VARIANT) {
            return new FeatureNotInVariantException(
                    code,
                    reason,
                    unknown,
                    context,
                    hint,
                    rpcName,
                    cause,
                    context.getOrDefault("variant", ""));
        }
        if (code == Code.UNAUTHENTICATED && reason == Reason.TOKEN_EXPIRED) {
            return new TokenExpiredException(code, reason, unknown, context, hint, rpcName, cause);
        }
        return byCode(code, reason, unknown, context, hint, rpcName, cause);
    }

    /** The code-to-class mapping of D611. Codes D611 does not name fall through to the base. */
    private static LoamsException byCode(
            Code code,
            Reason reason,
            String unknown,
            Map<String, String> metadata,
            String hint,
            String rpc,
            Throwable cause) {
        return switch (code) {
            case INVALID_ARGUMENT ->
                    new InvalidArgumentException(code, reason, unknown, metadata, hint, rpc, cause);
            case NOT_FOUND -> new NotFoundException(code, reason, unknown, metadata, hint, rpc, cause);
            case ALREADY_EXISTS ->
                    new AlreadyExistsException(code, reason, unknown, metadata, hint, rpc, cause);
            case PERMISSION_DENIED ->
                    new PermissionDeniedException(code, reason, unknown, metadata, hint, rpc, cause);
            case UNAUTHENTICATED ->
                    new UnauthenticatedException(code, reason, unknown, metadata, hint, rpc, cause);
            case FAILED_PRECONDITION ->
                    new FailedPreconditionException(code, reason, unknown, metadata, hint, rpc, cause);
            case RESOURCE_EXHAUSTED ->
                    new ResourceExhaustedException(code, reason, unknown, metadata, hint, rpc, cause);
            case UNAVAILABLE ->
                    new UnavailableException(code, reason, unknown, metadata, hint, rpc, cause);
            case DEADLINE_EXCEEDED ->
                    new DeadlineExceededException(code, reason, unknown, metadata, hint, rpc, cause);
            case ABORTED -> new AbortedException(code, reason, unknown, metadata, hint, rpc, cause);
            case INTERNAL -> new InternalException(code, reason, unknown, metadata, hint, rpc, cause);
            case UNIMPLEMENTED ->
                    new UnimplementedException(code, reason, unknown, metadata, hint, rpc, cause);
            default -> new LoamsException(code, reason, unknown, metadata, hint, rpc, cause, false);
        };
    }

    /**
     * The {@code ErrorInfo} a wire error carries, or {@code null} when it carries none.
     *
     * <p>Looked up <b>by type</b>: a service that adds a detail of its own must not move
     * {@code reason} out from under a caller.
     */
    public static ErrorInfoShape errorInfoOf(ConnectError error) {
        if (error == null) {
            return null;
        }
        ConnectErrorDetail detail = error.detail(Facade.ERROR_INFO_TYPE);
        if (detail == null) {
            return null;
        }
        try {
            ErrorInfo info = ErrorInfo.parseFrom(detail.value());
            return new ErrorInfoShape(info.getReason(), info.getMetadataMap(), info.getHint());
        } catch (IOException e) {
            // The detail's bytes did not parse. Reported as "no ErrorInfo" rather than dropped
            // silently, because the alternative is a Loams failure with no reason and no hint —
            // the one thing R8 says must not happen. The other details are still on the error.
            return null;
        }
    }

    /**
     * The SDK's own failures — a binding that does not resolve, a request whose shape it cannot
     * read, a usage error.
     *
     * <p>They are {@link Code#INTERNAL} with {@link Reason#INTERNAL} because they are bugs in
     * this package or a mistake in the caller's use of it, never a server fault. Reporting them
     * as {@code unavailable} would put them into the retry policy's retryable set and turn a
     * permanent wiring mistake into three attempts.
     */
    public static InternalException internal(String rpc, String message, Throwable cause) {
        return new InternalException(
                Code.INTERNAL,
                Reason.INTERNAL,
                "",
                Map.of(),
                "",
                rpc == null ? "" : rpc,
                cause == null ? new IllegalStateException(message) : causedBy(message, cause));
    }

    private static Throwable causedBy(String message, Throwable cause) {
        IllegalStateException wrapper = new IllegalStateException(message, cause);
        return wrapper;
    }
}