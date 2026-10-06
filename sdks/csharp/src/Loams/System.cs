// The module catalogue, feature detection and the version check (design §44 §4
// and §7.2, D600; runtime contract R5, R9).
//
// R5 has two halves and an SDK needs both:
//
//   1. **Without calling.** `GetInstance.services[]` says which packages this
//      binary carries. One call, no auth, cheap. The catalogue is cached for the
//      life of the client and concurrent readers share one in-flight fetch, so a
//      hundred `await GuardAsync("live")` at once cost one `GetInstance`.
//   2. **When the caller calls anyway.** Every RPC of an absent package answers
//      `unimplemented` with `reason = feature_not_in_variant` and the variant in
//      `metadata.variant`, which the transport turns into a
//      <see cref="FeatureNotInVariantError"/>.
//
// `GuardAsync` raises that **same type** from the catalogue, so one `catch` covers
// "the guard said no" and "the server refused" — which is the point of the guard
// raising the same type rather than a flag.
//
// R9 is the version check: the SDK declares <see cref="Facade.ProtoRev"/> and
// reports the server's `api_versions` beside it, and a package the SDK speaks that
// the server does not serve is a **warning, not an exception**. The SDK still works
// for the modules that are there; the caller decides what a missing one means.

using System.Collections.Frozen;

namespace Loams;

/// <summary>One entry of the API catalogue: a package and what this binary does with it.</summary>
/// <param name="Package">The proto package, for example <c>loams.live.v1</c>.</param>
/// <param name="Version">The package's API version, for example <c>v1</c>.</param>
/// <param name="Available">
/// Whether this binary serves it. False means every one of its RPCs answers
/// <c>unimplemented</c> with reason <c>feature_not_in_variant</c>.
/// </param>
/// <param name="Services">The services in the package, fully qualified.</param>
/// <param name="Unstable">
/// Whether the package's wire contract may still change (<c>loams.options.v1.
/// ModuleOptions.unstable</c>, §44 §10.3), so <c>buf breaking</c> skips it.
/// </param>
public sealed record ServiceStatus(string Package, string Version, bool Available, IReadOnlyList<string> Services, bool Unstable);

/// <summary>
/// The catalogue, derived from one <c>GetInstance</c> (R5). It answers "which of
/// these do I feature-detect" without a second call.
/// </summary>
/// <param name="Served">The packages this binary serves.</param>
/// <param name="Unavailable">The packages it knows of and does not serve.</param>
/// <param name="Services">Every entry, served or not.</param>
public sealed record ServiceCatalogue(
    IReadOnlyList<string> Served,
    IReadOnlyList<string> Unavailable,
    IReadOnlyList<ServiceStatus> Services);

/// <summary>
/// What <c>client.System.VersionAsync</c> reports: this SDK's proto revision beside
/// the server's <c>api_versions</c>, and whether they are compatible (R9).
/// </summary>
/// <param name="ProtoRev">This SDK's <see cref="Facade.ProtoRev"/>.</param>
/// <param name="ServerVersion">The server's own semver.</param>
/// <param name="ApiVersions">The packages the server serves.</param>
/// <param name="Missing">
/// Packages this SDK speaks that the server does not serve. A caller decides what
/// they mean; the SDK does not refuse to work because of them.
/// </param>
/// <param name="Compatible">
/// Whether the server serves every package this SDK speaks. A mismatch is a
/// warning, never an exception.
/// </param>
public sealed record VersionReport(
    string ProtoRev,
    string ServerVersion,
    IReadOnlyList<string> ApiVersions,
    IReadOnlyList<string> Missing,
    bool Compatible);

/// <summary>
/// The catalogue, feature detection and the version check.
/// </summary>
/// <remarks>
/// One object per client, sharing the client's invoker and cache. A separate
/// <c>SystemClient</c> a caller had to build would be a second thing to configure
/// and a second place for the cache to live, and the whole of R5 is that the
/// catalogue is shared.
/// </remarks>
public sealed class LoamsSystem
{
    private readonly CallInvoker _invoker;
    private readonly SemaphoreSlim _gate = new(1, 1);
    private readonly Dictionary<string, ServiceStatus> _catalogue = new(StringComparer.Ordinal);

    private Task<ServiceCatalogue>? _inFlight;

    /// <summary>Builds the system surface over a client's invoker.</summary>
    internal LoamsSystem(CallInvoker invoker) => _invoker = invoker;

    /// <summary>
    /// The variant name used when the guard refuses from the catalogue, which does
    /// not carry one.
    /// </summary>
    /// <remarks>
    /// The empty string, deliberately, rather than a plausible-looking guess. The
    /// corpus's <c>live_watch</c> case shows a real variant (<c>standard</c>) coming
    /// out of a server refusal's metadata; the guard learned the same fact from the
    /// catalogue, which has no such field, and a guessed variant in a log is worse
    /// than an honest absence.
    /// </remarks>
    public const string VariantUnknown = "";

    /// <summary>
    /// The catalogue, fetched once and cached for the life of the client (R5).
    /// </summary>
    /// <remarks>
    /// **Concurrent readers share one in-flight fetch.** A hundred callers asking at
    /// once must cost one <c>GetInstance</c>, not a hundred: the catalogue is the
    /// SDK's first call on every cold start, and a stampede of them is how an
    /// instance's version endpoint falls over at exactly the moment a fleet restarts.
    /// </remarks>
    public async Task<ServiceCatalogue> CatalogueAsync(CancellationToken cancellationToken = default)
    {
        // Fast path: a completed fetch needs no lock at all.
        if (_catalogue.Count > 0)
        {
            return Build();
        }

        await _gate.WaitAsync(cancellationToken).ConfigureAwait(false);
        try
        {
            if (_catalogue.Count > 0)
            {
                return Build();
            }
        }
        finally
        {
            _gate.Release();
        }

        // The fetch itself runs outside the gate, with the task published so the
        // second caller waits on **this** fetch rather than starting its own. The
        // gate is released first on purpose: holding it across the RPC would serialise
        // every reader behind the first one's network round trip even though they all
        // want the same answer.
        var fetch = _inFlight ??= FetchAsync(cancellationToken);
        try
        {
            return await fetch.ConfigureAwait(false);
        }
        finally
        {
            // Cleared so a **failed** fetch does not poison the client for ever: the
            // next caller tries again rather than inheriting a cached exception. A
            // successful one stays, because that is the cache.
            if (fetch.IsFaulted || fetch.IsCanceled)
            {
                lock (_catalogue)
                {
                    if (ReferenceEquals(_inFlight, fetch))
                    {
                        _inFlight = null;
                    }
                }
            }
        }
    }

    private async Task<ServiceCatalogue> FetchAsync(CancellationToken cancellationToken)
    {
        var binding = Facade.Binding("instance", "GetInstance");
        var response = await _invoker.UnaryAsync<Loams.Instance.V1.GetInstanceResponse>(
            binding, new Loams.Instance.V1.GetInstanceRequest(), CallOptions.None, cancellationToken)
            .ConfigureAwait(false);

        lock (_catalogue)
        {
            _catalogue.Clear();
            foreach (var service in response.Services)
            {
                // The generated `loams.instance.v1.ServiceStatus` is the wire type;
                // the SDK's own `Loams.ServiceStatus` is what the catalogue hands
                // back. Copied field for field rather than aliased, so the SDK's
                // public surface does not change shape when the proto does.
                _catalogue[service.Package] = new ServiceStatus(
                    service.Package, service.Version, service.Available, service.Services, service.Unstable);
            }
        }
        return Build();
    }

    private ServiceCatalogue Build()
    {
        lock (_catalogue)
        {
            var served = new List<string>();
            var unavailable = new List<string>();
            foreach (var entry in _catalogue.Values.OrderBy((entry) => entry.Package, StringComparer.Ordinal))
            {
                (entry.Available ? served : unavailable).Add(entry.Package);
            }
            return new ServiceCatalogue(served, unavailable,
                _catalogue.Values.OrderBy((entry) => entry.Package, StringComparer.Ordinal).ToArray());
        }
    }

    /// <summary>
    /// Whether the server serves a proto package, from the catalogue (R5). One call
    /// the first time and none after.
    /// </summary>
    public async Task<bool> AvailableAsync(string package, CancellationToken cancellationToken = default)
    {
        var catalogue = await CatalogueAsync(cancellationToken).ConfigureAwait(false);
        return catalogue.Served.Contains(package, StringComparer.Ordinal);
    }

    /// <summary>
    /// Whether the server serves a module, from the catalogue. The same question as
    /// <see cref="AvailableAsync"/> under a module name, because the guard resolves a
    /// module to its package.
    /// </summary>
    public async Task<bool> AvailableModuleAsync(string module, CancellationToken cancellationToken = default)
    {
        var catalogue = await CatalogueAsync(cancellationToken).ConfigureAwait(false);
        return catalogue.Served.Contains(PackageOf(module), StringComparer.Ordinal);
    }

    /// <summary>
    /// Fails with a <see cref="FeatureNotInVariantError"/> when the module is not
    /// served, and returns nothing when it is — spending no RPC on a call that cannot
    /// work (R5).
    /// </summary>
    /// <exception cref="FeatureNotInVariantError">
    /// The module is not in this build variant. The **same type** a refused call
    /// raises, so one catch covers both.
    /// </exception>
    public async Task GuardAsync(string module, CancellationToken cancellationToken = default)
    {
        if (await AvailableModuleAsync(module, cancellationToken).ConfigureAwait(false))
        {
            return;
        }

        var package = PackageOf(module);
        throw new FeatureNotInVariantError(
            $"loams.{module}",
            new Dictionary<string, string>(StringComparer.Ordinal) { ["package"] = package },
            message: $"loams.{package} is not served by this instance, so no call on loams.{module} can work");
    }

    /// <summary>
    /// This SDK's proto revision beside the server's <c>api_versions</c> (R9). A
    /// package the SDK speaks that the server does not serve is reported in
    /// <see cref="VersionReport.Missing"/>, not raised.
    /// </summary>
    public async Task<VersionReport> VersionAsync(CancellationToken cancellationToken = default)
    {
        var binding = Facade.Binding("instance", "GetInstance");
        var response = await _invoker.UnaryAsync<Loams.Instance.V1.GetInstanceResponse>(
            binding, new Loams.Instance.V1.GetInstanceRequest(), CallOptions.None, cancellationToken)
            .ConfigureAwait(false);

        var served = response.ApiVersions.ToArray();
        var missing = Facade.ProtoPackages
            .Where((package) => !served.Contains(package, StringComparer.Ordinal))
            .ToArray();

        return new VersionReport(Facade.ProtoRev, response.ServerVersion, served, missing,
            Compatible: missing.Length == 0);
    }

    /// <summary>
    /// Forgets the cached catalogue, so the next feature check calls again. For an
    /// instance whose build variant changed under a running process.
    /// </summary>
    public void InvalidateCatalogue()
    {
        lock (_catalogue)
        {
            _catalogue.Clear();
            _inFlight = null;
        }
    }

    /// <summary>The proto package a module name binds, or the name itself when it is one.</summary>
    /// <remarks>
    /// A module name and a package name are both accepted by the guards, because a
    /// caller feature-detecting from a log line or a proto descriptor has the package
    /// and a caller reading the README has the module, and neither should have to
    /// translate. An unknown name is returned unchanged, so the answer is "not
    /// served" rather than an exception about a typo.
    /// </remarks>
    public static string PackageOf(string moduleOrPackage)
    {
        foreach (var binding in Facade.Modules)
        {
            if (string.Equals(binding.Name, moduleOrPackage, StringComparison.Ordinal))
            {
                return binding.Package;
            }
        }
        return moduleOrPackage;
    }
}