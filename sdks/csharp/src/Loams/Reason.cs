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

    /// <summary>A <c>loams.house.v1</c> call on a server with no House front configured.</summary>
    HouseNotConfigured = 27,

    /// <summary>A House statement that does not parse, or is outside the House surface.</summary>
    HouseSyntaxError = 28,

    /// <summary>The table a House statement names does not exist.</summary>
    HouseUnknownTable = 29,

    /// <summary>The database a House statement names does not exist.</summary>
    HouseUnknownDatabase = 30,

    /// <summary>The caller may not run this House statement or query operation.</summary>
    HouseAccessDenied = 31,

    /// <summary>A read-only House statement tried to write or change state.</summary>
    HouseReadOnly = 32,

    /// <summary>The House statement ran past its timeout.</summary>
    HouseTimeout = 33,

    /// <summary>The House statement exceeded its memory limit.</summary>
    HouseMemoryLimit = 34,

    /// <summary>No House worker was free within the acquire timeout.</summary>
    HouseTooManyQueries = 35,

    /// <summary>The House statement is past a namespace quota.</summary>
    HouseQuotaExceeded = 36,

    /// <summary>The statement uses a function, engine or table the House deny list refuses.</summary>
    HouseDisabledFunction = 37,

    /// <summary>The House statement was cancelled.</summary>
    HouseCancelled = 38,

    /// <summary>The House query the call names was not found.</summary>
    HouseQueryNotFound = 39,

    /// <summary>The House engine raised an error with no more specific reason.</summary>
    HouseQueryFailed = 40,

    /// <summary>The project a <c>loams.postgres.v1</c> call names does not exist.</summary>
    ProjectNotFound = 41,

    /// <summary>The branch has child branches, so it cannot be deleted.</summary>
    BranchHasChildren = 42,

    /// <summary>The branch is protected and cannot be deleted or restored over.</summary>
    BranchProtected = 43,

    /// <summary>The requested LSN is older than the history retention.</summary>
    LsnOutOfRetention = 44,

    /// <summary>The branch already has a read-write endpoint.</summary>
    EndpointExistsForBranch = 45,

    /// <summary>The compute could not be started.</summary>
    ComputeStartFailed = 46,

    /// <summary>A create or resize would go past a quota.</summary>
    QuotaExceeded = 47,

    /// <summary>The storage layer the call needs is down.</summary>
    StorageUnavailable = 48,

    /// <summary>The secret was already issued for this idempotency key and is answered once.</summary>
    SecretAlreadyIssued = 49,

    /// <summary>A Live function's <c>paginate</c> got a malformed, altered or foreign cursor.</summary>
    LiveBadCursor = 50,

    /// <summary>The isolated worker running a Live function crashed; the call never committed.</summary>
    LiveWorkerCrashed = 51,

    /// <summary>Another Live deploy activated first, so <c>expected_active</c> no longer matches.</summary>
    LiveDeployConflict = 52,

    /// <summary>A write did not match its Live table's validator; the message names the table and field paths.</summary>
    LiveSchemaViolation = 53,
    /// <summary>A document RPC set both `filter` and `where`; set one.</summary>
    FilterConflict = 125,
    /// <summary>search.query_all needs a sort that ends in the primary key.</summary>
    SearchAfterNeedsTiebreak = 129,
    /// <summary>A consistency token is not the v1: encoding.</summary>
    InvalidConsistencyToken = 128,
    /// <summary>An app request has no namespace and the client has no default.</summary>
    NamespaceRequired = 127,
    /// <summary>The number of SQL parameters does not match the statement.</summary>
    SqlParamCount = 126,

    /// <summary>A Live function read an index a deploy is still backfilling.</summary>
    LiveIndexNotReady = 54,

    /// <summary>The named Loams Git repository does not exist.</summary>
    RepoNotFound = 55,

    /// <summary>A Loams Git repository of that name already exists.</summary>
    RepoAlreadyExists = 56,

    /// <summary>The repository was deleted and is waiting to be purged.</summary>
    RepoDeleting = 57,

    /// <summary>A ref write's expected old value no longer holds.</summary>
    RefStale = 58,

    /// <summary>A protection rule refuses the ref write.</summary>
    RefProtected = 59,

    /// <summary>The write is outside the credential's branch prefixes or cones.</summary>
    OutOfScope = 60,

    /// <summary>The repository's object format is not supported (SHA-1 only for now).</summary>
    ObjectFormatUnsupported = 61,

    /// <summary>The source repository is already as many forks deep as allowed.</summary>
    ForkTooDeep = 62,

    /// <summary>The repository's sequencer is shutting down, lost its lease, or could not open.</summary>
    SequencerUnavailable = 63,

    /// <summary>The idempotency key was used inside its window for a different request.</summary>
    IdempotencyKeyReused = 64,

    /// <summary>The named workspace does not exist.</summary>
    WorkspaceNotFound = 65,

    /// <summary>Another writer appended the workspace snapshot first.</summary>
    WorkspaceFenced = 66,
    /// <summary>A batch write stopped at a failed chunk; earlier chunks stay applied.</summary>
    BatchPartiallyApplied = 132,
    UnsupportedPgVersion = 133,
    BetaMajorRequiresOptIn = 134,
    ComputeMajorMismatchRefused = 135,
    ImportChecksumsEnabled = 136,

    /// <summary>The workspace or snapshot is read-only.</summary>
    WorkspaceReadOnly = 67,

    /// <summary>The owner already has a workspace of that name.</summary>
    WorkspaceNameTaken = 68,

    /// <summary>A snapshot or push names an object the repository does not have.</summary>
    ObjectMissing = 69,

    /// <summary>The leak scanner found a credential outside <c>env/</c>.</summary>
    SecretDetected = 70,

    /// <summary>An <c>env/</c> file holds a plaintext value.</summary>
    EnvUnsealed = 71,

    /// <summary>The credential has no grant for the environment.</summary>
    EnvAccessDenied = 72,

    /// <summary>A sealed <c>env/</c> reference names another repository's secret.</summary>
    EnvForeignReference = 73,

    /// <summary>The caller's org role is below the one the admin RPC needs, or the caller is not in the org.</summary>
    RoleRequired = 74,

    /// <summary>First-run setup has not created the org yet.</summary>
    SetupRequired = 75,

    /// <summary>The call carried no credential.</summary>
    TokenMissing = 76,

    /// <summary>The credential is not an API key, a session token or a JWT, or more than one Authorization header was sent.</summary>
    TokenMalformed = 77,

    /// <summary>The API key was revoked, or the console session was signed out or its person left the org.</summary>
    TokenRevoked = 78,

    /// <summary>The OIDC token's nbf is in the future.</summary>
    TokenNotYetValid = 79,

    /// <summary>The OIDC token's signature does not verify.</summary>
    TokenBadSignature = 80,

    /// <summary>The OIDC token is for another audience.</summary>
    TokenWrongAudience = 81,

    /// <summary>The OIDC token is from an issuer this engine does not trust.</summary>
    TokenWrongIssuer = 82,

    /// <summary>The OIDC token is signed with a key its issuer's JWKS does not have.</summary>
    TokenUnknownKey = 83,

    /// <summary>The OIDC token is signed with an algorithm other than RS256 or EdDSA.</summary>
    TokenAlgorithmNotAllowed = 84,

    /// <summary>No key file or metastore record admits the API key.</summary>
    ApiKeyInvalid = 85,

    /// <summary>This engine's auth mode does not take this kind of credential.</summary>
    CredentialNotAccepted = 86,

    /// <summary>A credential was sent in the query string.</summary>
    CredentialInQueryString = 87,

    /// <summary>A cookie-authenticated call that is not a GET lacked the session's CSRF token.</summary>
    CsrfTokenInvalid = 88,

    /// <summary>A valid OIDC or tailnet identity that is not on the engine's allowlist.</summary>
    PrincipalNotAllowed = 89,

    /// <summary>A browser call from an origin the engine's CORS allowlist does not have.</summary>
    CorsOriginNotAllowed = 90,

    /// <summary>The credential cannot be checked now: the issuer's JWKS or the principal store is unreachable.</summary>
    AuthUnavailable = 91,

    /// <summary>No join token with that id, or its secret does not match.</summary>
    JoinTokenUnknown = 137,

    /// <summary>The join token was already used; join tokens work once.</summary>
    JoinTokenSpent = 138,

    /// <summary>The join token is past its hour.</summary>
    JoinTokenExpired = 139,

    /// <summary>The CSR asks for an identity the join token or the current certificate does not hold.</summary>
    IdentityNotGranted = 140,

    /// <summary>The certificate, or an identity asked for, is on the CA's deny list.</summary>
    IdentityRevoked = 141,

    /// <summary>The deployment has no CA yet, or this node cannot open its intermediate key.</summary>
    CaNotInitialized = 142,

    /// <summary>A call to a Live app that is disabled or deleted.</summary>
    LiveAppDisabled = 92,

    /// <summary>CreateApp named a Live app that already exists.</summary>
    LiveAppExists = 93,

    /// <summary>A deployed Live function threw.</summary>
    LiveFunctionError = 94,

    /// <summary>A Live function ran past its CPU limit.</summary>
    LiveFunctionTimeout = 95,
    /// <summary>The client's own deadline for the call ran out; never retried.</summary>
    ClientDeadlineExceeded = 131,

    /// <summary>A Live function ran past its memory limit.</summary>
    LiveFunctionOutOfMemory = 96,

    /// <summary>A Live call exceeded one of the app's limits.</summary>
    LiveLimitExceeded = 97,
    /// <summary>A SQL answer is over the Arrow size cap; use Flight.</summary>
    ResultTooLarge = 130,

    /// <summary>A credential reached a Live app outside its environment.</summary>
    LiveAppMismatch = 98,

    /// <summary>A browser origin the addressed Live app does not allow.</summary>
    LiveOriginNotAllowed = 99,

    /// <summary>A Live session reached its maximum age; resume it.</summary>
    SessionRotate = 100,

    /// <summary>No Live node holds the session any more; resume it.</summary>
    SessionGone = 101,

    /// <summary>The Live node is shutting down; resume on another node.</summary>
    NodeDraining = 102,

    /// <summary>The named Loams Memory store does not exist.</summary>
    MemoryStoreNotFound = 103,

    /// <summary>The named memory space does not exist.</summary>
    MemorySpaceNotFound = 104,

    /// <summary>The store has as many spaces as it may.</summary>
    MemorySpaceLimit = 105,

    /// <summary>The object is managed by Loams Memory and refuses direct writes.</summary>
    MemoryManagedObject = 106,

    /// <summary>Another write changed the block first; re-read and merge.</summary>
    MemoryBlockVersionMismatch = 107,

    /// <summary>The block's value or limit is over its cap.</summary>
    MemoryBlockTooLarge = 108,

    /// <summary>The space's core memory would pass its cap.</summary>
    MemoryCoreBudgetExceeded = 109,

    /// <summary>The content of an add is over 1 MiB.</summary>
    MemoryContentTooLarge = 110,

    /// <summary>The content holds a secret and the store refuses secrets.</summary>
    MemorySecretDetected = 111,

    /// <summary>The content holds personal data and the store refuses it.</summary>
    MemoryPiiRefused = 112,

    /// <summary>The call needs a model the store has not configured.</summary>
    MemoryModelNotConfigured = 113,

    /// <summary>The store is local-only and the model host is not local.</summary>
    MemoryModelNotLocal = 114,

    /// <summary>The store's model token budget is spent.</summary>
    MemoryTokenBudgetExhausted = 115,

    /// <summary>Extracting facts from the episode failed; the episode is kept.</summary>
    MemoryExtractionFailed = 116,

    /// <summary>A read named more than 8 spaces.</summary>
    MemoryTooManySpaces = 117,

    /// <summary>A write would touch more than one space.</summary>
    MemoryCrossSpaceWrite = 118,

    /// <summary>The Loams Memory RPC is not served by this release yet.</summary>
    MemoryNotYetAvailable = 119,

    /// <summary>The repository has no code index.</summary>
    CodeRepoNotIndexed = 120,

    /// <summary>The repository's code index is still being built.</summary>
    CodeIndexBuilding = 121,

    /// <summary>The change has more files than a code query takes.</summary>
    CodeChangeTooLarge = 122,

    /// <summary>The file's language has no parser or resolver.</summary>
    CodeLanguageUnsupported = 123,

    /// <summary>The uploaded SCIP index does not parse or does not match.</summary>
    CodeScipInvalid = 124,

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
        Reason.HouseNotConfigured,
        Reason.HouseSyntaxError,
        Reason.HouseUnknownTable,
        Reason.HouseUnknownDatabase,
        Reason.HouseAccessDenied,
        Reason.HouseReadOnly,
        Reason.HouseTimeout,
        Reason.HouseMemoryLimit,
        Reason.HouseTooManyQueries,
        Reason.HouseQuotaExceeded,
        Reason.HouseDisabledFunction,
        Reason.HouseCancelled,
        Reason.HouseQueryNotFound,
        Reason.HouseQueryFailed,
        Reason.ProjectNotFound,
        Reason.BranchHasChildren,
        Reason.BranchProtected,
        Reason.LsnOutOfRetention,
        Reason.EndpointExistsForBranch,
        Reason.ComputeStartFailed,
        Reason.UnsupportedPgVersion,
        Reason.BetaMajorRequiresOptIn,
        Reason.ComputeMajorMismatchRefused,
        Reason.ImportChecksumsEnabled,
        Reason.QuotaExceeded,
        Reason.StorageUnavailable,
        Reason.SecretAlreadyIssued,
        Reason.RepoNotFound,
        Reason.RepoAlreadyExists,
        Reason.RepoDeleting,
        Reason.RefStale,
        Reason.RefProtected,
        Reason.OutOfScope,
        Reason.ObjectFormatUnsupported,
        Reason.ForkTooDeep,
        Reason.SequencerUnavailable,
        Reason.IdempotencyKeyReused,
        Reason.WorkspaceNotFound,
        Reason.WorkspaceFenced,
        Reason.BatchPartiallyApplied,
        Reason.WorkspaceReadOnly,
        Reason.WorkspaceNameTaken,
        Reason.ObjectMissing,
        Reason.SecretDetected,
        Reason.EnvUnsealed,
        Reason.EnvAccessDenied,
        Reason.EnvForeignReference,
        Reason.MemoryStoreNotFound,
        Reason.MemorySpaceNotFound,
        Reason.MemorySpaceLimit,
        Reason.MemoryManagedObject,
        Reason.MemoryBlockVersionMismatch,
        Reason.MemoryBlockTooLarge,
        Reason.MemoryCoreBudgetExceeded,
        Reason.MemoryContentTooLarge,
        Reason.MemorySecretDetected,
        Reason.MemoryPiiRefused,
        Reason.MemoryModelNotConfigured,
        Reason.MemoryModelNotLocal,
        Reason.MemoryTokenBudgetExhausted,
        Reason.MemoryExtractionFailed,
        Reason.MemoryTooManySpaces,
        Reason.MemoryCrossSpaceWrite,
        Reason.MemoryNotYetAvailable,
        Reason.CodeRepoNotIndexed,
        Reason.CodeIndexBuilding,
        Reason.CodeChangeTooLarge,
        Reason.CodeLanguageUnsupported,
        Reason.CodeScipInvalid,
        Reason.LiveBadCursor,
        Reason.LiveWorkerCrashed,
        Reason.LiveDeployConflict,
        Reason.LiveSchemaViolation,
        Reason.FilterConflict,
        Reason.SearchAfterNeedsTiebreak,
        Reason.InvalidConsistencyToken,
        Reason.NamespaceRequired,
        Reason.SqlParamCount,
        Reason.LiveIndexNotReady,
        Reason.LiveAppDisabled,
        Reason.LiveAppExists,
        Reason.LiveFunctionError,
        Reason.LiveFunctionTimeout,
        Reason.ClientDeadlineExceeded,
        Reason.LiveFunctionOutOfMemory,
        Reason.LiveLimitExceeded,
        Reason.ResultTooLarge,
        Reason.LiveAppMismatch,
        Reason.LiveOriginNotAllowed,
        Reason.SessionRotate,
        Reason.SessionGone,
        Reason.NodeDraining,
        Reason.RoleRequired,
        Reason.SetupRequired,
        Reason.TokenMissing,
        Reason.TokenMalformed,
        Reason.TokenRevoked,
        Reason.TokenNotYetValid,
        Reason.TokenBadSignature,
        Reason.TokenWrongAudience,
        Reason.TokenWrongIssuer,
        Reason.TokenUnknownKey,
        Reason.TokenAlgorithmNotAllowed,
        Reason.ApiKeyInvalid,
        Reason.CredentialNotAccepted,
        Reason.CredentialInQueryString,
        Reason.CsrfTokenInvalid,
        Reason.PrincipalNotAllowed,
        Reason.CorsOriginNotAllowed,
        Reason.AuthUnavailable,
        Reason.JoinTokenUnknown,
        Reason.JoinTokenSpent,
        Reason.JoinTokenExpired,
        Reason.IdentityNotGranted,
        Reason.IdentityRevoked,
        Reason.CaNotInitialized,
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
            [Reason.HouseNotConfigured] = "house_not_configured",
            [Reason.HouseSyntaxError] = "house_syntax_error",
            [Reason.HouseUnknownTable] = "house_unknown_table",
            [Reason.HouseUnknownDatabase] = "house_unknown_database",
            [Reason.HouseAccessDenied] = "house_access_denied",
            [Reason.HouseReadOnly] = "house_read_only",
            [Reason.HouseTimeout] = "house_timeout",
            [Reason.HouseMemoryLimit] = "house_memory_limit",
            [Reason.HouseTooManyQueries] = "house_too_many_queries",
            [Reason.HouseQuotaExceeded] = "house_quota_exceeded",
            [Reason.HouseDisabledFunction] = "house_disabled_function",
            [Reason.HouseCancelled] = "house_cancelled",
            [Reason.HouseQueryNotFound] = "house_query_not_found",
            [Reason.HouseQueryFailed] = "house_query_failed",
            [Reason.ProjectNotFound] = "project_not_found",
            [Reason.BranchHasChildren] = "branch_has_children",
            [Reason.BranchProtected] = "branch_protected",
            [Reason.LsnOutOfRetention] = "lsn_out_of_retention",
            [Reason.EndpointExistsForBranch] = "endpoint_exists_for_branch",
            [Reason.ComputeStartFailed] = "compute_start_failed",
            [Reason.UnsupportedPgVersion] = "unsupported_pg_version",
            [Reason.BetaMajorRequiresOptIn] = "beta_major_requires_opt_in",
            [Reason.ComputeMajorMismatchRefused] = "compute_major_mismatch_refused",
            [Reason.ImportChecksumsEnabled] = "import_checksums_enabled",
            [Reason.QuotaExceeded] = "quota_exceeded",
            [Reason.StorageUnavailable] = "storage_unavailable",
            [Reason.SecretAlreadyIssued] = "secret_already_issued",
            [Reason.RepoNotFound] = "repo_not_found",
            [Reason.RepoAlreadyExists] = "repo_already_exists",
            [Reason.RepoDeleting] = "repo_deleting",
            [Reason.RefStale] = "ref_stale",
            [Reason.RefProtected] = "ref_protected",
            [Reason.OutOfScope] = "out_of_scope",
            [Reason.ObjectFormatUnsupported] = "object_format_unsupported",
            [Reason.ForkTooDeep] = "fork_too_deep",
            [Reason.SequencerUnavailable] = "sequencer_unavailable",
            [Reason.IdempotencyKeyReused] = "idempotency_key_reused",
            [Reason.WorkspaceNotFound] = "workspace_not_found",
            [Reason.WorkspaceFenced] = "workspace_fenced",
            [Reason.BatchPartiallyApplied] = "batch_partially_applied",
            [Reason.WorkspaceReadOnly] = "workspace_read_only",
            [Reason.WorkspaceNameTaken] = "workspace_name_taken",
            [Reason.ObjectMissing] = "object_missing",
            [Reason.SecretDetected] = "secret_detected",
            [Reason.EnvUnsealed] = "env_unsealed",
            [Reason.EnvAccessDenied] = "env_access_denied",
            [Reason.EnvForeignReference] = "env_foreign_reference",
            [Reason.MemoryStoreNotFound] = "memory_store_not_found",
            [Reason.MemorySpaceNotFound] = "memory_space_not_found",
            [Reason.MemorySpaceLimit] = "memory_space_limit",
            [Reason.MemoryManagedObject] = "memory_managed_object",
            [Reason.MemoryBlockVersionMismatch] = "memory_block_version_mismatch",
            [Reason.MemoryBlockTooLarge] = "memory_block_too_large",
            [Reason.MemoryCoreBudgetExceeded] = "memory_core_budget_exceeded",
            [Reason.MemoryContentTooLarge] = "memory_content_too_large",
            [Reason.MemorySecretDetected] = "memory_secret_detected",
            [Reason.MemoryPiiRefused] = "memory_pii_refused",
            [Reason.MemoryModelNotConfigured] = "memory_model_not_configured",
            [Reason.MemoryModelNotLocal] = "memory_model_not_local",
            [Reason.MemoryTokenBudgetExhausted] = "memory_token_budget_exhausted",
            [Reason.MemoryExtractionFailed] = "memory_extraction_failed",
            [Reason.MemoryTooManySpaces] = "memory_too_many_spaces",
            [Reason.MemoryCrossSpaceWrite] = "memory_cross_space_write",
            [Reason.MemoryNotYetAvailable] = "memory_not_yet_available",
            [Reason.CodeRepoNotIndexed] = "code_repo_not_indexed",
            [Reason.CodeIndexBuilding] = "code_index_building",
            [Reason.CodeChangeTooLarge] = "code_change_too_large",
            [Reason.CodeLanguageUnsupported] = "code_language_unsupported",
            [Reason.CodeScipInvalid] = "code_scip_invalid",
            [Reason.LiveBadCursor] = "live_bad_cursor",
            [Reason.LiveWorkerCrashed] = "live_worker_crashed",
            [Reason.LiveDeployConflict] = "live_deploy_conflict",
            [Reason.LiveSchemaViolation] = "live_schema_violation",
            [Reason.FilterConflict] = "filter_conflict",
            [Reason.SearchAfterNeedsTiebreak] = "search_after_needs_tiebreak",
            [Reason.InvalidConsistencyToken] = "invalid_consistency_token",
            [Reason.NamespaceRequired] = "namespace_required",
            [Reason.SqlParamCount] = "sql_param_count",
            [Reason.LiveIndexNotReady] = "live_index_not_ready",
            [Reason.LiveAppDisabled] = "live_app_disabled",
            [Reason.LiveAppExists] = "live_app_exists",
            [Reason.LiveFunctionError] = "live_function_error",
            [Reason.LiveFunctionTimeout] = "live_function_timeout",
            [Reason.ClientDeadlineExceeded] = "client_deadline_exceeded",
            [Reason.LiveFunctionOutOfMemory] = "live_function_out_of_memory",
            [Reason.LiveLimitExceeded] = "live_limit_exceeded",
            [Reason.ResultTooLarge] = "result_too_large",
            [Reason.LiveAppMismatch] = "live_app_mismatch",
            [Reason.LiveOriginNotAllowed] = "live_origin_not_allowed",
            [Reason.SessionRotate] = "session_rotate",
            [Reason.SessionGone] = "session_gone",
            [Reason.NodeDraining] = "node_draining",
            [Reason.RoleRequired] = "role_required",
            [Reason.SetupRequired] = "setup_required",
            [Reason.TokenMissing] = "token_missing",
            [Reason.TokenMalformed] = "token_malformed",
            [Reason.TokenRevoked] = "token_revoked",
            [Reason.TokenNotYetValid] = "token_not_yet_valid",
            [Reason.TokenBadSignature] = "token_bad_signature",
            [Reason.TokenWrongAudience] = "token_wrong_audience",
            [Reason.TokenWrongIssuer] = "token_wrong_issuer",
            [Reason.TokenUnknownKey] = "token_unknown_key",
            [Reason.TokenAlgorithmNotAllowed] = "token_algorithm_not_allowed",
            [Reason.ApiKeyInvalid] = "api_key_invalid",
            [Reason.CredentialNotAccepted] = "credential_not_accepted",
            [Reason.CredentialInQueryString] = "credential_in_query_string",
            [Reason.CsrfTokenInvalid] = "csrf_token_invalid",
            [Reason.PrincipalNotAllowed] = "principal_not_allowed",
            [Reason.CorsOriginNotAllowed] = "cors_origin_not_allowed",
            [Reason.AuthUnavailable] = "auth_unavailable",
            [Reason.JoinTokenUnknown] = "join_token_unknown",
            [Reason.JoinTokenSpent] = "join_token_spent",
            [Reason.JoinTokenExpired] = "join_token_expired",
            [Reason.IdentityNotGranted] = "identity_not_granted",
            [Reason.IdentityRevoked] = "identity_revoked",
            [Reason.CaNotInitialized] = "ca_not_initialized",
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
            [Reason.HouseNotConfigured] = Code.Unimplemented,
            [Reason.HouseSyntaxError] = Code.InvalidArgument,
            [Reason.HouseUnknownTable] = Code.NotFound,
            [Reason.HouseUnknownDatabase] = Code.NotFound,
            [Reason.HouseAccessDenied] = Code.PermissionDenied,
            [Reason.HouseReadOnly] = Code.FailedPrecondition,
            [Reason.HouseTimeout] = Code.DeadlineExceeded,
            [Reason.HouseMemoryLimit] = Code.ResourceExhausted,
            [Reason.HouseTooManyQueries] = Code.ResourceExhausted,
            [Reason.HouseQuotaExceeded] = Code.ResourceExhausted,
            [Reason.HouseDisabledFunction] = Code.PermissionDenied,
            [Reason.HouseCancelled] = Code.Cancelled,
            [Reason.HouseQueryNotFound] = Code.NotFound,
            [Reason.HouseQueryFailed] = Code.Unknown,
            [Reason.ProjectNotFound] = Code.NotFound,
            [Reason.BranchHasChildren] = Code.FailedPrecondition,
            [Reason.BranchProtected] = Code.FailedPrecondition,
            [Reason.BetaMajorRequiresOptIn] = Code.FailedPrecondition,
            [Reason.ComputeMajorMismatchRefused] = Code.FailedPrecondition,
            [Reason.ImportChecksumsEnabled] = Code.FailedPrecondition,
            [Reason.LsnOutOfRetention] = Code.FailedPrecondition,
            [Reason.EndpointExistsForBranch] = Code.AlreadyExists,
            [Reason.ComputeStartFailed] = Code.Unavailable,
            [Reason.QuotaExceeded] = Code.ResourceExhausted,
            [Reason.StorageUnavailable] = Code.Unavailable,
            [Reason.SecretAlreadyIssued] = Code.FailedPrecondition,
            [Reason.RepoNotFound] = Code.NotFound,
            [Reason.RepoAlreadyExists] = Code.AlreadyExists,
            [Reason.RepoDeleting] = Code.FailedPrecondition,
            [Reason.RefStale] = Code.Aborted,
            [Reason.RefProtected] = Code.FailedPrecondition,
            [Reason.OutOfScope] = Code.PermissionDenied,
            [Reason.ObjectFormatUnsupported] = Code.Unimplemented,
            [Reason.ForkTooDeep] = Code.FailedPrecondition,
            [Reason.SequencerUnavailable] = Code.Unavailable,
            [Reason.IdempotencyKeyReused] = Code.FailedPrecondition,
            [Reason.WorkspaceNotFound] = Code.NotFound,
            [Reason.WorkspaceFenced] = Code.Aborted,
            [Reason.BatchPartiallyApplied] = Code.Aborted,
            [Reason.WorkspaceReadOnly] = Code.FailedPrecondition,
            [Reason.WorkspaceNameTaken] = Code.AlreadyExists,
            [Reason.ObjectMissing] = Code.FailedPrecondition,
            [Reason.SecretDetected] = Code.FailedPrecondition,
            [Reason.EnvUnsealed] = Code.InvalidArgument,
            [Reason.EnvAccessDenied] = Code.PermissionDenied,
            [Reason.EnvForeignReference] = Code.PermissionDenied,
            [Reason.MemoryStoreNotFound] = Code.NotFound,
            [Reason.MemorySpaceNotFound] = Code.NotFound,
            [Reason.MemorySpaceLimit] = Code.ResourceExhausted,
            [Reason.MemoryManagedObject] = Code.FailedPrecondition,
            [Reason.MemoryBlockVersionMismatch] = Code.Aborted,
            [Reason.MemoryBlockTooLarge] = Code.InvalidArgument,
            [Reason.MemoryCoreBudgetExceeded] = Code.ResourceExhausted,
            [Reason.MemoryContentTooLarge] = Code.InvalidArgument,
            [Reason.MemorySecretDetected] = Code.FailedPrecondition,
            [Reason.MemoryPiiRefused] = Code.FailedPrecondition,
            [Reason.MemoryModelNotConfigured] = Code.FailedPrecondition,
            [Reason.MemoryModelNotLocal] = Code.FailedPrecondition,
            [Reason.MemoryTokenBudgetExhausted] = Code.ResourceExhausted,
            [Reason.MemoryExtractionFailed] = Code.FailedPrecondition,
            [Reason.MemoryTooManySpaces] = Code.InvalidArgument,
            [Reason.MemoryCrossSpaceWrite] = Code.InvalidArgument,
            [Reason.MemoryNotYetAvailable] = Code.Unimplemented,
            [Reason.CodeRepoNotIndexed] = Code.FailedPrecondition,
            [Reason.CodeIndexBuilding] = Code.Unavailable,
            [Reason.CodeChangeTooLarge] = Code.InvalidArgument,
            [Reason.CodeLanguageUnsupported] = Code.FailedPrecondition,
            [Reason.CodeScipInvalid] = Code.InvalidArgument,
            [Reason.LiveBadCursor] = Code.InvalidArgument,
            [Reason.LiveWorkerCrashed] = Code.Internal,
            [Reason.LiveDeployConflict] = Code.FailedPrecondition,
            [Reason.LiveSchemaViolation] = Code.InvalidArgument,
            [Reason.FilterConflict] = Code.InvalidArgument,
            [Reason.SearchAfterNeedsTiebreak] = Code.InvalidArgument,
            [Reason.InvalidConsistencyToken] = Code.InvalidArgument,
            [Reason.NamespaceRequired] = Code.InvalidArgument,
            [Reason.SqlParamCount] = Code.InvalidArgument,
            [Reason.UnsupportedPgVersion] = Code.InvalidArgument,
            [Reason.LiveIndexNotReady] = Code.FailedPrecondition,
            [Reason.LiveAppDisabled] = Code.FailedPrecondition,
            [Reason.LiveAppExists] = Code.FailedPrecondition,
            [Reason.LiveFunctionError] = Code.Unknown,
            [Reason.LiveFunctionTimeout] = Code.DeadlineExceeded,
            [Reason.ClientDeadlineExceeded] = Code.DeadlineExceeded,
            [Reason.LiveFunctionOutOfMemory] = Code.ResourceExhausted,
            [Reason.LiveLimitExceeded] = Code.ResourceExhausted,
            [Reason.ResultTooLarge] = Code.ResourceExhausted,
            [Reason.LiveAppMismatch] = Code.PermissionDenied,
            [Reason.LiveOriginNotAllowed] = Code.PermissionDenied,
            [Reason.SessionRotate] = Code.Unavailable,
            [Reason.SessionGone] = Code.NotFound,
            [Reason.NodeDraining] = Code.Unavailable,
            [Reason.RoleRequired] = Code.PermissionDenied,
            [Reason.SetupRequired] = Code.FailedPrecondition,
            [Reason.TokenMissing] = Code.Unauthenticated,
            [Reason.TokenMalformed] = Code.Unauthenticated,
            [Reason.TokenRevoked] = Code.Unauthenticated,
            [Reason.TokenNotYetValid] = Code.Unauthenticated,
            [Reason.TokenBadSignature] = Code.Unauthenticated,
            [Reason.TokenWrongAudience] = Code.Unauthenticated,
            [Reason.TokenWrongIssuer] = Code.Unauthenticated,
            [Reason.TokenUnknownKey] = Code.Unauthenticated,
            [Reason.TokenAlgorithmNotAllowed] = Code.Unauthenticated,
            [Reason.ApiKeyInvalid] = Code.Unauthenticated,
            [Reason.CredentialNotAccepted] = Code.Unauthenticated,
            [Reason.CredentialInQueryString] = Code.Unauthenticated,
            [Reason.CsrfTokenInvalid] = Code.PermissionDenied,
            [Reason.PrincipalNotAllowed] = Code.PermissionDenied,
            [Reason.CorsOriginNotAllowed] = Code.PermissionDenied,
            [Reason.AuthUnavailable] = Code.Unavailable,
            [Reason.JoinTokenUnknown] = Code.Unauthenticated,
            [Reason.JoinTokenSpent] = Code.Unauthenticated,
            [Reason.JoinTokenExpired] = Code.Unauthenticated,
            [Reason.IdentityNotGranted] = Code.PermissionDenied,
            [Reason.IdentityRevoked] = Code.PermissionDenied,
            [Reason.CaNotInitialized] = Code.FailedPrecondition,
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