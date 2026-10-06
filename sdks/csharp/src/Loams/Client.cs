// The `Loams` object: one client with namespaced modules (design §44 §7.1).
//
//     await using var client = new LoamsClient(new LoamsClientOptions
//     {
//         Endpoint = Environment.GetEnvironmentVariable("LOAMS_ENDPOINT")!,
//         Auth = new EnvTokenSource(),
//     });
//     var info = await client.Instance.GetInstanceAsync(new GetInstanceRequest());
//
// What is generated and what is hand-written, once more, because it decides where
// a change goes. The **module surface** is generated from the
// `loams.options.v1` annotations on the protos and lives in `Facade.cs` and
// `Modules.cs`; the **runtime** behind those methods is hand-written, once, in
// this package: transport, credentials, retry, errors, tokens, pagination,
// streams. This file is the thin join — it builds the invoker, wires the runtime,
// and holds the client-wide defaults. It contains no RPC path and no retry class,
// which is why annotating a proto is enough to add an SDK method.
//
// # An empty endpoint is a usage error
//
// `LoamsClientOptions.Endpoint` is `required` and validated here rather than
// defaulted to localhost. A client that quietly talks to the wrong instance is
// worse than one that does not start, and "I forgot to set the endpoint" is the
// mistake that default-to-localhost hides until it writes to somebody's data.

using System.Collections.Frozen;

namespace Loams;

/// <summary>How to build a <see cref="LoamsClient"/>.</summary>
public sealed record LoamsClientOptions
{
    /// <summary>
    /// The instance's base URL, for example <c>https://acme.loams.dev</c>. A
    /// loopback stack is <c>http://127.0.0.1:8080</c>. Required, and validated: an
    /// empty endpoint is a usage error rather than a request to localhost.
    /// </summary>
    public required string Endpoint { get; init; }

    /// <summary>
    /// The bearer source: an <see cref="ApiKeyTokenSource"/> for a script or a CI
    /// job, a <see cref="ScriptedTokenSource"/>, a <see cref="DelegateTokenSource"/>
    /// for a desktop app that signs in after the client was built, or any
    /// <see cref="ITokenSource"/>.
    /// </summary>
    /// <remarks>
    /// Omitted means an unauthenticated client, which is what
    /// <c>GetInstance</c> needs anyway and what <c>mock_status_get_instance</c> is
    /// recorded as: the R9 version check must work before a caller has a token.
    /// </remarks>
    public ITokenSource? Auth { get; init; }

    /// <summary>The wire protocol. Connect unless a caller has a reason.</summary>
    public Protocol Protocol { get; init; } = Protocol.Connect;

    /// <summary>The message encoding. Binary protobuf unless a caller asks for JSON.</summary>
    public Codec Codec { get; init; } = Codec.Proto;

    /// <summary>
    /// The <c>HttpClient</c> the transport uses. It wins over the other transport
    /// knobs, so a caller with its own TLS configuration, proxy or timeout supplies
    /// it here.
    /// </summary>
    public HttpClient? HttpClient { get; init; }

    /// <summary>
    /// A handler the transport wraps, for a test or an authenticating proxy.
    /// Ignored when <see cref="HttpClient"/> is supplied.
    /// </summary>
    public HttpMessageHandler? Handler { get; init; }

    /// <summary>Headers added to every request, for a gateway or a proxy.</summary>
    public IReadOnlyDictionary<string, string> Headers { get; init; } =
        FrozenDictionary<string, string>.Empty;

    /// <summary>
    /// The retries after the first attempt, for every call.
    /// </summary>
    /// <remarks>
    /// <b>Zero means <see cref="RetryPolicy.DefaultMaxRetries"/> (3), not none.</b>
    /// The design's number is the client's number: an SDK that silently did not retry
    /// unless it was configured to would be an SDK whose safe-by-default behaviour is
    /// a sharp edge, and a caller writing <c>new LoamsClientOptions { Endpoint = … }</c>
    /// would get something other from every other SDK. C#'s <c>default(int)</c> is the
    /// problem, and it is solved by an explicit opt-out:
    ///
    ///     var client = new LoamsClient(new LoamsClientOptions { Endpoint = "…", NoRetries = true });
    ///
    /// A negative value is treated as the default rather than as none, because a
    /// negative budget is a mistake and "unlimited retries" is not a thing an SDK
    /// offers. <see cref="CallOptions.MaxRetries"/> on a **call** still means exactly
    /// what it says, including zero: writing the number there is an unambiguous
    /// choice, which is why the two fields behave differently.
    /// </remarks>
    public int MaxRetries { get; init; } = RetryPolicy.DefaultMaxRetries;

    /// <summary>Turns off automatic retries for the whole client.</summary>
    public bool NoRetries { get; init; }

    /// <summary>
    /// Holds a session consistency token across calls (D609).
    /// </summary>
    /// <remarks>
    /// <b>Off by default</b>: every read is then <c>STRONG</c> on its own, which is
    /// correct but does not give read-your-writes across processes. Turn it on when
    /// one process is both writing and reading, and remember it keeps the token it
    /// was given rather than merging — see <see cref="ConsistencySession"/>.
    /// </remarks>
    public bool SessionConsistency { get; init; }

    /// <summary>
    /// How long to wait for response headers, in seconds. The caller's
    /// <c>CancellationToken</c> is the deadline for the call itself.
    /// </summary>
    public TimeSpan ResponseHeadersTimeout { get; init; } = TimeSpan.FromSeconds(100);
}

/// <summary>
/// One SDK over one instance, with namespaced modules.
/// </summary>
/// <remarks>
/// It is safe for concurrent use: the transport, the token source and the session
/// store are each either immutable or guarded, and the only mutable state in the
/// client itself is the catalogue cache, which is behind its own lock. A client
/// <b>must</b> be reused rather than rebuilt per call — a client per call means a new
/// HTTP connection pool per call, and a new idempotency key per call for the same
/// logical work.
/// </remarks>
public sealed class LoamsClient : IDisposable, IAsyncDisposable
{
    private readonly Transport _transport;
    private readonly ConsistencySession? _session;
    private bool _disposed;

    /// <summary>Builds a client.</summary>
    /// <exception cref="ArgumentException">
    /// The endpoint is empty or whitespace. Naming it as a usage error rather than
    /// defaulting it is deliberate — see <see cref="LoamsClientOptions.Endpoint"/>.
    /// </exception>
    public LoamsClient(LoamsClientOptions options)
    {
        ArgumentNullException.ThrowIfNull(options);
        if (string.IsNullOrWhiteSpace(options.Endpoint))
        {
            throw new ArgumentException(
                "LoamsClientOptions.Endpoint is empty; it is the instance's base URL, for example " +
                "https://acme.loams.dev. It is deliberately not defaulted to localhost, because a client " +
                "that quietly talks to the wrong instance is worse than one that does not start.",
                nameof(options));
        }

        var endpoint = options.Endpoint.TrimEnd('/');

        _transport = new Transport(new TransportOptions
        {
            Endpoint = endpoint,
            Protocol = options.Protocol,
            Codec = options.Codec,
            HttpClient = options.HttpClient,
            Handler = options.Handler,
            DefaultHeaders = options.Headers,
            ResponseHeadersTimeout = options.ResponseHeadersTimeout,
        });

        if (options.SessionConsistency)
        {
            _session = new ConsistencySession();
        }

        var maxRetries = options.MaxRetries;
        if (maxRetries < 0)
        {
            maxRetries = RetryPolicy.DefaultMaxRetries;
        }
        if (maxRetries == 0 && !options.NoRetries)
        {
            // Zero from the default-valued field is "the caller did not say", and
            // the design's number is 3. See the Options doc comment.
            maxRetries = RetryPolicy.DefaultMaxRetries;
        }
        if (options.NoRetries)
        {
            maxRetries = 0;
        }

        Invoker = new CallInvoker(_transport, options.Auth, maxRetries, _session);

        Instance = new InstanceModule(Invoker);
        Live = new LiveModule(Invoker);
        Tables = new TablesModule(Invoker);
        Approvals = new ApprovalsModule(Invoker);
        Devices = new DevicesModule(Invoker);
        Notifications = new NotificationsModule(Invoker);
        Operations = new OperationsModule(Invoker);
        System = new LoamsSystem(Invoker);

        Modules = new Dictionary<string, object>(StringComparer.Ordinal)
        {
            ["instance"] = Instance,
            ["live"] = Live,
            ["tables"] = Tables,
            ["approvals"] = Approvals,
            ["devices"] = Devices,
            ["notifications"] = Notifications,
            ["operations"] = Operations,
        };
    }

    /// <summary>The call path, for a caller building their own client.</summary>
    public CallInvoker Invoker { get; }

    /// <summary><c>loams.instance</c> — what this instance is, and who the caller is.</summary>
    public InstanceModule Instance { get; }

    /// <summary>
    /// <c>loams.live</c> — the live sync session half. Its package is unstable, so
    /// its wire contract may still change (§44 §10.3).
    /// </summary>
    public LiveModule Live { get; }

    /// <summary><c>loams.tables</c> — the table half of <c>loams.live.v1</c>.</summary>
    public TablesModule Tables { get; }

    /// <summary><c>loams.approvals</c> — the approval promises destructive operations wait on.</summary>
    public ApprovalsModule Approvals { get; }

    /// <summary><c>loams.devices</c> — paired devices and their notification preferences.</summary>
    public DevicesModule Devices { get; }

    /// <summary><c>loams.notifications</c> — the caller's notification inbox.</summary>
    public NotificationsModule Notifications { get; }

    /// <summary><c>loams.operations</c> — running and finished operations.</summary>
    public OperationsModule Operations { get; }

    /// <summary>The module catalogue, feature detection and the version check.</summary>
    public LoamsSystem System { get; }

    /// <summary>
    /// Every module, by name: the catalogue a caller iterates when it wants to
    /// feature-detect without hard-coding a field.
    /// </summary>
    public IReadOnlyDictionary<string, object> Modules { get; }

    /// <summary>The proto revision this SDK declares (§44 §10.3).</summary>
    public string ProtoRev => Facade.ProtoRev;

    /// <summary>Every proto package in the module, as <c>GetInstance</c> names them.</summary>
    public IReadOnlyList<string> ProtoPackages => Facade.ProtoPackages;

    /// <summary>The binding table, as the SDK sees it.</summary>
    public IReadOnlyList<ModuleBinding> ModuleCatalogue => Facade.Modules;

    /// <summary>
    /// A module by name, so a caller can feature-detect without a hard-coded field:
    /// <c>client.Module("collections")</c> is false until API1 Task 2.
    /// </summary>
    public bool TryGetModule(string name, out object module) => Modules.TryGetValue(name, out module!);

    /// <summary>The binding a module and call name identify.</summary>
    public CallBinding Binding(string module, string call) => Facade.Binding(module, call);

    /// <summary>
    /// The session consistency token store, or <see langword="null"/> when
    /// <see cref="LoamsClientOptions.SessionConsistency"/> was off.
    /// </summary>
    /// <remarks>
    /// Null rather than an inert store, so "not on" and "on but empty" do not look
    /// the same to a caller that checks.
    /// </remarks>
    public ConsistencySession? Session => _session;

    /// <summary>The wire protocol this client speaks.</summary>
    public Protocol Protocol => _transport.Protocol;

    /// <summary>The codec this client sends.</summary>
    public Codec Codec => _transport.Codec;

    /// <summary>Forgets the cached service catalogue, so the next feature check calls again.</summary>
    public void InvalidateCatalogue() => System.InvalidateCatalogue();

    /// <summary>
    /// The items of a paged call (R6), with the binding resolved from a module and
    /// call name:
    ///
    ///     await foreach (var item in client.PaginateAsync("collections", "ListCollections",
    ///         (request, token, ct) => client.Collections.ListCollectionsAsync(request, ct),
    ///         request, response => response.Collections))
    ///     {
    ///         use(item);
    ///     }
    ///
    /// It is <c>Paginator.ItemsAsync</c> with the
    /// binding looked up, so a caller does not have to resolve one by hand.
    /// <c>collections</c> and <c>ListCollections</c> arrive with API1 Task 2; until
    /// then no generated call is paged and this throws naming the call. The runtime
    /// half is pinned by <c>csharp_pagination_iterator</c> against a stub.
    /// </summary>
    public IAsyncEnumerable<TItem> PaginateAsync<TRequest, TResponse, TItem>(
        string module,
        string call,
        PageFetcher<TRequest, TResponse> fetch,
        TRequest request,
        System.Func<TResponse, IReadOnlyList<TItem>> items,
        CancellationToken cancellationToken = default)
        where TRequest : class
        where TResponse : class =>
        Paginator.ItemsAsync(Binding(module, call), fetch, request, items, cancellationToken);

    /// <summary>Releases the transport's idle connections and its client, when it owns them.</summary>
    public void Dispose()
    {
        if (_disposed)
        {
            return;
        }
        _disposed = true;
        _transport.Dispose();
    }

    /// <summary>
    /// Releases the transport asynchronously, for a caller inside an
    /// <c>await using</c>.
    /// </summary>
    /// <remarks>
    /// <see cref="Dispose()"/> does the same work synchronously, so this is not a
    /// second implementation — it exists because <c>await using</c> is the idiom for
    /// a disposable holding network resources, and a type that cannot be
    /// <c>await using</c> gets closed with <c>using</c> and a comment nobody reads.
    /// </remarks>
    public ValueTask DisposeAsync()
    {
        Dispose();
        return ValueTask.CompletedTask;
    }
}