// The typed error hierarchy of design §44 §7.4, decision D611 (runtime contract
// R8).
//
// A failed RPC carries a Connect code and one `loams.errors.v1.ErrorInfo` in its
// details. The **code** gives the class — a taxonomy that does not change within
// an API major version — and the **`reason`** is the stable branch. The message
// is for a person and may change; nothing in this SDK branches on it.
//
// # C#'s branch is a type test
//
// Go reaches its hierarchy with `errors.As`, Java with `instanceof`, and C# has
// both:
//
//     catch (LoamsError error) when error is NotFoundError { … }
//     catch (LoamsError error) when error.Reason is Reason.ApprovalExpired { … }
//
// A `when` clause rather than a bare `catch (NotFoundError)`, because every
// subclass is a `LoamsError` and a bare catch of the subclass would be equally
// correct while losing the properties the base carries. `LoamsError` is the base
// of the hierarchy **and** the thing every failure is, so a `catch (LoamsError)`
// with no filter catches all of them — including a failure from below the API.
//
// Three cases R8 requires to stay distinct, and which this hierarchy keeps
// distinct:
//
//   - a reason from a **newer** server, which this SDK's registry does not have:
//     it is surfaced as text in <see cref="LoamsError.UnknownReason"/> and
//     flagged, not dropped;
//   - a failure from **below the API** — a socket, a refused connection, a
//     cancelled <see cref="CancellationToken"/> — which carries no reason at all
//     (<see cref="Reason.None"/>) and names no server code;
//   - a <see cref="LoamsError"/> that has already been mapped, which is returned
//     unchanged if mapped twice, so wrapping an SDK's own error never loses its
//     reason.

using System.Collections.Frozen;
using Google.Protobuf;
using Google.Protobuf.Collections;

namespace Loams;

/// <summary>
/// The Connect code, the canonical classification of a failure. It does not
/// change within an API major version.
/// </summary>
public enum Code
{
    /// <summary>The operation was cancelled, typically by the caller.</summary>
    Cancelled = 1,

    /// <summary>An error whose code the server did not say, or a failure from below the API.</summary>
    Unknown = 2,

    /// <summary>The client specified an invalid argument.</summary>
    InvalidArgument = 3,

    /// <summary>The deadline passed before the operation could complete.</summary>
    DeadlineExceeded = 4,

    /// <summary>Some requested entity was not found.</summary>
    NotFound = 5,

    /// <summary>The entity that a client attempted to create already exists.</summary>
    AlreadyExists = 6,

    /// <summary>The caller does not have permission to execute the specified operation.</summary>
    PermissionDenied = 7,

    /// <summary>Some resource has been exhausted, perhaps a per-user quota.</summary>
    ResourceExhausted = 8,

    /// <summary>The operation was rejected because the system is not in the state it requires.</summary>
    FailedPrecondition = 9,

    /// <summary>The operation was aborted, typically due to a concurrency issue.</summary>
    Aborted = 10,

    /// <summary>The operation was attempted past the valid range.</summary>
    OutOfRange = 11,

    /// <summary>The operation is not implemented or is not supported in this build.</summary>
    Unimplemented = 12,

    /// <summary>An internal error. A bug in the server or the SDK.</summary>
    Internal = 13,

    /// <summary>The service is currently unavailable.</summary>
    Unavailable = 14,

    /// <summary>Unrecoverable data loss or corruption.</summary>
    DataLoss = 15,

    /// <summary>The request does not have valid authentication credentials.</summary>
    Unauthenticated = 16,
}

/// <summary>
/// The fields of <c>loams.errors.v1.ErrorInfo</c> the runtime reads: the stable
/// cause, the structured context and the caller's next step.
/// </summary>
/// <param name="Reason">
/// The stable cause, <c>snake_case</c>. Empty when the detail carried none.
/// </param>
/// <param name="Metadata">
/// Structured context, for example <c>{"variant": "standard"}</c>. Never secrets.
/// </param>
/// <param name="Hint">A short next step in the caller's locale.</param>
public sealed record ErrorInfoShape(string Reason, IReadOnlyDictionary<string, string> Metadata, string Hint);

/// <summary>
/// Everything a Loams failure carries, and the base of the typed hierarchy. A
/// failure whose code D611 does not name is one of these directly.
/// </summary>
public class LoamsError : Exception
{
    /// <summary>The Connect code, the canonical classification.</summary>
    public Code Code { get; }

    /// <summary>
    /// The stable cause, when the server sent an <c>ErrorInfo</c>.
    /// <see cref="Reason.None"/> means the failure came from below the API — a
    /// socket, a timeout, a cancelled token — not from a Loams service, and that
    /// is a different thing from any reason in the registry.
    /// </summary>
    public Reason Reason { get; }

    /// <summary>
    /// A reason off the wire that this SDK's registry does not have, meaning the
    /// server is newer than the SDK (R8). Surfaced rather than dropped: losing it
    /// would leave a caller unable to tell "not supported here" from "not
    /// supported at all". It is a string rather than a <see cref="Reason"/>
    /// because no enum value can name it.
    /// </summary>
    public string? UnknownReason { get; }

    /// <summary>The structured context the server sent. Never secrets.</summary>
    public IReadOnlyDictionary<string, string> Metadata { get; }

    /// <summary>A short next step in the caller's locale, when the server sent one.</summary>
    public string Hint { get; }

    /// <summary>The RPC that failed, as <c>package.Service/Method</c>.</summary>
    public string Rpc { get; }

    /// <summary>The raw <c>ErrorInfo</c> detail, when there was one.</summary>
    public ErrorInfoShape? Detail { get; }

    /// <summary>The HTTP status the failure arrived with, or null when it never spoke.</summary>
    public int? HttpStatus { get; }

    /// <summary>Builds a failure. Every constructor in the hierarchy funnels here.</summary>
    public LoamsError(
        Code code,
        string rpc,
        Reason reason = Reason.None,
        string? unknownReason = null,
        IReadOnlyDictionary<string, string>? metadata = null,
        string hint = "",
        ErrorInfoShape? detail = null,
        int? httpStatus = null,
        Exception? innerException = null,
        string? message = null)
        : base(message ?? defaultMessage(code, rpc, reason, unknownReason), innerException)
    {
        Code = code;
        Rpc = rpc;
        Reason = reason;
        UnknownReason = unknownReason;
        Metadata = metadata ?? EmptyMetadata;
        Hint = hint;
        Detail = detail;
        HttpStatus = httpStatus;
    }

    private static readonly IReadOnlyDictionary<string, string> EmptyMetadata =
        FrozenDictionary<string, string>.Empty;

    private static string defaultMessage(Code code, string rpc, Reason reason, string? unknownReason)
    {
        var cause = unknownReason is { Length: > 0 }
            ? unknownReason
            : reason == Reason.None ? string.Empty : ReasonRegistry.Name(reason);
        var prefix = string.IsNullOrEmpty(rpc) ? code.ToString().ToLowerInvariant() : $"{rpc}: {code.ToString().ToLowerInvariant()}";
        return cause.Length == 0 ? prefix : $"{prefix} ({cause})";
    }
}

/// <summary>
/// What every failure is, and the base the whole hierarchy shares. A
/// <c>catch (LoamsError)</c> with no filter catches a typed failure, a refusal
/// the registry has no reason for, and a failure from below the API alike — which
/// is the point: one type to catch, and <see cref="LoamsError.Reason"/> to branch
/// on.
/// </summary>
public abstract class LoamsException : LoamsError
{
    /// <summary>Builds a typed failure.</summary>
    protected LoamsException(
        Code code,
        string rpc,
        Reason reason = Reason.None,
        string? unknownReason = null,
        IReadOnlyDictionary<string, string>? metadata = null,
        string hint = "",
        ErrorInfoShape? detail = null,
        int? httpStatus = null,
        Exception? innerException = null,
        string? message = null)
        : base(code, rpc, reason, unknownReason, metadata, hint, detail, httpStatus, innerException, message)
    {
    }
}

// One class per Connect code D611 names, so the branch is a type test. None adds
// a field of its own: the branch is what a caller wants and the reason is on the
// base. Codes D611 does not name (`cancelled`, `out_of_range`, `data_loss`) fall
// through to a `LoamsError` itself, which is why `Code` is still readable from
// any of them.

/// <summary><c>invalid_argument</c>: a malformed field or an unparseable value.</summary>
public sealed class InvalidArgumentError : LoamsException
{
    /// <summary>Builds an <c>invalid_argument</c> failure.</summary>
    public InvalidArgumentError(string rpc, Reason reason = Reason.InvalidArgument, string? unknownReason = null,
        IReadOnlyDictionary<string, string>? metadata = null, string hint = "", ErrorInfoShape? detail = null,
        int? httpStatus = null, string? message = null)
        : base(Code.InvalidArgument, rpc, reason, unknownReason, metadata, hint, detail, httpStatus, null, message) { }
}

/// <summary><c>not_found</c>: the named resource does not exist.</summary>
public sealed class NotFoundError : LoamsException
{
    /// <summary>Builds a <c>not_found</c> failure.</summary>
    public NotFoundError(string rpc, Reason reason = Reason.NotFound, string? unknownReason = null,
        IReadOnlyDictionary<string, string>? metadata = null, string hint = "", ErrorInfoShape? detail = null,
        int? httpStatus = null, string? message = null)
        : base(Code.NotFound, rpc, reason, unknownReason, metadata, hint, detail, httpStatus, null, message) { }
}

/// <summary><c>already_exists</c>: the named resource already exists.</summary>
public sealed class AlreadyExistsError : LoamsException
{
    /// <summary>Builds an <c>already_exists</c> failure.</summary>
    public AlreadyExistsError(string rpc, Reason reason = Reason.AlreadyExists, string? unknownReason = null,
        IReadOnlyDictionary<string, string>? metadata = null, string hint = "", ErrorInfoShape? detail = null,
        int? httpStatus = null, string? message = null)
        : base(Code.AlreadyExists, rpc, reason, unknownReason, metadata, hint, detail, httpStatus, null, message) { }
}

/// <summary><c>permission_denied</c>: the caller's role may not make this call.</summary>
public sealed class PermissionDeniedError : LoamsException
{
    /// <summary>Builds a <c>permission_denied</c> failure.</summary>
    public PermissionDeniedError(string rpc, Reason reason = Reason.PermissionDenied, string? unknownReason = null,
        IReadOnlyDictionary<string, string>? metadata = null, string hint = "", ErrorInfoShape? detail = null,
        int? httpStatus = null, string? message = null)
        : base(Code.PermissionDenied, rpc, reason, unknownReason, metadata, hint, detail, httpStatus, null, message) { }
}

/// <summary>
/// <c>unauthenticated</c>: no credential, or one that cannot be used. This is the
/// class R1's refresh applies under; <see cref="TokenExpiredError"/> is a
/// narrower failure inside it and is distinguished by
/// <see cref="LoamsError.Reason"/>, not by the class.
/// </summary>
public class UnauthenticatedError : LoamsException
{
    /// <summary>Builds an <c>unauthenticated</c> failure.</summary>
    public UnauthenticatedError(string rpc, Reason reason = Reason.Unauthenticated, string? unknownReason = null,
        IReadOnlyDictionary<string, string>? metadata = null, string hint = "", ErrorInfoShape? detail = null,
        int? httpStatus = null, string? message = null)
        : base(Code.Unauthenticated, rpc, reason, unknownReason, metadata, hint, detail, httpStatus, null, message) { }
}

/// <summary><c>failed_precondition</c>: the call's preconditions do not hold.</summary>
public sealed class FailedPreconditionError : LoamsException
{
    /// <summary>Builds a <c>failed_precondition</c> failure.</summary>
    public FailedPreconditionError(string rpc, Reason reason = Reason.FailedPrecondition, string? unknownReason = null,
        IReadOnlyDictionary<string, string>? metadata = null, string hint = "", ErrorInfoShape? detail = null,
        int? httpStatus = null, string? message = null)
        : base(Code.FailedPrecondition, rpc, reason, unknownReason, metadata, hint, detail, httpStatus, null, message) { }
}

/// <summary><c>resource_exhausted</c>: a backpressure or quota refusal.</summary>
public sealed class ResourceExhaustedError : LoamsException
{
    /// <summary>Builds a <c>resource_exhausted</c> failure.</summary>
    public ResourceExhaustedError(string rpc, Reason reason = Reason.ResourceExhausted, string? unknownReason = null,
        IReadOnlyDictionary<string, string>? metadata = null, string hint = "", ErrorInfoShape? detail = null,
        int? httpStatus = null, string? message = null)
        : base(Code.ResourceExhausted, rpc, reason, unknownReason, metadata, hint, detail, httpStatus, null, message) { }
}

/// <summary><c>unavailable</c>: a dependency is down, or this node cannot serve the read.</summary>
public sealed class UnavailableError : LoamsException
{
    /// <summary>Builds an <c>unavailable</c> failure.</summary>
    public UnavailableError(string rpc, Reason reason = Reason.Unavailable, string? unknownReason = null,
        IReadOnlyDictionary<string, string>? metadata = null, string hint = "", ErrorInfoShape? detail = null,
        int? httpStatus = null, string? message = null)
        : base(Code.Unavailable, rpc, reason, unknownReason, metadata, hint, detail, httpStatus, null, message) { }
}

/// <summary><c>deadline_exceeded</c>: the caller's deadline passed.</summary>
public sealed class DeadlineExceededError : LoamsException
{
    /// <summary>Builds a <c>deadline_exceeded</c> failure.</summary>
    public DeadlineExceededError(string rpc, Reason reason = Reason.DeadlineExceeded, string? unknownReason = null,
        IReadOnlyDictionary<string, string>? metadata = null, string hint = "", ErrorInfoShape? detail = null,
        int? httpStatus = null, string? message = null)
        : base(Code.DeadlineExceeded, rpc, reason, unknownReason, metadata, hint, detail, httpStatus, null, message) { }
}

/// <summary><c>aborted</c>: a concurrent write won; the caller retries.</summary>
public sealed class AbortedError : LoamsException
{
    /// <summary>Builds an <c>aborted</c> failure.</summary>
    public AbortedError(string rpc, Reason reason = Reason.Aborted, string? unknownReason = null,
        IReadOnlyDictionary<string, string>? metadata = null, string hint = "", ErrorInfoShape? detail = null,
        int? httpStatus = null, string? message = null)
        : base(Code.Aborted, rpc, reason, unknownReason, metadata, hint, detail, httpStatus, null, message) { }
}

/// <summary><c>internal</c>: a bug. The message and the request id go to the log.</summary>
public sealed class InternalError : LoamsException
{
    /// <summary>Builds an <c>internal</c> failure.</summary>
    public InternalError(string rpc, Reason reason = Reason.Internal, string? unknownReason = null,
        IReadOnlyDictionary<string, string>? metadata = null, string hint = "", ErrorInfoShape? detail = null,
        int? httpStatus = null, string? message = null)
        : base(Code.Internal, rpc, reason, unknownReason, metadata, hint, detail, httpStatus, null, message) { }
}

/// <summary><c>unimplemented</c>: the handler does not exist in this build.</summary>
public class UnimplementedError : LoamsException
{
    /// <summary>Builds an <c>unimplemented</c> failure.</summary>
    public UnimplementedError(string rpc, Reason reason = Reason.NotImplemented, string? unknownReason = null,
        IReadOnlyDictionary<string, string>? metadata = null, string hint = "", ErrorInfoShape? detail = null,
        int? httpStatus = null, string? message = null)
        : base(Code.Unimplemented, rpc, reason, unknownReason, metadata, hint, detail, httpStatus, null, message) { }
}

/// <summary>
/// A package this build variant does not carry (design §44 §4, D600).
/// </summary>
/// <remarks>
/// The server answers <c>unimplemented</c> with
/// <c>reason = feature_not_in_variant</c> and names the variant in
/// <c>metadata.variant</c>, which is what <see cref="Variant"/> reads. A caller
/// usually never gets here: <c>client.System.GuardAsync</c> feature-detects
/// from <c>GetInstance.Services[]</c> before calling, so an unavailable module
/// raises this same type from the guard with no request spent. One
/// <c>catch</c> therefore covers "the guard said no" and "the server refused",
/// which is the point of the guard raising the same type.
/// </remarks>
public sealed class FeatureNotInVariantError : UnimplementedError
{
    /// <summary>
    /// The build variant that was asked for, from <c>metadata.variant</c>. The
    /// guard raises this type from the catalogue, which does not carry a variant
    /// name, so it is <see cref="string.Empty"/> there rather than a guess.
    /// </summary>
    public string Variant { get; }

    /// <summary>Builds a variant refusal, reading the variant out of the metadata.</summary>
    public FeatureNotInVariantError(string rpc, IReadOnlyDictionary<string, string>? metadata = null,
        string hint = "", ErrorInfoShape? detail = null, int? httpStatus = null, string? message = null)
        : base(rpc, Reason.FeatureNotInVariant, metadata: metadata, hint: hint, detail: detail,
               httpStatus: httpStatus, message: message)
    {
        Variant = metadata is not null && metadata.TryGetValue("variant", out var variant) ? variant : string.Empty;
    }
}

/// <summary>
/// A token the server rejected as expired: <c>unauthenticated</c> with reason
/// <c>token_expired</c>. The runtime refreshes once and retries once (D608, R1);
/// a second expiry reaches the caller as this type.
/// </summary>
public sealed class TokenExpiredError : UnauthenticatedError
{
    /// <summary>Builds a <c>token_expired</c> failure.</summary>
    public TokenExpiredError(string rpc, IReadOnlyDictionary<string, string>? metadata = null,
        string hint = "", ErrorInfoShape? detail = null, int? httpStatus = null, string? message = null)
        : base(rpc, Reason.TokenExpired, metadata: metadata, hint: hint, detail: detail,
               httpStatus: httpStatus, message: message) { }
}

/// <summary>
/// What the error mapping needs from a failure that arrived over the wire: the
/// code, the message and the <c>ErrorInfo</c> detail, whatever protocol carried
/// it. The transport fills this in; the mapping below reads it.
/// </summary>
public sealed record WireFailure(
    Code Code,
    string Message,
    ErrorInfoShape? Detail = null,
    int? HttpStatus = null,
    string? Rpc = null);

/// <summary>
/// Turns anything that came back — or threw — into the typed hierarchy (D611).
/// </summary>
public static class ErrorMapper
{
    /// <summary>
    /// Maps a wire failure into the class its code names, with <c>reason</c> and
    /// <c>metadata</c> lifted out of the <c>ErrorInfo</c> detail.
    /// </summary>
    /// <remarks>
    /// The detail is looked up <b>by its type</b>, never by position, so a service
    /// that adds a detail of its own cannot move <c>reason</c> out from under a
    /// caller. A detail whose bytes did not parse is reported as "no
    /// <c>ErrorInfo</c>" rather than dropped silently, because the alternative is
    /// a Loams failure with no reason and no hint, which is the one thing R8 says
    /// must not happen.
    /// </remarks>
    public static LoamsError Map(WireFailure failure, string rpc)
    {
        ArgumentNullException.ThrowIfNull(failure);

        var detail = failure.Detail;
        var metadata = detail?.Metadata ?? EmptyMetadata;
        var hint = detail?.Hint ?? string.Empty;

        var known = detail is { Reason.Length: > 0 } && ReasonRegistry.FromWire(detail.Reason) is { } mapped
            ? mapped
            : Reason.None;
        var unknown = known == Reason.None && detail is { Reason.Length: > 0 } ? detail.Reason : null;

        // The two reasons D611 gives their own class are decided **before** the
        // code-to-class table runs, because both are refinements of a code the
        // table would otherwise answer on its own.
        if (known == Reason.FeatureNotInVariant)
        {
            return new FeatureNotInVariantError(rpc, metadata, hint, detail, failure.HttpStatus, failure.Message);
        }
        if (failure.Code == Code.Unauthenticated && known == Reason.TokenExpired)
        {
            return new TokenExpiredError(rpc, metadata, hint, detail, failure.HttpStatus, failure.Message);
        }

        return ByCode(failure.Code, rpc, known, unknown, metadata, hint, detail, failure.HttpStatus, failure.Message);
    }

    private static readonly IReadOnlyDictionary<string, string> EmptyMetadata =
        FrozenDictionary<string, string>.Empty;

    private static LoamsError ByCode(Code code, string rpc, Reason reason, string? unknown,
        IReadOnlyDictionary<string, string> metadata, string hint, ErrorInfoShape? detail, int? httpStatus,
        string message) => code switch
        {
            Code.InvalidArgument => new InvalidArgumentError(rpc, reason, unknown, metadata, hint, detail, httpStatus, message),
            Code.NotFound => new NotFoundError(rpc, reason, unknown, metadata, hint, detail, httpStatus, message),
            Code.AlreadyExists => new AlreadyExistsError(rpc, reason, unknown, metadata, hint, detail, httpStatus, message),
            Code.PermissionDenied => new PermissionDeniedError(rpc, reason, unknown, metadata, hint, detail, httpStatus, message),
            Code.Unauthenticated => new UnauthenticatedError(rpc, reason, unknown, metadata, hint, detail, httpStatus, message),
            Code.FailedPrecondition => new FailedPreconditionError(rpc, reason, unknown, metadata, hint, detail, httpStatus, message),
            Code.ResourceExhausted => new ResourceExhaustedError(rpc, reason, unknown, metadata, hint, detail, httpStatus, message),
            Code.Unavailable => new UnavailableError(rpc, reason, unknown, metadata, hint, detail, httpStatus, message),
            Code.DeadlineExceeded => new DeadlineExceededError(rpc, reason, unknown, metadata, hint, detail, httpStatus, message),
            Code.Aborted => new AbortedError(rpc, reason, unknown, metadata, hint, detail, httpStatus, message),
            Code.Internal => new InternalError(rpc, reason, unknown, metadata, hint, detail, httpStatus, message),
            Code.Unimplemented => new UnimplementedError(rpc, reason, unknown, metadata, hint, detail, httpStatus, message),
            _ => new LoamsError(code, rpc, reason, unknown, metadata, hint, detail, httpStatus, null, message),
        };

    /// <summary>
    /// Maps an exception that never spoke to the API: a socket, a refused
    /// connection, a cancelled <see cref="CancellationToken"/>, a bug in this SDK.
    /// </summary>
    /// <remarks>
    /// A cancellation or a deadline keeps its own code rather than becoming
    /// <see cref="Code.Unknown"/>, because a deadline that expired is a deadline
    /// and hiding it behind <c>unknown</c> would lose the one failure a caller can
    /// always act on. Everything else is <see cref="Code.Unknown"/> with
    /// <see cref="Reason.None"/>, which is exactly what "a failure from below the
    /// API" means and is deliberately not any reason in the registry.
    /// </remarks>
    public static LoamsError FromException(Exception exception, string rpc)
    {
        ArgumentNullException.ThrowIfNull(exception);

        // Already ours: returned unchanged, so mapping twice loses nothing.
        if (exception is LoamsError already)
        {
            return already;
        }

        var code = exception switch
        {
            OperationCanceledException => Code.Cancelled,
            TimeoutException => Code.DeadlineExceeded,
            HttpRequestException => Code.Unavailable,
            _ => Code.Unknown,
        };

        return new LoamsError(code, rpc, Reason.None, metadata: EmptyMetadata, innerException: exception,
            message: exception.Message);
    }

    /// <summary>Whether a failure came from Loams rather than from below the API.</summary>
    public static bool IsLoamsError(Exception? exception) => exception is LoamsError;

    /// <summary>
    /// The reason a failure carries, or <see cref="Reason.None"/> when it carries
    /// none. A failure from below the API has no reason, and that is a different
    /// thing from a Loams service refusing.
    /// </summary>
    public static Reason ReasonOf(Exception? exception) =>
        exception is LoamsError error ? error.Reason : Reason.None;

    /// <summary>The code a failure carries, or <see cref="Code.Unknown"/>.</summary>
    public static Code CodeOf(Exception? exception) =>
        exception is LoamsError error ? error.Code : Code.Unknown;

    /// <summary>
    /// An SDK-internal failure — a binding that does not resolve, a request whose
    /// shape it cannot read. <see cref="Code.Internal"/> because they are bugs in
    /// this package, not in the caller or the server.
    /// </summary>
    internal static LoamsError Internal(string rpc, string message, Exception? inner = null) =>
        new(Code.Internal, rpc, Reason.Internal, metadata: EmptyMetadata, innerException: inner, message: message);
}

/// <summary>
/// Reads the <c>loams.errors.v1.ErrorInfo</c> detail out of the two shapes the
/// wire uses, so the rest of the SDK never sees either.
/// </summary>
/// <remarks>
/// Two shapes, one answer. A Connect JSON error body carries
/// <c>details[]</c> as <c>{"type", "value"}</c> with <c>value</c> base64 of the
/// serialized detail. A gRPC or gRPC-Web refusal carries the same detail inside
/// <c>grpc-status-details-bin</c>, which is base64 of a <c>google.rpc.Status</c>
/// whose <c>details[]</c> are <c>google.protobuf.Any</c>. The lookup is by type
/// URL in both cases, never by position.
/// </remarks>
public static class ErrorInfoCodec
{
    /// <summary>The type URL and name of the detail this SDK reads.</summary>
    public const string ErrorInfoType = "loams.errors.v1.ErrorInfo";

    /// <summary>The type URL a server puts in an <c>Any</c> for that detail.</summary>
    public const string ErrorInfoTypeUrl = "type.googleapis.com/loams.errors.v1.ErrorInfo";

    /// <summary>
    /// Decodes a detail from its serialized bytes, or <see langword="null"/> when
    /// the bytes are not an <c>ErrorInfo</c> this runtime can read.
    /// </summary>
    public static ErrorInfoShape? Decode(ReadOnlySpan<byte> bytes)
    {
        try
        {
            var info = Loams.Errors.V1.ErrorInfo.Parser.ParseFrom(bytes.ToArray());
            return new ErrorInfoShape(info.Reason, Frozen(info.Metadata), info.Hint);
        }
        catch (InvalidProtocolBufferException)
        {
            // The narrow type, not `Exception`: the only thing this parse can
            // throw is a malformed `ErrorInfo`, and catching wider would also
            // swallow an `OutOfMemoryException` or a `NullReferenceException` from
            // a bug in the generated code and report it as "the server sent no
            // reason", which is a lie that hides the bug.
            //
            // Reported as "no ErrorInfo" rather than thrown: the alternative is a
            // Loams failure with no reason and no hint.
            return null;
        }
    }

    /// <summary>
    /// A <c>MapField</c> as an immutable dictionary.
    /// </summary>
    /// <remarks>
    /// Copied rather than wrapped: a <c>MapField</c> is mutable and the caller holds
    /// a reference to it, so an <see cref="ErrorInfoShape"/> that wrapped one would
    /// change under a caller who compared two failures. The map is at most a handful
    /// of entries — the corpus's largest is two — so the copy is not worth worrying
    /// about.
    /// </remarks>
    private static IReadOnlyDictionary<string, string> Frozen(MapField<string, string> metadata)
    {
        var copy = new Dictionary<string, string>(metadata.Count, StringComparer.Ordinal);
        foreach (var (key, value) in metadata)
        {
            copy[key] = value;
        }
        return copy;
    }

    /// <summary>
    /// The detail whose type URL is <see cref="ErrorInfoTypeUrl"/>, searched
    /// across the Any values a <c>google.rpc.Status</c> carries. Position is
    /// never used.
    /// </summary>
    /// <remarks>
    /// <c>google.rpc.Status</c> is read by hand rather than through a generated
    /// type, because no proto in this repository declares it and adding one for
    /// a message only ever appears inside an error trailer would be a second
    /// source of truth for the wire format. Only the two fields this reads are
    /// walked — <c>code</c> and <c>details</c> — and an unknown field is skipped
    /// rather than refused, because a server is free to add one.
    /// </remarks>
    public static ErrorInfoShape? FromStatusBytes(ReadOnlySpan<byte> statusBytes)
    {
        var reader = new CodedInputStream(statusBytes.ToArray());
        try
        {
            while (!reader.IsAtEnd)
            {
                var tag = reader.ReadTag();
                if (tag == 0)
                {
                    break;
                }
                switch (WireFormat.GetTagFieldNumber(tag))
                {
                    case 3: // repeated google.protobuf.Any details
                    {
                        // Read as a length-delimited blob rather than as a nested
                        // message: `CodedInputStream` has no push/pop limit, and the
                        // blob is handed to a parser that does its own framing.
                        var shape = FromAnyBytes(reader.ReadBytes().ToByteArray());
                        if (shape is not null)
                        {
                            return shape;
                        }
                        break;
                    }
                    case 1: // int32 code
                    case 2: // string message
                        reader.SkipLastField();
                        break;
                    default:
                        reader.SkipLastField();
                        break;
                }
            }
        }
        catch (InvalidProtocolBufferException)
        {
            return null;
        }
        return null;
    }

    /// <summary>
    /// The <c>ErrorInfo</c> inside a serialized <c>google.protobuf.Any</c>, or
    /// null when the Any names something else.
    /// </summary>
    private static ErrorInfoShape? FromAnyBytes(byte[] anyBytes)
    {
        var reader = new CodedInputStream(anyBytes);
        string? typeUrl = null;
        byte[]? value = null;
        while (!reader.IsAtEnd)
        {
            var tag = reader.ReadTag();
            if (tag == 0)
            {
                break;
            }
            switch (WireFormat.GetTagFieldNumber(tag))
            {
                case 1: // string type_url
                    typeUrl = reader.ReadString();
                    break;
                case 2: // bytes value
                    value = reader.ReadBytes().ToByteArray();
                    break;
                default:
                    reader.SkipLastField();
                    break;
            }
        }
        if (typeUrl is null || value is null || !typeUrl.EndsWith(ErrorInfoTypeUrl, StringComparison.Ordinal))
        {
            return null;
        }
        return Decode(value);
    }
}