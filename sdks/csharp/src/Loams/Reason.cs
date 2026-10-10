// The `ErrorInfo.reason` registry (design §44 §7.4, D611; runtime contract R8).
//
// A failed RPC carries a Connect code and one `loams.errors.v1.ErrorInfo` in its
// details. The **code** gives the class and the **`reason`** is the branch: a
// stable `snake_case` string, registered in `docs/api/reasons.md` and never
// renamed within an API major version. A caller branches on `Reason`, never on
// the message, which may change.
//
// The enum is the registry in the only form C# can make exhaustive: a `switch`
// over it is checked by the compiler, and a reason the registry adds later is a
// value a caller cannot have handled without a warning. Three states are
// deliberately distinct rather than collapsed (R8):
//
//   - `Reason.None` — the failure came from **below the API** (a socket, a
//     timeout, a cancelled token) or the server sent no `ErrorInfo`. It carries
//     no reason, which is not the same thing as any reason in the registry;
//   - a reason off the registry, from a server newer than this SDK: it is
//     surfaced as the **text** in `LoamsError.UnknownReason` rather than
//     dropped, because dropping it leaves a caller unable to tell "not
//     supported here" from "not supported at all";
//   - a reason in the registry, which is `LoamsError.Reason`.
//
// # Provenance
//
// Hand-written, and it is the same hand-written answer `sdks/go` and `sdks/java`
// give: design §44 §7.3's **Q604 fallback**, which permits a hand-written
// facade checked by the same conformance suite where the generator is too costly
// for a language. The C# renderer does not exist in
// `crates/loams-facade-gen`, and this file is the module surface's half of that
// fallback (see `Facade.cs` for the other half).
//
// The list is transcribed from `docs/api/reasons.md`, and
// `csharp_error_reason_mapping` reads that page and fails on a drift, so this
// enum cannot quietly fall behind the registry. **Do not add a reason here
// without adding its row to that page first**: the registry's rule is that a
// reason is registered before an RPC returns it.

using System.Collections.Frozen;

namespace Loams;

/// <summary>
/// The stable, machine-readable cause of a failed RPC. See
/// <c>docs/api/reasons.md</c>, the single registry.
/// </summary>
public enum Reason
{
    /// <summary>
    /// The failure carried no reason: it came from below the API, or the server
    /// sent an error with no <c>ErrorInfo</c>. Distinct from every registry
    /// reason, and R8 requires it to stay distinct — conflating it with
    /// <see cref="TokenExpired"/> would make a client refresh a request that has
    /// no credential to refresh.
    /// </summary>
    None = 0,

    /// <summary>The approval was still pending and is no longer.</summary>
    ApprovalExpired = 1,

    /// <summary>The approval was already decided, by this or another caller.</summary>
    ApprovalAlreadyDecided = 2,

    /// <summary>The decision named a revision the server is not on.</summary>
    ApprovalStaleRevision = 3,

    /// <summary>An agent requested the operation, so its requester cannot approve it.</summary>
    RequesterCannotApprove = 4,

    /// <summary>The decision proof did not verify against the paired device key.</summary>
    DecisionProofInvalid = 5,

    /// <summary>
    /// The credential is valid but too old for this decision. R1's refresh
    /// trigger; distinct from <see cref="Unauthenticated"/>.
    /// </summary>
    StepUpRequired = 6,

    /// <summary>Rejecting, or approving something destructive, needs a reason.</summary>
    ReasonRequired = 7,

    /// <summary>The decision is not one the policy allows.</summary>
    InvalidDecision = 8,

    /// <summary>The pairing grant is past its expiry.</summary>
    PairingExpired = 9,

    /// <summary>The pairing grant was already redeemed; it is single-use.</summary>
    PairingUsed = 10,

    /// <summary>The device the token is bound to has been revoked.</summary>
    DeviceRevoked = 11,

    /// <summary>The push target the request named is not registered.</summary>
    PushTargetUnknown = 12,

    /// <summary>A stub handler whose service has not landed yet.</summary>
    NotImplemented = 13,

    /// <summary>
    /// A catalogue package whose engine is not in this build variant
    /// (design §44 §4, D600). The variant is in <c>metadata.variant</c>.
    /// </summary>
    FeatureNotInVariant = 14,

    /// <summary>A malformed field or an unparseable value.</summary>
    InvalidArgument = 15,

    /// <summary>The named resource does not exist.</summary>
    NotFound = 16,

    /// <summary>The named resource already exists.</summary>
    AlreadyExists = 17,

    /// <summary>The caller's role may not make this call.</summary>
    PermissionDenied = 18,

    /// <summary>
    /// A rejected access token, which R1 refreshes once and retries once. A
    /// second expiry reaches the caller as a
    /// <see cref="Loams.TokenExpiredError"/>.
    /// </summary>
    TokenExpired = 19,

    /// <summary>No credential, or one that cannot be used.</summary>
    Unauthenticated = 20,

    /// <summary>The call's preconditions do not hold.</summary>
    FailedPrecondition = 21,

    /// <summary>A backpressure or quota refusal.</summary>
    ResourceExhausted = 22,

    /// <summary>A dependency is down, or this node cannot serve the read.</summary>
    Unavailable = 23,

    /// <summary>The caller's deadline passed.</summary>
    DeadlineExceeded = 24,

    /// <summary>A concurrent write won; the caller retries.</summary>
    Aborted = 25,

    /// <summary>A bug in the server. The message and the request id go to the log.</summary>
    Internal = 26,
}

/// <summary>
/// The registry itself: the wire name of every <see cref="Reason"/>, the Connect
/// code each is raised under, and the reverse lookup.
/// </summary>
/// <remarks>
/// Three lookups, and they answer different questions. <see cref="FromWire"/>
/// answers "what did the server say" and returns <see langword="null"/> for a
/// reason this SDK does not have, which the runtime surfaces as
/// <c>UnknownReason</c> rather than dropping. <see cref="Name"/> answers "what
/// do I write in a log" and returns the empty string for
/// <see cref="Reason.None"/>, because a failure with no reason has no reason to
/// print. <see cref="CodeOf"/> answers "what class is it", which is what the
/// retry policy and the error hierarchy branch on.
/// </remarks>
public static class ReasonRegistry
{
    /// <summary>
    /// The registry in the registry's own order, which is the order of the table
    /// in <c>docs/api/reasons.md</c>: the AP0-specific causes first, then the
    /// generic code-to-class rows. Kept as one ordered array rather than derived
    /// from a dictionary, because "the registry's order" is a property
    /// <c>All</c> promises and a hash map does not have one.
    /// </summary>
    private static readonly Reason[] InRegistryOrder =
    [
        Reason.ApprovalExpired,
        Reason.ApprovalAlreadyDecided,
        Reason.ApprovalStaleRevision,
        Reason.RequesterCannotApprove,
        Reason.DecisionProofInvalid,
        Reason.StepUpRequired,
        Reason.ReasonRequired,
        Reason.InvalidDecision,
        Reason.PairingExpired,
        Reason.PairingUsed,
        Reason.DeviceRevoked,
        Reason.PushTargetUnknown,
        Reason.NotImplemented,
        Reason.FeatureNotInVariant,
        Reason.InvalidArgument,
        Reason.NotFound,
        Reason.AlreadyExists,
        Reason.PermissionDenied,
        Reason.TokenExpired,
        Reason.Unauthenticated,
        Reason.FailedPrecondition,
        Reason.ResourceExhausted,
        Reason.Unavailable,
        Reason.DeadlineExceeded,
        Reason.Aborted,
        Reason.Internal,
    ];

    private static readonly FrozenDictionary<Reason, string> Names =
        new Dictionary<Reason, string>
        {
            [Reason.ApprovalExpired] = "approval_expired",
            [Reason.ApprovalAlreadyDecided] = "approval_already_decided",
            [Reason.ApprovalStaleRevision] = "approval_stale_revision",
            [Reason.RequesterCannotApprove] = "requester_cannot_approve",
            [Reason.DecisionProofInvalid] = "decision_proof_invalid",
            [Reason.StepUpRequired] = "step_up_required",
            [Reason.ReasonRequired] = "reason_required",
            [Reason.InvalidDecision] = "invalid_decision",
            [Reason.PairingExpired] = "pairing_expired",
            [Reason.PairingUsed] = "pairing_used",
            [Reason.DeviceRevoked] = "device_revoked",
            [Reason.PushTargetUnknown] = "push_target_unknown",
            [Reason.NotImplemented] = "not_implemented",
            [Reason.FeatureNotInVariant] = "feature_not_in_variant",
            [Reason.InvalidArgument] = "invalid_argument",
            [Reason.NotFound] = "not_found",
            [Reason.AlreadyExists] = "already_exists",
            [Reason.PermissionDenied] = "permission_denied",
            [Reason.TokenExpired] = "token_expired",
            [Reason.Unauthenticated] = "unauthenticated",
            [Reason.FailedPrecondition] = "failed_precondition",
            [Reason.ResourceExhausted] = "resource_exhausted",
            [Reason.Unavailable] = "unavailable",
            [Reason.DeadlineExceeded] = "deadline_exceeded",
            [Reason.Aborted] = "aborted",
            [Reason.Internal] = "internal",
        }.ToFrozenDictionary();

    /// <summary>
    /// The reverse lookup, built by hand rather than by a key-selector projection.
    /// </summary>
    /// <remarks>
    /// The projection overload reads as the obvious thing and does not compile
    /// without naming three type arguments, because a
    /// <c>FrozenDictionary</c> is an <c>IEnumerable&lt;KeyValuePair&gt;</c> rather
    /// than the dictionary the overload was written for. A loop is shorter than the
    /// type arguments, and it runs once at type-initialisation.
    /// </remarks>
    private static readonly FrozenDictionary<string, Reason> ByWire = BuildByWire();

    private static FrozenDictionary<string, Reason> BuildByWire()
    {
        var byWire = new Dictionary<string, Reason>(Names.Count, StringComparer.Ordinal);
        foreach (var (reason, wire) in Names)
        {
            byWire[wire] = reason;
        }
        return byWire.ToFrozenDictionary(StringComparer.Ordinal);
    }

    private static readonly FrozenDictionary<Reason, Code> Codes =
        new Dictionary<Reason, Code>
        {
            // The rows above `invalid_argument` in docs/api/reasons.md, which
            // are AP0's specific causes. The rows below it are the generic
            // code-to-class rows and map to the code named in their own name.
            [Reason.ApprovalExpired] = Code.FailedPrecondition,
            [Reason.ApprovalAlreadyDecided] = Code.FailedPrecondition,
            [Reason.ApprovalStaleRevision] = Code.FailedPrecondition,
            [Reason.RequesterCannotApprove] = Code.PermissionDenied,
            [Reason.DecisionProofInvalid] = Code.PermissionDenied,
            [Reason.StepUpRequired] = Code.Unauthenticated,
            [Reason.ReasonRequired] = Code.InvalidArgument,
            [Reason.InvalidDecision] = Code.InvalidArgument,
            [Reason.PairingExpired] = Code.FailedPrecondition,
            [Reason.PairingUsed] = Code.FailedPrecondition,
            [Reason.DeviceRevoked] = Code.Unauthenticated,
            [Reason.PushTargetUnknown] = Code.NotFound,
            [Reason.NotImplemented] = Code.Unimplemented,
            [Reason.FeatureNotInVariant] = Code.Unimplemented,
            [Reason.InvalidArgument] = Code.InvalidArgument,
            [Reason.NotFound] = Code.NotFound,
            [Reason.AlreadyExists] = Code.AlreadyExists,
            [Reason.PermissionDenied] = Code.PermissionDenied,
            [Reason.TokenExpired] = Code.Unauthenticated,
            [Reason.Unauthenticated] = Code.Unauthenticated,
            [Reason.FailedPrecondition] = Code.FailedPrecondition,
            [Reason.ResourceExhausted] = Code.ResourceExhausted,
            [Reason.Unavailable] = Code.Unavailable,
            [Reason.DeadlineExceeded] = Code.DeadlineExceeded,
            [Reason.Aborted] = Code.Aborted,
            [Reason.Internal] = Code.Internal,
        }.ToFrozenDictionary();

    /// <summary>Every reason in the registry, in the registry's order.</summary>
    public static IReadOnlyList<Reason> All { get; } = InRegistryOrder;

    /// <summary>Every registry reason's wire name, in the registry's order.</summary>
    public static IReadOnlyList<string> AllNames { get; } =
        InRegistryOrder.Select(Name).ToArray();

    /// <summary>The stable <c>snake_case</c> wire name, or the empty string for
    /// <see cref="Reason.None"/>.</summary>
    public static string Name(Reason reason) =>
        Names.TryGetValue(reason, out var wire) ? wire : string.Empty;

    /// <summary>
    /// The reason a wire string names, or <see langword="null"/> for one this
    /// SDK's registry does not have — which means the server is newer, and the
    /// caller must be told rather than handed a default.
    /// </summary>
    public static Reason? FromWire(string wire) =>
        ByWire.TryGetValue(wire, out var reason) ? reason : null;

    /// <summary>Whether a wire string is in this SDK's registry.</summary>
    public static bool IsKnown(string wire) => ByWire.ContainsKey(wire);

    /// <summary>The Connect code a reason is raised under, from the same registry.</summary>
    public static Code CodeOf(Reason reason) =>
        Codes.TryGetValue(reason, out var code) ? code : Code.Unknown;
}