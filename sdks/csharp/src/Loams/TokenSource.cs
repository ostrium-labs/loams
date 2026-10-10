// Credentials: the token sources and R1's one-refresh-one-retry (design §44 §7.4,
// D608; runtime contract R1).
//
// A client's bearer comes from a token source and travels in
// `Authorization: Bearer` — never in the query string, never in a URL. A 401
// carrying `reason = token_expired` triggers **exactly one** refresh and **one**
// retry; a second expiry is reported.
//
// # Why one refresh and not a loop
//
// A loop is the obvious implementation and it is wrong: if a server answers
// `token_expired` to a token the source has just refreshed, the credential is not
// the problem and retrying forever converts a refusal into a hang. R1's "exactly
// one" is therefore enforced here rather than left to each token source, so no
// source can accidentally retry twice and none can accidentally retry never.
//
// # Cancellation is part of the refresh
//
// `GetTokenAsync` and `RefreshAsync` both take the caller's
// `CancellationToken`. A caller who cancels during a token fetch gets the
// cancellation, not a token fetch that finishes and starts a request nobody wants.

using System.Collections.Frozen;

namespace Loams;

/// <summary>
/// Where a client's bearer comes from.
/// </summary>
/// <remarks>
/// Two methods, not one, and the split is the whole of R1: <see cref="GetTokenAsync"/>
/// is what every attempt asks, and <see cref="RefreshAsync"/> is what the runtime
/// calls **once** on a <c>token_expired</c>. A source that cannot refresh — an API
/// key, which does not expire — implements the second as a no-op and the runtime
/// does not spend a retry on it, because a retry with the same key answers the
/// same 401.
/// </remarks>
public interface ITokenSource
{
    /// <summary>The bearer to send, without the <c>Bearer </c> prefix.</summary>
    ValueTask<string> GetTokenAsync(CancellationToken cancellationToken);

    /// <summary>
    /// Obtains a new bearer after the server rejected the last one. Called at
    /// most once per logical call.
    /// </summary>
    ValueTask RefreshAsync(CancellationToken cancellationToken);

    /// <summary>
    /// Whether this source can produce a different bearer on
    /// <see cref="RefreshAsync"/>. A source that cannot says false, and the
    /// runtime then reports the expiry rather than retrying with the same
    /// credential — the shape R1 calls out for an API key.
    /// </summary>
    bool CanRefresh { get; }
}

/// <summary>
/// An API key, which does not expire: a fixed bearer with no refresh (D608's
/// <c>ApiKey</c>).
/// </summary>
/// <remarks>
/// <see cref="RefreshAsync"/> throws rather than being a silent no-op, and the
/// runtime never calls it because <see cref="CanRefresh"/> is false. Throwing is
/// the honest body for a method the interface requires and this object cannot
/// honour: a silent no-op would let a future caller that ignored
/// <see cref="CanRefresh"/> believe a refresh had happened.
/// </remarks>
public sealed class ApiKeyTokenSource : ITokenSource
{
    private readonly string _key;

    /// <summary>Builds a source over an API key.</summary>
    /// <exception cref="ArgumentException">The key is empty. A client with no
    /// credential is a caller error, and finding out at construction is better
    /// than finding out as a 401.</exception>
    public ApiKeyTokenSource(string key)
    {
        ArgumentException.ThrowIfNullOrEmpty(key);
        _key = key;
    }

    /// <inheritdoc/>
    public bool CanRefresh => false;

    /// <inheritdoc/>
    public ValueTask<string> GetTokenAsync(CancellationToken cancellationToken) =>
        ValueTask.FromResult(_key);

    /// <inheritdoc/>
    public ValueTask RefreshAsync(CancellationToken cancellationToken) =>
        throw new InvalidOperationException(
            "an API key does not expire and cannot be refreshed; the runtime reads CanRefresh and does not retry");
}

/// <summary>A fixed bearer, for a token the caller manages itself.</summary>
public sealed class StaticTokenSource : ITokenSource
{
    private readonly string _token;

    /// <summary>Builds a source over a token.</summary>
    public StaticTokenSource(string token)
    {
        ArgumentException.ThrowIfNullOrEmpty(token);
        _token = token;
    }

    /// <inheritdoc/>
    public bool CanRefresh => false;

    /// <inheritdoc/>
    public ValueTask<string> GetTokenAsync(CancellationToken cancellationToken) =>
        ValueTask.FromResult(_token);

    /// <inheritdoc/>
    public ValueTask RefreshAsync(CancellationToken cancellationToken) =>
        throw new InvalidOperationException(
            "a static token is managed by the caller, so this SDK cannot refresh it; " +
            "implement ITokenSource for a source that can");
}

/// <summary>
/// A bearer read from the environment (<c>LOAMS_API_KEY</c>, <c>LOAMS_TOKEN</c>,
/// <c>LOAMS_ENDPOINT</c> — D608's <c>Env</c>), for a script or a CI job.
/// </summary>
/// <remarks>
/// Read **once**, at construction, not per call: an environment variable cannot
/// change under a running process in any way that is useful to an SDK, and
/// re-reading it per call would make a client whose variable disappeared mid-run
/// fail on a later call with no visible cause. <see cref="CanRefresh"/> is false
/// because there is nothing to refresh from.
/// </remarks>
public sealed class EnvTokenSource : ITokenSource
{
    private readonly string _token;

    /// <summary>Builds a source from the process environment.</summary>
    /// <param name="variableName">
    /// The variable to read, or null for <c>LOAMS_API_KEY</c> then
    /// <c>LOAMS_TOKEN</c> in that order.
    /// </param>
    /// <exception cref="InvalidOperationException">
    /// Neither variable is set. Named in the message, because "the environment is
    /// missing a credential" is a message a script author can act on.
    /// </exception>
    public EnvTokenSource(string? variableName = null)
    {
        if (variableName is not null)
        {
            _token = Environment.GetEnvironmentVariable(variableName) ??
                     throw new InvalidOperationException(
                         $"the {variableName} environment variable is not set");
            return;
        }
        _token = Environment.GetEnvironmentVariable("LOAMS_API_KEY") ??
                 Environment.GetEnvironmentVariable("LOAMS_TOKEN") ??
                 throw new InvalidOperationException(
                     "neither LOAMS_API_KEY nor LOAMS_TOKEN is set; pass a variable name to read a different one");
    }

    /// <inheritdoc/>
    public bool CanRefresh => false;

    /// <inheritdoc/>
    public ValueTask<string> GetTokenAsync(CancellationToken cancellationToken) =>
        ValueTask.FromResult(_token);

    /// <inheritdoc/>
    public ValueTask RefreshAsync(CancellationToken cancellationToken) =>
        throw new InvalidOperationException("an environment variable cannot be refreshed");
}

/// <summary>
/// A source that hands out a scripted sequence of bearers and counts refreshes
/// (D608's <c>OidcExchange</c> shape, minus the exchange).
/// </summary>
/// <remarks>
/// The **class under test** rather than a production source: R1's "exactly one
/// refresh" is a claim about how many times <see cref="RefreshAsync"/> was
/// called, and it cannot be checked from the token values alone — a source that
/// returned the same stale token twice would look identical from the outside. So
/// the refresh count is a first-class part of this type.
///
/// # What it is not
///
/// It is **not** the OIDC exchange of D608. That one posts RFC 8693 to the
/// instance's <c>/oauth/token</c> endpoint, which no instance serves yet (MT, API1
/// Task 7), so an implementation would be a protocol client with nothing to talk
/// to. It is not left out silently: `README.md` "What is not here yet" says so.
/// </remarks>
public sealed class ScriptedTokenSource : ITokenSource
{
    private readonly string[] _tokens;
    private readonly Func<CancellationToken, ValueTask>? _onRefresh;
    private int _index;
    private int _refreshes;

    /// <summary>Builds a source over a scripted sequence of bearers.</summary>
    /// <param name="tokens">
    /// The bearers, in order. The last one repeats once the sequence runs out,
    /// because a source that ran dry mid-call would turn a test into a
    /// <c>IndexOutOfRangeException</c> that says nothing.
    /// </param>
    /// <param name="onRefresh">Called from <see cref="RefreshAsync"/>, for a test that hooks the refresh.</param>
    public ScriptedTokenSource(string[] tokens, Func<CancellationToken, ValueTask>? onRefresh = null)
    {
        ArgumentNullException.ThrowIfNull(tokens);
        if (tokens.Length == 0)
        {
            throw new ArgumentException("a scripted token source needs at least one token", nameof(tokens));
        }
        _tokens = tokens;
        _onRefresh = onRefresh;
    }

    /// <inheritdoc/>
    public bool CanRefresh => true;

    /// <summary>How many times the runtime has refreshed. R1's claim is about this number.</summary>
    public int Refreshes => Volatile.Read(ref _refreshes);

    /// <inheritdoc/>
    public ValueTask<string> GetTokenAsync(CancellationToken cancellationToken) =>
        ValueTask.FromResult(_tokens[Math.Min(Volatile.Read(ref _index), _tokens.Length - 1)]);

    /// <inheritdoc/>
    public async ValueTask RefreshAsync(CancellationToken cancellationToken)
    {
        Interlocked.Increment(ref _refreshes);
        if (_onRefresh is not null)
        {
            await _onRefresh(cancellationToken).ConfigureAwait(false);
        }
        Interlocked.Increment(ref _index);
    }
}

/// <summary>
/// A source whose bearer a caller supplies per call, for the case where the
/// credential is not available at construction time — a desktop app that signs in
/// after the client was built, for instance.
/// </summary>
/// <remarks>
/// A delegate rather than an interface implementation, because the shape of this
/// SDK's users is "I have a function that gives me a token", and an interface here
/// would be three members to write for that. <see cref="CanRefresh"/> is true and
/// refresh is a no-op, because the delegate is called again on the retry and so
/// produces a newer token if the caller's storage has one — which is exactly the
/// contract, without this SDK knowing where tokens come from.
/// </remarks>
public sealed class DelegateTokenSource : ITokenSource
{
    private readonly Func<CancellationToken, ValueTask<string>> _token;
    private readonly Func<CancellationToken, ValueTask>? _refresh;

    /// <summary>Builds a source over a delegate.</summary>
    /// <param name="token">Returns the current bearer.</param>
    /// <param name="refresh">
    /// Invalidates the caller's cached token, or null when the delegate has no
    /// cache to clear.
    /// </param>
    public DelegateTokenSource(
        Func<CancellationToken, ValueTask<string>> token,
        Func<CancellationToken, ValueTask>? refresh = null)
    {
        ArgumentNullException.ThrowIfNull(token);
        _token = token;
        _refresh = refresh;
    }

    /// <inheritdoc/>
    public bool CanRefresh => true;

    /// <inheritdoc/>
    public ValueTask<string> GetTokenAsync(CancellationToken cancellationToken) => _token(cancellationToken);

    /// <inheritdoc/>
    public async ValueTask RefreshAsync(CancellationToken cancellationToken)
    {
        if (_refresh is not null)
        {
            await _refresh(cancellationToken).ConfigureAwait(false);
        }
    }
}

/// <summary>
/// The session consistency token store (design §44 §7.4, D609; runtime contract
/// R4).
/// </summary>
/// <remarks>
/// **Off by default.** Every read is then <c>STRONG</c> on its own, which is
/// correct but does not give read-your-writes across processes. Turn it on when
/// one process is both writing and reading.
///
/// # Why this store refuses to merge
///
/// §44 §7.4 says a store "merges returned tokens", by max offset per stream and
/// partition — which needs the token's **encoding**, and that encoding is not in
/// any proto yet (§05 §5 defines the semantics; API1's write paths will carry an
/// opaque <c>v1:</c> string). So this store keeps the token it was given, and two
/// different tokens meeting is reported as an error rather than merged into a
/// wrong one: a silently-wrong consistency token reads stale data, which is worse
/// than a failure. When the encoding lands, `Merge` is the one method that changes.
/// </remarks>
public sealed class ConsistencySession
{
    private readonly object _gate = new();
    private string? _token;

    /// <summary>The token later reads go out with, or null when none has been recorded.</summary>
    public string? Current
    {
        get
        {
            lock (_gate)
            {
                return _token;
            }
        }
    }

    /// <summary>How many distinct tokens this session has been given. One is the healthy case.</summary>
    public int Recorded { get; private set; }

    /// <summary>
    /// Folds a response's token in.
    /// </summary>
    /// <exception cref="InvalidOperationException">
    /// The token differs from the one already held. The message names both, because
    /// "the encoding is not in the protos yet" is the reason to fix this by
    /// turning the session off, and a caller needs to know that is the choice.
    /// </exception>
    public void Merge(string? token)
    {
        if (string.IsNullOrEmpty(token))
        {
            return;
        }
        lock (_gate)
        {
            if (_token is not null && !string.Equals(_token, token, StringComparison.Ordinal))
            {
                throw new InvalidOperationException(
                    $"this session holds consistency token '{_token}' and was just given '{token}', and the " +
                    "token encoding is not in the protos yet, so they cannot be merged; turn the session " +
                    "off (Options.SessionConsistency = false) or thread tokens by hand");
            }
            _token = token;
            Recorded++;
        }
    }

    /// <summary>
    /// The token a read goes out with: an explicit one wins, otherwise the
    /// session's — which is what makes <c>WithSessionConsistency</c>
    /// read-your-writes rather than recording a token and never sending it (R4).
    /// </summary>
    public static string? Resolve(string? explicitToken, ConsistencySession? session) =>
        !string.IsNullOrEmpty(explicitToken) ? explicitToken : session?.Current;

    /// <summary>The header a consistency token travels in.</summary>
    public const string Header = "loams-consistency-token";

    /// <summary>The header names read as well: the empty string means <c>STRONG</c>.</summary>
    public static readonly IReadOnlyDictionary<string, string> StrongRead =
        FrozenDictionary<string, string>.Empty;
}