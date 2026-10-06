// The module methods: one per bound call, three to five lines each.
//
// These are the hand-written half of design §44 §7.3's Q604 fallback, and they are
// the SDK's **public surface**: a caller uses `client.Instance.GetInstanceAsync`,
// never the generated stub. Deleted when the C# renderer lands, at which point
// `scripts/sdk/gen.sh csharp` writes them with the same names and signatures.
//
// Nothing here holds an RPC path or a retry class. Each method resolves its
// binding by name from `Facade` and hands the invoker the request, which is what
// makes the hand-written fallback unable to drift from the `loams.options.v1`
// annotations while it exists: the annotations are the only place a path is
// written down, and these methods only name them.
//
// # `CancellationToken` last, always
//
// Every method takes it, defaults it, and threads it. A method without one cannot
// be cancelled, and a caller's deadline has to cover the whole call — see
// `CallInvoker`. It is the last parameter so a caller who does not care can leave it
// off, and so the optional `CallOptions` is never mistaken for it.

using Loams.Approvals.V1;
using Loams.Devices.V1;
using Loams.Instance.V1;
using Loams.Live.V1;
using Loams.Notifications.V1;
using Loams.Operations.V1;

namespace Loams;

/// <summary>
/// <c>loams.instance</c> — what this instance is, and who the caller is on it
/// (design §44 §7.2).
/// </summary>
public sealed class InstanceModule
{
    private readonly CallInvoker _invoker;

    internal InstanceModule(CallInvoker invoker) => _invoker = invoker;

    /// <summary>
    /// What this instance is and how to sign in to it. Needs no credentials, which is
    /// why it is the first thing any client calls.
    /// </summary>
    /// <remarks>
    /// The R9 contract: the version check must work before a caller has a token. A
    /// client with a stale credential still gets an answer here, so
    /// <c>client.System.VersionAsync</c> can be the first call a program makes.
    /// </remarks>
    public Task<GetInstanceResponse> GetInstanceAsync(
        GetInstanceRequest? request = null,
        CallOptions? options = null,
        CancellationToken cancellationToken = default) =>
        _invoker.UnaryAsync<GetInstanceResponse>(Facade.Binding("instance", "GetInstance"),
            request ?? new GetInstanceRequest(), options, cancellationToken);

    /// <summary>
    /// The calling principal, its org and the environments it can reach.
    /// </summary>
    /// <remarks>
    /// The corpus's four <c>instance_who_am_i_*</c> cases are this call on a build
    /// with no authentication yet: <c>unimplemented</c> with reason
    /// <c>not_implemented</c>. An SDK branches on the reason, never on the message.
    /// </remarks>
    public Task<WhoAmIResponse> WhoAmIAsync(
        WhoAmIRequest? request = null,
        CallOptions? options = null,
        CancellationToken cancellationToken = default) =>
        _invoker.UnaryAsync<WhoAmIResponse>(Facade.Binding("instance", "WhoAmI"),
            request ?? new WhoAmIRequest(), options, cancellationToken);
}

/// <summary>
/// <c>loams.live</c> — the live sync session half. Its package is unstable, so its
/// wire contract may still change (§44 §10.3, R1).
/// </summary>
public sealed class LiveModule
{
    private readonly CallInvoker _invoker;

    internal LiveModule(CallInvoker invoker) => _invoker = invoker;

    /// <summary>
    /// The transitions of a query set, as an async sequence.
    /// </summary>
    /// <remarks>
    /// A server stream is an <see cref="IAsyncEnumerable{T}"/>, which is C#'s native
    /// async iteration (§44 §7.1) and needs no wrapper:
    ///
    ///     await foreach (var transition in client.Live.WatchAsync(request))
    ///     {
    ///         Apply(transition);
    ///     }
    ///
    /// **The refusal arrives inside the Connect envelope, not as an HTTP status.**
    /// The corpus's <c>live_watch</c> is exactly this — a 200 whose end-of-stream
    /// frame carries <c>unimplemented</c> — so a client reading only status codes
    /// sees a success and then nothing. Pass <c>options</c> with a
    /// <see cref="CallOptions.Resume"/> to reconnect from the cursor across a node
    /// restart (R7).
    /// </remarks>
    public IAsyncEnumerable<Transition> WatchAsync(
        WatchRequest request,
        CallOptions? options = null,
        CancellationToken cancellationToken = default) =>
        _invoker.ServerStreamAsync<Transition>(Facade.Binding("live", "Watch"), request, options,
            cancellationToken);

    /// <summary>Changes what a query set watches.</summary>
    public Task<ModifyQuerySetResponse> ModifyQuerySetAsync(
        ModifyQuerySetRequest request,
        CallOptions? options = null,
        CancellationToken cancellationToken = default) =>
        _invoker.UnaryAsync<ModifyQuerySetResponse>(Facade.Binding("live", "ModifyQuerySet"), request, options, cancellationToken);
}

/// <summary>
/// <c>loams.tables</c> — the table half of <c>loams.live.v1</c> (design §44 §7.2).
/// </summary>
public sealed class TablesModule
{
    private readonly CallInvoker _invoker;

    internal TablesModule(CallInvoker invoker) => _invoker = invoker;

    /// <summary>
    /// The rows of a query.
    /// </summary>
    /// <remarks>
    /// The corpus's four <c>live_query_*</c> cases are this call on a build whose
    /// engine is not in the variant: <c>unimplemented</c> with reason
    /// <c>feature_not_in_variant</c> and the variant in the metadata. That is R5's
    /// second half — <c>client.System.GuardAsync("tables")</c> costs no RPC and
    /// raises the same type.
    /// </remarks>
    public Task<QueryResponse> QueryAsync(
        QueryRequest request,
        CallOptions? options = null,
        CancellationToken cancellationToken = default) =>
        _invoker.UnaryAsync<QueryResponse>(Facade.Binding("tables", "Query"), request, options, cancellationToken);

    /// <summary>Writes rows.</summary>
    /// <remarks>
    /// A mutation: <c>idempotency_key</c> is <c>optional</c> in the proto, so a
    /// caller who leaves it out sends none and the call is not retryable — which the
    /// SDK reads from the schema and honours, rather than guessing from the object.
    /// Leave it out and the SDK mints a UUIDv7 per logical call and reuses it on
    /// every retry (R3).
    /// </remarks>
    public Task<MutateResponse> MutateAsync(
        MutateRequest request,
        CallOptions? options = null,
        CancellationToken cancellationToken = default) =>
        _invoker.UnaryAsync<MutateResponse>(Facade.Binding("tables", "Mutate"), request, options, cancellationToken);

    /// <summary>Deploys a change.</summary>
    /// <remarks>
    /// <c>DeployRequest</c> declares <b>no</b> <c>idempotency_key</c>, so this call is
    /// never keyed and never auto-retried: keying it would invent a field the schema
    /// does not have, which the server rejects.
    /// </remarks>
    public Task<DeployResponse> DeployAsync(
        DeployRequest request,
        CallOptions? options = null,
        CancellationToken cancellationToken = default) =>
        _invoker.UnaryAsync<DeployResponse>(Facade.Binding("tables", "Deploy"), request, options, cancellationToken);
}

/// <summary>
/// <c>loams.approvals</c> — the approval promises destructive operations wait on
/// (design §44 §7.2, AP0).
/// </summary>
public sealed class ApprovalsModule
{
    private readonly CallInvoker _invoker;

    internal ApprovalsModule(CallInvoker invoker) => _invoker = invoker;

    /// <summary>The approvals the caller may see.</summary>
    public Task<ListApprovalsResponse> ListApprovalsAsync(
        ListApprovalsRequest request,
        CallOptions? options = null,
        CancellationToken cancellationToken = default) =>
        _invoker.UnaryAsync<ListApprovalsResponse>(Facade.Binding("approvals", "ListApprovals"), request, options, cancellationToken);

    /// <summary>One approval by id.</summary>
    public Task<GetApprovalResponse> GetApprovalAsync(
        GetApprovalRequest request,
        CallOptions? options = null,
        CancellationToken cancellationToken = default) =>
        _invoker.UnaryAsync<GetApprovalResponse>(Facade.Binding("approvals", "GetApproval"), request, options, cancellationToken);

    /// <summary>Settles an approval promise, once, as one write (R3).</summary>
    /// <remarks>
    /// The corpus's `mock_state_idempotent_decide` is this call sent twice with the
    /// same key: the approval state really moved between them, both answers are
    /// identical bytes and it is at revision 2. A client that regenerates the key per
    /// attempt turns that into a second write, which is the failure the field exists
    /// to prevent.
    /// </remarks>
    public Task<DecideApprovalResponse> DecideApprovalAsync(
        DecideApprovalRequest request,
        CallOptions? options = null,
        CancellationToken cancellationToken = default) =>
        _invoker.UnaryAsync<DecideApprovalResponse>(Facade.Binding("approvals", "DecideApproval"), request, options, cancellationToken);

    /// <summary>
    /// The approval stream: a snapshot, then changes, then a heartbeat every 15 s
    /// (AP0 Ruling 3).
    /// </summary>
    /// <remarks>
    /// **Heartbeats are liveness, not data**, and this sequence does not yield them —
    /// a stream that yields heartbeats to a UI renders an empty row every fifteen
    /// seconds. That is the whole of `mock_state_stream_heartbeat`, which records two
    /// frames and one message.
    ///
    /// Every event case reaches the caller: <c>snapshot</c>, <c>upsert</c>,
    /// <c>remove</c> and <c>heartbeat</c>. An SDK that handles only <c>upsert</c>
    /// leaves a decided approval on screen forever, which R7 calls worse than an
    /// error — <c>mock_state_stream_resume_remove</c> is the recording of exactly
    /// that frame.
    /// </remarks>
    public IAsyncEnumerable<WatchApprovalsResponse> WatchApprovalsAsync(
        WatchApprovalsRequest request,
        CallOptions? options = null,
        CancellationToken cancellationToken = default) =>
        _invoker.ServerStreamAsync<WatchApprovalsResponse>(Facade.Binding("approvals", "WatchApprovals"),
            request, options, cancellationToken);
}

/// <summary>
/// <c>loams.devices</c> — paired devices and their notification preferences
/// (design §44 §7.2, AP0).
/// </summary>
public sealed class DevicesModule
{
    private readonly CallInvoker _invoker;

    internal DevicesModule(CallInvoker invoker) => _invoker = invoker;

    /// <summary>The caller's paired devices.</summary>
    /// <remarks>
    /// Marked <c>NO_SIDE_EFFECTS</c> in the proto, so it is also the one app RPC an
    /// SDK should issue as a cacheable GET (design §44 §4).
    /// </remarks>
    public Task<ListDevicesResponse> ListDevicesAsync(
        ListDevicesRequest request,
        CallOptions? options = null,
        CancellationToken cancellationToken = default) =>
        _invoker.UnaryAsync<ListDevicesResponse>(Facade.Binding("devices", "ListDevices"), request, options, cancellationToken);
}

/// <summary>
/// <c>loams.notifications</c> — the caller's notification inbox (design §44 §7.2,
/// AP0).
/// </summary>
public sealed class NotificationsModule
{
    private readonly CallInvoker _invoker;

    internal NotificationsModule(CallInvoker invoker) => _invoker = invoker;

    /// <summary>The inbox.</summary>
    public Task<ListNotificationsResponse> ListNotificationsAsync(
        ListNotificationsRequest request,
        CallOptions? options = null,
        CancellationToken cancellationToken = default) =>
        _invoker.UnaryAsync<ListNotificationsResponse>(Facade.Binding("notifications", "ListNotifications"), request, options,
            cancellationToken);

    /// <summary>The inbox, as a stream of CloudEvents.</summary>
    public IAsyncEnumerable<WatchNotificationsResponse> WatchNotificationsAsync(
        WatchNotificationsRequest request,
        CallOptions? options = null,
        CancellationToken cancellationToken = default) =>
        _invoker.ServerStreamAsync<WatchNotificationsResponse>(
            Facade.Binding("notifications", "WatchNotifications"), request, options, cancellationToken);

    /// <summary>Marks notifications read.</summary>
    public Task<MarkReadResponse> MarkReadAsync(
        MarkReadRequest request,
        CallOptions? options = null,
        CancellationToken cancellationToken = default) =>
        _invoker.UnaryAsync<MarkReadResponse>(Facade.Binding("notifications", "MarkRead"), request, options, cancellationToken);
}

/// <summary>
/// <c>loams.operations</c> — running and finished operations (design §44 §7.2).
/// </summary>
public sealed class OperationsModule
{
    private readonly CallInvoker _invoker;

    internal OperationsModule(CallInvoker invoker) => _invoker = invoker;

    /// <summary>One operation by id.</summary>
    public Task<GetOperationResponse> GetOperationAsync(
        GetOperationRequest request,
        CallOptions? options = null,
        CancellationToken cancellationToken = default) =>
        _invoker.UnaryAsync<GetOperationResponse>(Facade.Binding("operations", "GetOperation"), request, options, cancellationToken);

    /// <summary>The operations the caller can see.</summary>
    public Task<ListOperationsResponse> ListOperationsAsync(
        ListOperationsRequest request,
        CallOptions? options = null,
        CancellationToken cancellationToken = default) =>
        _invoker.UnaryAsync<ListOperationsResponse>(Facade.Binding("operations", "ListOperations"), request, options,
            cancellationToken);

    /// <summary>The operations, as a stream.</summary>
    public IAsyncEnumerable<WatchOperationsResponse> WatchOperationsAsync(
        WatchOperationsRequest request,
        CallOptions? options = null,
        CancellationToken cancellationToken = default) =>
        _invoker.ServerStreamAsync<WatchOperationsResponse>(
            Facade.Binding("operations", "WatchOperations"), request, options, cancellationToken);

    /// <summary>Cancels a running operation.</summary>
    public Task<CancelOperationResponse> CancelOperationAsync(
        CancelOperationRequest request,
        CallOptions? options = null,
        CancellationToken cancellationToken = default) =>
        _invoker.UnaryAsync<CancelOperationResponse>(Facade.Binding("operations", "CancelOperation"), request, options,
            cancellationToken);
}