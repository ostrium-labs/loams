// The module surface (design §44 §7.3, D606) and the reason registry it hangs
// off — here, the binding table.
//
// # Provenance — read this before editing
//
// `facade/Modules.g.cs`, `facade/Bindings.g.cs` and `Reason.cs` are the
// **Q604 hand-written-facade fallback**, which design §44 §7.3 explicitly allows:
//
//     "If the plugin proves too costly for a language, that language falls back
//     to a hand-written facade checked by the same conformance suite (D606,
//     Q604)."
//
// The C# renderer does not exist in `crates/loams-facade-gen`, which ships
// `typescript.rs` only, and this file's header names the annotations it was
// transcribed from. It is a transcription of the `loams.options.v1` options in
// `proto/`, field for field, so the runtime behind it is surface-agnostic: when
// the renderer lands, these three files are deleted and
// `scripts/sdk/gen.sh csharp` writes replacements with the same exported names.
// **Do not grow it.** If a call is missing, the proto is missing the
// `loams.options.v1.facade` annotation — see Q604.
//
// Nothing here holds an RPC path in a method signature or a retry class: each
// module method resolves its binding by name and hands the invoker a request and
// a response factory. That is why the hand-written fallback cannot drift from
// the annotations while it exists, and why adding a call is a proto edit.
//
// The **stubs** the invoker serializes are real and are checked in
// (`src/Loams/Gen/`, from `protoc-gen-csharp` over `proto/`, D604): this SDK is
// generated from the protos, not a REST wrapper.

using System.Collections.Frozen;

namespace Loams;

/// <summary>The <c>idempotency_level</c> off the proto method, which is what the retry class is derived from.</summary>
public enum IdempotencyLevel
{
    /// <summary>No level declared: a mutation, which may only be retried when it carries an idempotency key.</summary>
    None = 0,

    /// <summary><c>IDEMPOTENT</c>: repeating it is the same call.</summary>
    Idempotent = 1,

    /// <summary><c>NO_SIDE_EFFECTS</c>: a read, retryable on its own.</summary>
    NoSideEffects = 2,
}

/// <summary>Whether the SDK may retry a call on its own.</summary>
public enum RetryClass
{
    /// <summary>A mutation: retried only once it carries an idempotency key.</summary>
    Manual = 0,

    /// <summary>A read or an idempotent RPC: the SDK retries it.</summary>
    Safe = 1,
}

/// <summary>Whether a call answers with one message or with a stream.</summary>
public enum Streaming
{
    /// <summary>One request, one response.</summary>
    Unary = 0,

    /// <summary>
    /// A server stream. There is no client streaming and no bidi (D420): a
    /// browser cannot do it over <c>fetch</c>, and half-duplex works through
    /// every proxy.
    /// </summary>
    Server = 1,
}

/// <summary>
/// The two fields a paged call pages on, as <c>FacadeOptions.pagination</c>
/// names them ("<c>items:next_page_token</c>").
/// </summary>
/// <remarks>
/// The names are the **generated C# property names**, because that is what the
/// iterator sets and reads: <c>Collections</c>, <c>NextPageToken</c>,
/// <c>PageSize</c>, <c>PageToken</c>. <b>No annotated RPC is paged yet</b> —
/// <c>ListCollections</c> arrives with API1 Task 2 — so this is exercised
/// against a stub in <c>csharp_pagination_iterator</c> rather than end to end.
/// </remarks>
/// <param name="ItemsField">The response's repeated field, in PascalCase.</param>
/// <param name="NextPageTokenField">The response's token field, in PascalCase.</param>
/// <param name="PageSizeField">The request's <c>page_size</c> field, in PascalCase.</param>
/// <param name="PageTokenField">The request's token field, in PascalCase.</param>
public sealed record Pagination(
    string ItemsField,
    string NextPageTokenField,
    string PageSizeField = "PageSize",
    string PageTokenField = "PageToken");

/// <summary>
/// One facade call, as the runtime dispatches it: everything a module method
/// hands the invoker and nothing the invoker has to guess.
/// </summary>
/// <param name="Module">The SDK module, in snake_case, as §44 §7.1 spells it for a C# module.</param>
/// <param name="Name">The method name, in PascalCase (§44 §7.1).</param>
/// <param name="ProtoName">The name <c>FacadeOptions</c> gave it, verbatim.</param>
/// <param name="Rpc">The full RPC path, <c>package.Service/Method</c>.</param>
/// <param name="Service">The fully qualified service name.</param>
/// <param name="Package">The proto package, which is the module catalogue's key.</param>
/// <param name="Idempotency">The method's <c>idempotency_level</c>.</param>
/// <param name="Retry">The class derived from <paramref name="Idempotency"/> or from <c>FacadeOptions.retry_safe</c>.</param>
/// <param name="Streaming">Unary or server.</param>
/// <param name="Pagination">The two fields it pages on, or null when it does not page.</param>
/// <param name="RequestTypeName">The generated request type's full name, for the dynamic path.</param>
/// <param name="ResponseTypeName">The generated response type's full name, for the dynamic path.</param>
public sealed record CallBinding(
    string Module,
    string Name,
    string ProtoName,
    string Rpc,
    string Service,
    string Package,
    IdempotencyLevel Idempotency,
    RetryClass Retry,
    Streaming Streaming,
    Pagination? Pagination,
    string RequestTypeName,
    string ResponseTypeName)
{
    /// <summary>
    /// Whether the request's **schema** declares <c>idempotency_key</c>.
    /// </summary>
    /// <remarks>
    /// Asked of the generated descriptor rather than of a property, because a
    /// proto3 <c>optional string idempotency_key</c> is a nullable C# property and a
    /// request that leaves it out sends no key at all — in which case the mutation
    /// is not retryable and the SDK must know that, or it would hand out a promise
    /// the server cannot keep. The same question
    /// <see cref="Idempotency.Apply"/> asks at the moment it decides.
    /// </remarks>
    public bool TakesIdempotencyKey => Loams.Idempotency.SchemaHasKey(RequestTypeName);
}

/// <summary>One module and the calls it exposes.</summary>
/// <param name="Name">The module name, in snake_case.</param>
/// <param name="Summary">The one-line description from <c>ModuleOptions.summary</c>.</param>
/// <param name="Package">The proto package it binds.</param>
/// <param name="Service">The service, or null for a module that spans two.</param>
/// <param name="Unstable">
/// Whether the package's wire contract may still change (R1's rule), so a caller
/// can feature-detect rather than depend on it.
/// </param>
public sealed record ModuleBinding(
    string Name,
    string Summary,
    string Package,
    string? Service,
    bool Unstable);

/// <summary>
/// The module catalogue, the binding table and the reason registry: the whole
/// generated surface, in one place, so the runtime has a single thing to read.
/// </summary>
/// <remarks>
/// Hand-written under Q604; see the file header. The tables are
/// <c>FrozenDictionary</c> because they are read on every call and written never,
/// and a lookup that has to take a lock to answer "is this a bound call" would be
/// a surprising cost in the one path every request takes.
/// </remarks>
public static class Facade
{
    /// <summary>
    /// The proto revision this SDK was generated from (<c>LOAMS_PROTO_REV</c>,
    /// design §44 §10.3). Pre-1.0, so it is the API major rather than a release
    /// tag; <c>client.System.VersionAsync</c> checks it against the server's
    /// <c>GetInstance.ApiVersions</c>.
    /// </summary>
    public const string ProtoRev = "v1";

    /// <summary>Every proto package in the module, as <c>GetInstance</c> names them.</summary>
    public static IReadOnlyList<string> ProtoPackages { get; } =
    [
        "loams.instance.v1",
        "loams.live.v1",
        "loams.approvals.v1",
        "loams.devices.v1",
        "loams.notifications.v1",
        "loams.operations.v1",
        "loams.errors.v1",
        "loams.options.v1",
    ];

    /// <summary>The module catalogue, in the order §44 §7.2 lists them.</summary>
    public static IReadOnlyList<ModuleBinding> Modules { get; } =
    [
        new ModuleBinding("instance", "What this instance is, and who the caller is on it.",
            "loams.instance.v1", "loams.instance.v1.InstanceService", Unstable: false),
        new ModuleBinding("live", "The live sync session half.", "loams.live.v1",
            "loams.live.v1.LiveService", Unstable: true),
        new ModuleBinding("tables", "The table half of loams.live.v1.", "loams.live.v1",
            "loams.live.v1.LiveService", Unstable: true),
        new ModuleBinding("approvals", "The approval promises destructive operations wait on.",
            "loams.approvals.v1", "loams.approvals.v1.ApprovalService", Unstable: false),
        new ModuleBinding("devices", "Paired devices and their notification preferences.",
            "loams.devices.v1", "loams.devices.v1.DeviceService", Unstable: false),
        new ModuleBinding("notifications", "The caller's notification inbox.",
            "loams.notifications.v1", "loams.notifications.v1.NotificationService", Unstable: false),
        new ModuleBinding("operations", "Running and finished operations.",
            "loams.operations.v1", "loams.operations.v1.OperationsService", Unstable: false),
    ];

    /// <summary>
    /// Every bound call, keyed by <c>(module, name)</c>. See the file header: this
    /// is transcribed from <c>loams.options.v1</c> and is replaced by the
    /// generator's output when the C# renderer lands.
    /// </summary>
    public static IReadOnlyDictionary<(string Module, string Name), CallBinding> Bindings { get; } =
        BindingsByKey().ToFrozenDictionary();

    /// <summary>The binding a module and call name identify.</summary>
    /// <exception cref="KeyNotFoundException">
    /// There is no such call. Naming the pair in the message is the point: the
    /// caller who mistyped a method name gets the pair back, not a stack trace.
    /// </exception>
    public static CallBinding Binding(string module, string name) =>
        Bindings.TryGetValue((module, name), out var binding)
            ? binding
            : throw new KeyNotFoundException(
                $"loams.{module} has no bound call {name}; the bound calls are " +
                string.Join(", ", Bindings.Keys.Where((key) => key.Module == module)
                    .Select((key) => key.Name).Order(StringComparer.Ordinal)));

    /// <summary>The binding an RPC path identifies, or null when nothing binds it.</summary>
    public static CallBinding? BindingForRpc(string rpc) => ByRpc.GetValueOrDefault(rpc);

    private static readonly Dictionary<string, CallBinding> ByRpc =
        Bindings.Values.ToDictionary((binding) => binding.Rpc, StringComparer.Ordinal);

    private static Dictionary<(string Module, string Name), CallBinding> BindingsByKey()
    {
        var table = new Dictionary<(string, string), CallBinding>();

        // Each row is one `rpc` line and one `option` line of a proto, transcribed
        // field for field. `idempotency_level` becomes the retry class by the rule
        // D610 states — a read or an idempotent RPC retries on its own, a mutation
        // does not — so a row's retry class is derived here rather than written
        // down, which is what keeps a binding from claiming a mutation is safe.
        void Add(string module, string name, string protoName, string service, string method,
            IdempotencyLevel idempotency, Streaming streaming, string requestType, string responseType,
            Pagination? pagination = null, RetryClass? retry = null)
        {
            var package = service[..service.LastIndexOf('.')];
            var @class = retry ?? (idempotency == IdempotencyLevel.None ? RetryClass.Manual : RetryClass.Safe);
            var binding = new CallBinding(module, name, protoName, $"{service}/{method}", service, package,
                idempotency, @class, streaming, pagination, requestType, responseType);
            table[(module, name)] = binding;
        }

        // ---- loams.approvals.v1.ApprovalService (proto/loams/approvals/v1/approvals.proto) ----

        Add("approvals", "ListApprovals", "listApprovals", "loams.approvals.v1.ApprovalService",
            "ListApprovals", IdempotencyLevel.NoSideEffects, Streaming.Unary,
            "Loams.Approvals.V1.ListApprovalsRequest", "Loams.Approvals.V1.ListApprovalsResponse");
        Add("approvals", "GetApproval", "getApproval", "loams.approvals.v1.ApprovalService",
            "GetApproval", IdempotencyLevel.NoSideEffects, Streaming.Unary,
            "Loams.Approvals.V1.GetApprovalRequest", "Loams.Approvals.V1.GetApprovalResponse");
        Add("approvals", "WatchApprovals", "watchApprovals", "loams.approvals.v1.ApprovalService",
            "WatchApprovals", IdempotencyLevel.None, Streaming.Server,
            "Loams.Approvals.V1.WatchApprovalsRequest", "Loams.Approvals.V1.WatchApprovalsResponse");
        Add("approvals", "DecideApproval", "decideApproval", "loams.approvals.v1.ApprovalService",
            "DecideApproval", IdempotencyLevel.None, Streaming.Unary,
            "Loams.Approvals.V1.DecideApprovalRequest", "Loams.Approvals.V1.DecideApprovalResponse");

        // ---- loams.devices.v1.DeviceService (proto/loams/devices/v1/devices.proto) ----

        Add("devices", "CreatePairing", "createPairing", "loams.devices.v1.DeviceService",
            "CreatePairing", IdempotencyLevel.None, Streaming.Unary,
            "Loams.Devices.V1.CreatePairingRequest", "Loams.Devices.V1.CreatePairingResponse");
        Add("devices", "ListDevices", "listDevices", "loams.devices.v1.DeviceService",
            "ListDevices", IdempotencyLevel.NoSideEffects, Streaming.Unary,
            "Loams.Devices.V1.ListDevicesRequest", "Loams.Devices.V1.ListDevicesResponse");
        Add("devices", "RenameDevice", "renameDevice", "loams.devices.v1.DeviceService",
            "RenameDevice", IdempotencyLevel.None, Streaming.Unary,
            "Loams.Devices.V1.RenameDeviceRequest", "Loams.Devices.V1.RenameDeviceResponse");
        Add("devices", "RevokeDevice", "revokeDevice", "loams.devices.v1.DeviceService",
            "RevokeDevice", IdempotencyLevel.None, Streaming.Unary,
            "Loams.Devices.V1.RevokeDeviceRequest", "Loams.Devices.V1.RevokeDeviceResponse");
        Add("devices", "RegisterPushTarget", "registerPushTarget", "loams.devices.v1.DeviceService",
            "RegisterPushTarget", IdempotencyLevel.None, Streaming.Unary,
            "Loams.Devices.V1.RegisterPushTargetRequest", "Loams.Devices.V1.RegisterPushTargetResponse");
        Add("devices", "UnregisterPushTarget", "unregisterPushTarget", "loams.devices.v1.DeviceService",
            "UnregisterPushTarget", IdempotencyLevel.None, Streaming.Unary,
            "Loams.Devices.V1.UnregisterPushTargetRequest", "Loams.Devices.V1.UnregisterPushTargetResponse");
        Add("devices", "GetNotificationPreferences", "getNotificationPreferences",
            "loams.devices.v1.DeviceService", "GetNotificationPreferences",
            IdempotencyLevel.NoSideEffects, Streaming.Unary,
            "Loams.Devices.V1.GetNotificationPreferencesRequest",
            "Loams.Devices.V1.GetNotificationPreferencesResponse");
        Add("devices", "SetNotificationPreferences", "setNotificationPreferences",
            "loams.devices.v1.DeviceService", "SetNotificationPreferences",
            IdempotencyLevel.None, Streaming.Unary,
            "Loams.Devices.V1.SetNotificationPreferencesRequest",
            "Loams.Devices.V1.SetNotificationPreferencesResponse");
        // The corpus's `mock_error_encodings` and `mock_error_not_implemented` are
        // this call: a declared RPC whose handler is a stub, answered
        // `unimplemented` with reason `not_implemented`, in all four encodings.
        Add("devices", "SendTestNotification", "sendTestNotification", "loams.devices.v1.DeviceService",
            "SendTestNotification", IdempotencyLevel.None, Streaming.Unary,
            "Loams.Devices.V1.SendTestNotificationRequest", "Loams.Devices.V1.SendTestNotificationResponse");

        // ---- loams.notifications.v1.NotificationService ----

        Add("notifications", "ListNotifications", "listNotifications",
            "loams.notifications.v1.NotificationService", "ListNotifications",
            IdempotencyLevel.NoSideEffects, Streaming.Unary,
            "Loams.Notifications.V1.ListNotificationsRequest", "Loams.Notifications.V1.ListNotificationsResponse");
        Add("notifications", "WatchNotifications", "watchNotifications",
            "loams.notifications.v1.NotificationService", "WatchNotifications",
            IdempotencyLevel.None, Streaming.Server,
            "Loams.Notifications.V1.WatchNotificationsRequest",
            "Loams.Notifications.V1.WatchNotificationsResponse");
        Add("notifications", "MarkRead", "markRead", "loams.notifications.v1.NotificationService",
            "MarkRead", IdempotencyLevel.None, Streaming.Unary,
            "Loams.Notifications.V1.MarkReadRequest", "Loams.Notifications.V1.MarkReadResponse");

        // ---- loams.operations.v1.OperationsService ----

        Add("operations", "GetOperation", "getOperation", "loams.operations.v1.OperationsService",
            "GetOperation", IdempotencyLevel.NoSideEffects, Streaming.Unary,
            "Loams.Operations.V1.GetOperationRequest", "Loams.Operations.V1.GetOperationResponse");
        Add("operations", "ListOperations", "listOperations", "loams.operations.v1.OperationsService",
            "ListOperations", IdempotencyLevel.NoSideEffects, Streaming.Unary,
            "Loams.Operations.V1.ListOperationsRequest", "Loams.Operations.V1.ListOperationsResponse");
        Add("operations", "WatchOperations", "watchOperations", "loams.operations.v1.OperationsService",
            "WatchOperations", IdempotencyLevel.None, Streaming.Server,
            "Loams.Operations.V1.WatchOperationsRequest", "Loams.Operations.V1.WatchOperationsResponse");
        Add("operations", "CancelOperation", "cancelOperation", "loams.operations.v1.OperationsService",
            "CancelOperation", IdempotencyLevel.None, Streaming.Unary,
            "Loams.Operations.V1.CancelOperationRequest", "Loams.Operations.V1.CancelOperationResponse");

        // loams.instance.v1.InstanceService (proto/loams/instance/v1/instance.proto).
        Add("instance", "GetInstance", "getInstance", "loams.instance.v1.InstanceService", "GetInstance",
            IdempotencyLevel.NoSideEffects, Streaming.Unary,
            "Loams.Instance.V1.GetInstanceRequest", "Loams.Instance.V1.GetInstanceResponse");
        Add("instance", "WhoAmI", "whoAmI", "loams.instance.v1.InstanceService", "WhoAmI",
            IdempotencyLevel.NoSideEffects, Streaming.Unary,
            "Loams.Instance.V1.WhoAmIRequest", "Loams.Instance.V1.WhoAmIResponse");

        // loams.live.v1.LiveService (proto/loams/live/v1/live.proto). `live` wraps
        // Watch and ModifyQuerySet; `tables` wraps Query, Mutate and Deploy, per
        // `FacadeOptions.module` on the proto and design §44 §7.2.
        Add("live", "Watch", "watch", "loams.live.v1.LiveService", "Watch",
            IdempotencyLevel.None, Streaming.Server,
            "Loams.Live.V1.WatchRequest", "Loams.Live.V1.Transition");
        Add("live", "ModifyQuerySet", "modifyQuerySet", "loams.live.v1.LiveService", "ModifyQuerySet",
            IdempotencyLevel.None, Streaming.Unary,
            "Loams.Live.V1.ModifyQuerySetRequest", "Loams.Live.V1.ModifyQuerySetResponse");
        Add("tables", "Query", "query", "loams.live.v1.LiveService", "Query",
            IdempotencyLevel.None, Streaming.Unary,
            "Loams.Live.V1.QueryRequest", "Loams.Live.V1.QueryResponse");
        Add("tables", "Mutate", "mutate", "loams.live.v1.LiveService", "Mutate",
            IdempotencyLevel.None, Streaming.Unary,
            "Loams.Live.V1.MutateRequest", "Loams.Live.V1.MutateResponse");
        Add("tables", "Deploy", "deploy", "loams.live.v1.LiveService", "Deploy",
            IdempotencyLevel.None, Streaming.Unary,
            "Loams.Live.V1.DeployRequest", "Loams.Live.V1.DeployResponse");

        return table;
    }
}