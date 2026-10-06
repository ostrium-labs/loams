// The invoker: the call path a facade method delegates to (design §44 §7.4;
// runtime contract R1–R4).
//
// It is the one place a binding and a request message become an RPC, and it does
// four things the generated facade cannot:
//
//   - attaches the bearer from the client's token source, **per attempt**, because
//     R1's refresh has to change it between two attempts of one logical call;
//   - hands the retry class to <see cref="RetryLoop"/>, which applies it with M1.6's
//     backoff numbers and refreshes the token once on `token_expired`;
//   - gives a mutating call an idempotency key **once per logical call** and reuses
//     it on every retry, so a retried write is the same write (R3);
//   - turns whatever comes back into the typed `LoamsError` hierarchy, so a caller
//     branches on a `reason` and never on a message.
//
// # The same message on every attempt
//
// R3 is enforced structurally rather than by convention: the keyed message is
// computed **before** the loop and the sender is handed the same
// <see cref="Attempt.Request"/> value every time. A sender that built its own
// request would be one refactor away from regenerating the key per attempt, which
// is the exact failure the corpus's `mock_state_idempotent_decide` records.

using Google.Protobuf;

namespace Loams;

/// <summary>A per-call override. Everything is optional: the client's defaults apply.</summary>
public sealed record CallOptions
{
    /// <summary>An empty set of overrides, which is what a caller who asked for nothing gets.</summary>
    public static readonly CallOptions None = new();

    /// <summary>The retries after the first attempt for this call. Zero disables them.</summary>
    public int? MaxRetries { get; init; }

    /// <summary>
    /// Overrides the call's retry class for this call only. False on a read stops
    /// the SDK retrying it; true on a mutation retries something the proto says is
    /// not safe to repeat, which is only correct when the call carries an
    /// idempotency key.
    /// </summary>
    public bool? RetrySafe { get; init; }

    /// <summary>
    /// The caller's own idempotency key, making the retry theirs rather than the
    /// SDK's. Omit it and the runtime mints a UUIDv7 per logical call and reuses it
    /// on every retry (R3).
    /// </summary>
    public string? IdempotencyKey { get; init; }

    /// <summary>Headers for this call. <c>Authorization</c> is set by the runtime and wins.</summary>
    public IReadOnlyDictionary<string, string> Headers { get; init; } =
        new Dictionary<string, string>();

    /// <summary>Reads at this consistency token (D609).</summary>
    public string? ConsistencyToken { get; init; }

    /// <summary>
    /// Folds this call's responses into the client's session store and attaches the
    /// stored token to later reads. Off unless asked for, in both directions (R4).
    /// </summary>
    public bool UseSessionConsistency { get; init; }

    /// <summary>
    /// Makes a server stream reconnect from its last cursor on a retryable failure
    /// (R7). See <see cref="StreamResume{TRequest, TResponse}"/>.
    /// </summary>
    public IStreamResume? Resume { get; init; }
}

/// <summary>One attempt of a call, with everything the sender needs to make it.</summary>
/// <param name="Request">
/// The message, with the idempotency key already set. It is the **same value** on
/// every attempt, which is the whole of R3: a key regenerated per attempt turns
/// one write into two.
/// </param>
/// <param name="AttemptNumber">The zero-based attempt number.</param>
/// <param name="Refreshed">True once R1's single refresh has happened.</param>
/// <param name="CancellationToken">The caller's token.</param>
public readonly record struct Attempt(
    IMessage Request,
    int AttemptNumber,
    bool Refreshed,
    CancellationToken CancellationToken);

/// <summary>One attempt: the message out, the response or the failure back.</summary>
public delegate Task<IMessage> AttemptSender(Attempt attempt);

/// <summary>Everything the retry loop needs, so the policy is testable without a transport.</summary>
/// <param name="RetrySafe">
/// The call's class from the generated binding, or true once the call has been
/// keyed — a mutation becomes retryable when it carries an idempotency key, which
/// is what makes the repeat safe.
/// </param>
/// <param name="MaxRetries">The retries after the first attempt.</param>
/// <param name="CanRefresh">
/// Whether the client's token source can refresh at all. A source that cannot (an
/// API key) makes R1's refresh a no-op **and** the retry is skipped rather than
/// spent on a request that cannot work.
/// </param>
/// <param name="Refresh">The client's refresh, or null for a client with no source.</param>
public sealed record RetryPlan(
    bool RetrySafe,
    int MaxRetries,
    bool CanRefresh = false,
    Func<CancellationToken, ValueTask>? Refresh = null);

/// <summary>The retry loop (R1, R2, R3).</summary>
public static class RetryLoop
{
    /// <summary>
    /// Runs <paramref name="send"/> until it answers or the plan says stop.
    /// </summary>
    /// <param name="rpc">The RPC, for the failures' messages.</param>
    /// <param name="request">
    /// The request message. Passed in rather than taken from the sender, because R3
    /// requires the **same** message on every attempt and a sender that built its
    /// own would be one refactor away from regenerating the key.
    /// </param>
    /// <param name="plan">The retry class, the budget and the refresh.</param>
    /// <param name="send">One attempt.</param>
    /// <param name="cancellationToken">The caller's token.</param>
    /// <param name="onRetry">
    /// Called before each backoff with the wait that is about to happen, instead of
    /// the wait. The conformance suite uses it to keep a retry test fast; nothing in
    /// the SDK does, and it is a parameter rather than a test seam precisely so that
    /// is visible.
    /// </param>
    /// <exception cref="LoamsError">Whatever the last attempt threw, mapped.</exception>
    public static async Task<IMessage> RunAsync(
        string rpc,
        IMessage request,
        RetryPlan plan,
        AttemptSender send,
        CancellationToken cancellationToken,
        Func<TimeSpan, CancellationToken, Task>? onRetry = null)
    {
        ArgumentNullException.ThrowIfNull(request);
        ArgumentNullException.ThrowIfNull(plan);
        ArgumentNullException.ThrowIfNull(send);

        var refreshed = false;
        for (var attempt = 0; ; attempt++)
        {
            if (cancellationToken.IsCancellationRequested)
            {
                // Checked **before** the attempt rather than after the failure: a
                // caller who cancelled between attempts must not spend one.
                throw new LoamsError(Code.Cancelled, rpc,
                    innerException: new OperationCanceledException(cancellationToken));
            }

            try
            {
                return await send(new Attempt(request, attempt, refreshed, cancellationToken)).ConfigureAwait(false);
            }
            catch (Exception thrown)
            {
                var mapped = ErrorMapper.FromException(thrown, rpc);

                // R1: a rejection whose reason is `token_expired` gets exactly one
                // refresh and one retry. A second expiry is reported rather than
                // looped on — a loop is the obvious implementation and it turns a
                // refusal into a hang.
                if (mapped is TokenExpiredError && !refreshed && plan.CanRefresh && plan.Refresh is not null)
                {
                    try
                    {
                        await plan.Refresh(cancellationToken).ConfigureAwait(false);
                    }
                    catch (Exception refreshFailure)
                    {
                        // A refresh that itself failed is the caller's answer: it is
                        // a real failure with its own cause, and reporting the
                        // original expiry would hide why nothing improved.
                        throw ErrorMapper.FromException(refreshFailure, rpc);
                    }

                    refreshed = true;
                    // The refresh retry is not charged to the budget: the same
                    // logical call, so the attempt index does not advance.
                    attempt--;
                    continue;
                }

                if (cancellationToken.IsCancellationRequested)
                {
                    throw GaveUp(mapped, thrown);
                }

                if (!RetryPolicy.ShouldRetry(attempt, plan.MaxRetries, plan.RetrySafe, mapped.Code,
                        cancellationToken.IsCancellationRequested))
                {
                    throw mapped;
                }

                var wait = RetryPolicy.Backoff(attempt);
                if (onRetry is not null)
                {
                    await onRetry(wait, cancellationToken).ConfigureAwait(false);
                }
                else
                {
                    // The token goes to the delay, so a cancellation during a backoff
                    // ends the call rather than waiting out the remaining seconds.
                    await Task.Delay(wait, cancellationToken).ConfigureAwait(false);
                }
            }
        }
    }

    /// <summary>
    /// Records that the caller's cancellation ended a call while a retryable failure
    /// was still outstanding.
    /// </summary>
    /// <remarks>
    /// It keeps the type, code and reason the server sent and says the caller is why
    /// it stopped. It is a <b>new</b> exception rather than the original one with a
    /// longer message: the point is to preserve the *type* a caller caught, and C#
    /// has nowhere to hang an extra message on an existing exception without losing
    /// the subclass. The original stays reachable as
    /// <see cref="Exception.InnerException"/>.
    /// </remarks>
    internal static LoamsError GaveUp(LoamsError mapped, Exception cause)
    {
        var note = $"{mapped.Message} (the caller gave up before the retry)";
        return mapped switch
        {
            FeatureNotInVariantError => new FeatureNotInVariantError(mapped.Rpc, mapped.Metadata,
                mapped.Hint, mapped.Detail, mapped.HttpStatus, note),
            TokenExpiredError => new TokenExpiredError(mapped.Rpc, mapped.Metadata, mapped.Hint,
                mapped.Detail, mapped.HttpStatus, note),
            NotFoundError => new NotFoundError(mapped.Rpc, mapped.Reason, mapped.UnknownReason,
                mapped.Metadata, mapped.Hint, mapped.Detail, mapped.HttpStatus, note),
            InvalidArgumentError => new InvalidArgumentError(mapped.Rpc, mapped.Reason, mapped.UnknownReason,
                mapped.Metadata, mapped.Hint, mapped.Detail, mapped.HttpStatus, note),
            AlreadyExistsError => new AlreadyExistsError(mapped.Rpc, mapped.Reason, mapped.UnknownReason,
                mapped.Metadata, mapped.Hint, mapped.Detail, mapped.HttpStatus, note),
            PermissionDeniedError => new PermissionDeniedError(mapped.Rpc, mapped.Reason, mapped.UnknownReason,
                mapped.Metadata, mapped.Hint, mapped.Detail, mapped.HttpStatus, note),
            UnauthenticatedError => new UnauthenticatedError(mapped.Rpc, mapped.Reason, mapped.UnknownReason,
                mapped.Metadata, mapped.Hint, mapped.Detail, mapped.HttpStatus, note),
            FailedPreconditionError => new FailedPreconditionError(mapped.Rpc, mapped.Reason, mapped.UnknownReason,
                mapped.Metadata, mapped.Hint, mapped.Detail, mapped.HttpStatus, note),
            ResourceExhaustedError => new ResourceExhaustedError(mapped.Rpc, mapped.Reason, mapped.UnknownReason,
                mapped.Metadata, mapped.Hint, mapped.Detail, mapped.HttpStatus, note),
            UnavailableError => new UnavailableError(mapped.Rpc, mapped.Reason, mapped.UnknownReason,
                mapped.Metadata, mapped.Hint, mapped.Detail, mapped.HttpStatus, note),
            DeadlineExceededError => new DeadlineExceededError(mapped.Rpc, mapped.Reason, mapped.UnknownReason,
                mapped.Metadata, mapped.Hint, mapped.Detail, mapped.HttpStatus, note),
            AbortedError => new AbortedError(mapped.Rpc, mapped.Reason, mapped.UnknownReason,
                mapped.Metadata, mapped.Hint, mapped.Detail, mapped.HttpStatus, note),
            InternalError => new InternalError(mapped.Rpc, mapped.Reason, mapped.UnknownReason,
                mapped.Metadata, mapped.Hint, mapped.Detail, mapped.HttpStatus, note),
            UnimplementedError => new UnimplementedError(mapped.Rpc, mapped.Reason, mapped.UnknownReason,
                mapped.Metadata, mapped.Hint, mapped.Detail, mapped.HttpStatus, note),
            _ => new LoamsError(mapped.Code, mapped.Rpc, mapped.Reason, mapped.UnknownReason, mapped.Metadata,
                mapped.Hint, mapped.Detail, mapped.HttpStatus, cause, note),
        };
    }
}

/// <summary>
/// The client's call path: a binding plus a request message becomes a response
/// message, with everything the SDK owns applied on the way.
/// </summary>
public sealed class CallInvoker
{
    private readonly ITokenSource? _source;
    private readonly ConsistencySession? _session;
    private readonly int _maxRetries;

    /// <summary>Builds an invoker over a transport and a client's credentials.</summary>
    public CallInvoker(Transport transport, ITokenSource? source, int maxRetries, ConsistencySession? session)
    {
        Transport = transport ?? throw new ArgumentNullException(nameof(transport));
        _source = source;
        _maxRetries = maxRetries;
        _session = session;
    }

    /// <summary>The transport calls go over.</summary>
    public Transport Transport { get; }

    /// <summary>The protocol this client speaks.</summary>
    public Protocol Protocol => Transport.Protocol;

    /// <summary>The codec this client sends.</summary>
    public Codec Codec => Transport.Codec;

    /// <summary>The client's default retry budget; a call overrides it.</summary>
    public int MaxRetries => _maxRetries;

    /// <summary>
    /// Makes one unary call with the whole runtime contract applied.
    /// </summary>
    /// <remarks>
    /// The response is the generated type the caller named, because the module
    /// methods that call this hold the type statically — which is why
    /// <typeparamref name="TResponse"/> is a type parameter and not a
    /// <see cref="IMessage"/> the caller casts.
    /// </remarks>
    public async Task<TResponse> UnaryAsync<TResponse>(
        CallBinding binding,
        IMessage request,
        CallOptions? options,
        CancellationToken cancellationToken)
        where TResponse : IMessage<TResponse>, new()
    {
        ArgumentNullException.ThrowIfNull(binding);
        ArgumentNullException.ThrowIfNull(request);

        if (binding.Streaming != Streaming.Unary)
        {
            throw ErrorMapper.Internal(binding.Rpc,
                $"{binding.Name} is a server stream; call ServerStreamAsync for it");
        }

        var settings = options ?? CallOptions.None;

        // R3: keyed **once**, before the first attempt, and the same message goes to
        // every attempt. A binding whose schema declares no `idempotency_key` is
        // left exactly as the caller wrote it.
        var keyed = Idempotency.Apply(request, settings.IdempotencyKey ?? string.Empty);

        var plan = PlanFor(binding, settings, keyed.Keyed);
        var token = ConsistencySession.Resolve(settings.ConsistencyToken,
            settings.UseSessionConsistency ? _session : null);

        var response = await RetryLoop.RunAsync(
            binding.Rpc,
            keyed.Request,
            plan,
            (attempt) => SendUnaryAsync(binding, attempt, settings, token),
            cancellationToken).ConfigureAwait(false);

        if (response is not TResponse typed)
        {
            throw ErrorMapper.Internal(binding.Rpc,
                $"{binding.Name} answered with a {response.GetType().Name}, not a {typeof(TResponse).Name}");
        }

        // A store that cannot merge a token counts it and carries on: the RPC
        // succeeded, and turning that into an error would make a caller that
        // retries on it perform the write twice.
        RecordConsistency(settings, typed);
        return typed;
    }

    private async Task<IMessage> SendUnaryAsync(
        CallBinding binding,
        Attempt attempt,
        CallOptions settings,
        string? consistencyToken)
    {
        var body = MessageCodec.Serialize(attempt.Request, Codec);
        var headers = await HeadersAsync(settings, attempt.CancellationToken).ConfigureAwait(false);
        if (consistencyToken is { Length: > 0 })
        {
            headers = new Dictionary<string, string>(headers, StringComparer.OrdinalIgnoreCase)
            {
                [ConsistencySession.Header] = consistencyToken,
            };
        }

        var response = await Transport.SendAsync(new TransportRequest(binding, body, headers),
            attempt.CancellationToken).ConfigureAwait(false);

        if (ResponseReader.UnaryFailure(response, Protocol, binding.Rpc) is { } failure)
        {
            throw ErrorMapper.Map(failure, binding.Rpc);
        }

        var parser = DescriptorFor(binding.ResponseTypeName);
        return MessageCodec.Deserialize(parser, response.Body, Codec);
    }

    /// <summary>
    /// One unary call whose response type is only known at runtime.
    /// </summary>
    /// <remarks>
    /// The dynamic path beside <see cref="UnaryAsync{TResponse}"/>, and it exists for
    /// two callers: a proxy or relay that forwards calls it does not have a generated
    /// type for, and the conformance suite, which replays a corpus whose responses are
    /// descriptors rather than C# types. It goes through the **same** retry loop,
    /// idempotency decision, bearer and error mapping — a second code path here would
    /// be a second set of conformance claims to hold up.
    /// </remarks>
    public async Task<IMessage> UnaryDynamicAsync(
        CallBinding binding,
        IMessage request,
        CallOptions? options,
        CancellationToken cancellationToken)
    {
        ArgumentNullException.ThrowIfNull(binding);
        ArgumentNullException.ThrowIfNull(request);

        if (binding.Streaming != Streaming.Unary)
        {
            throw ErrorMapper.Internal(binding.Rpc, $"{binding.Name} is a server stream; call ServerStreamAsync for it");
        }

        var settings = options ?? CallOptions.None;
        var keyed = Idempotency.Apply(request, settings.IdempotencyKey ?? string.Empty);
        var plan = PlanFor(binding, settings, keyed.Keyed);
        var token = ConsistencySession.Resolve(settings.ConsistencyToken,
            settings.UseSessionConsistency ? _session : null);

        var response = await RetryLoop.RunAsync(binding.Rpc, keyed.Request, plan,
            (attempt) => SendUnaryAsync(binding, attempt, settings, token), cancellationToken).ConfigureAwait(false);

        RecordConsistency(settings, response);
        return response;
    }

    /// <summary>
    /// One server stream whose response type is only known at runtime, plus the
    /// counters a caller needs to reason about it.
    /// </summary>
    public ServerStreamHandle OpenServerStream(
        CallBinding binding,
        IMessage request,
        CallOptions? options,
        CancellationToken cancellationToken)
    {
        ArgumentNullException.ThrowIfNull(binding);
        ArgumentNullException.ThrowIfNull(request);

        if (binding.Streaming != Streaming.Server)
        {
            throw ErrorMapper.Internal(binding.Rpc, $"{binding.Name} is unary; call UnaryAsync for it");
        }

        var settings = options ?? CallOptions.None;
        var reader = new ServerStreamReader(this, binding, request, settings, settings.Resume);
        return new ServerStreamHandle(reader, reader.ReadAsync(cancellationToken));
    }

    /// <summary>Opens a server stream, with the errors mapped and the resume wired in.</summary>
    public IAsyncEnumerable<TResponse> ServerStreamAsync<TResponse>(
        CallBinding binding,
        IMessage request,
        CallOptions? options,
        CancellationToken cancellationToken)
        where TResponse : IMessage<TResponse>, new()
    {
        ArgumentNullException.ThrowIfNull(binding);
        ArgumentNullException.ThrowIfNull(request);

        if (binding.Streaming != Streaming.Server)
        {
            throw ErrorMapper.Internal(binding.Rpc,
                $"{binding.Name} is unary; call UnaryAsync for it");
        }

        var handle = OpenServerStream(binding, request, options, cancellationToken);
        return Cast<TResponse>(handle.Messages, binding);
    }

    private static async IAsyncEnumerable<TResponse> Cast<TResponse>(
        IAsyncEnumerable<IMessage> source,
        CallBinding binding)
        where TResponse : IMessage<TResponse>, new()
    {
        await foreach (var message in source.ConfigureAwait(false))
        {
            if (message is TResponse typed)
            {
                yield return typed;
                continue;
            }
            throw ErrorMapper.Internal(binding.Rpc,
                $"{binding.Name} yielded a {message.GetType().Name}, not a {typeof(TResponse).Name}");
        }
    }

    /// <summary>
    /// The headers one attempt goes out with, including the bearer fetched **per
    /// attempt**.
    /// </summary>
    /// <remarks>
    /// Per attempt rather than per call, and that is the whole of R1's mechanism:
    /// the refresh changes the token, and the retry has to carry the new one. A
    /// bearer computed once per call would be the stale one on every retry after a
    /// refresh, and the failure would look like "the refresh did not work" rather
    /// than "the refresh was never used".
    ///
    /// The caller's own <c>Authorization</c> is <b>dropped</b>, not merged: a
    /// caller-supplied bearer is one the runtime cannot refresh, and honouring it
    /// would make R1 unreachable for the call that set it.
    /// </remarks>
    internal async Task<IReadOnlyDictionary<string, string>> HeadersAsync(
        CallOptions settings,
        CancellationToken cancellationToken)
    {
        var headers = new Dictionary<string, string>(StringComparer.OrdinalIgnoreCase);
        foreach (var (name, value) in settings.Headers)
        {
            if (string.Equals(name, "Authorization", StringComparison.OrdinalIgnoreCase))
            {
                continue;
            }
            headers[name] = value;
        }

        if (_source is not null)
        {
            var token = await _source.GetTokenAsync(cancellationToken).ConfigureAwait(false);
            if (!string.IsNullOrEmpty(token))
            {
                headers["Authorization"] = token;
            }
        }
        return headers;
    }

    private RetryPlan PlanFor(CallBinding binding, CallOptions settings, bool keyed)
    {
        var maxRetries = settings.MaxRetries ?? _maxRetries;
        if (maxRetries < 0)
        {
            maxRetries = 0;
        }

        // D610: a `safe` call always retries; a mutation retries once it carries an
        // idempotency key, which `Idempotency.Apply` has just decided.
        var retrySafe = binding.Retry == RetryClass.Safe || keyed;
        if (settings.RetrySafe is { } forced)
        {
            retrySafe = forced;
        }

        // The refresh is taken as a delegate rather than as the source, so the plan
        // holds a value that does not capture a nullable source and has to be
        // re-checked at every use.
        return _source is null
            ? new RetryPlan(retrySafe, maxRetries)
            : new RetryPlan(retrySafe, maxRetries, _source.CanRefresh, _source.RefreshAsync);
    }

    /// <summary>
    /// Folds a response's consistency token into the session, when the caller opted
    /// in. A merge failure is swallowed on purpose — see the caller.
    /// </summary>
    private void RecordConsistency(CallOptions settings, IMessage response)
    {
        if (_session is null || !settings.UseSessionConsistency)
        {
            return;
        }
        var field = response.Descriptor.FindFieldByName("consistency_token");
        if (field?.Accessor.GetValue(response) is not string token || token.Length == 0)
        {
            return;
        }
        try
        {
            _session.Merge(token);
        }
        catch (InvalidOperationException)
        {
            // No RPC carries a `consistency_token` yet (R4), and when one does the
            // store may legitimately be asked to merge two it cannot. The call
            // succeeded; refusing it here would make a caller that retries on the
            // error perform the write twice.
        }
    }

    private static Google.Protobuf.Reflection.MessageDescriptor DescriptorFor(string clrTypeName) =>
        Descriptors.Find(clrTypeName)
        ?? throw ErrorMapper.Internal(string.Empty, $"no generated message type named {clrTypeName}");
}
