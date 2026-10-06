// Server streams and the resume policy (design §44 §7.4; runtime contract R7).
//
// A server stream is an `IAsyncEnumerable<T>`, which is C#'s native async
// iteration and needs no wrapper type to be usable in a `foreach await`. There is
// no client streaming and no bidi (D420).
//
// # Why a stream is not "retried like a unary call"
//
// A unary call is idempotent or it is not, so the retry decision is a property of
// the call. A stream is different: the server hands out cursors, and a client
// that reconnects from the beginning re-yields everything the caller already saw,
// while a client that reconnects from nothing misses everything that changed while
// it was away. Both are worse than an error — one duplicates, the other silently
// misses — so a stream reconnects **from the last cursor the caller applied** and
// carries on. That is R7, and it is opt-in per call because it needs the caller's
// RPC to spell "resume from" and the caller is the only one who knows how.
//
// # Heartbeats are liveness, not data
//
// The server sends an empty frame on a timer so proxies do not idle the connection
// out (§37 §8.3). Yielding those to a `foreach` would render an empty row every
// fifteen seconds, so a heartbeat is counted and not yielded. The corpus's
// `mock_state_stream_heartbeat` is the recording of the distinction: two frames,
// one message.

using System.Net.Http.Headers;
using System.Runtime.CompilerServices;
using Google.Protobuf;

namespace Loams;

/// <summary>A cursor a stream resumes from: an opaque server-issued position (R7).</summary>
/// <param name="Value">The cursor's own encoding, which the server defines and the SDK treats as opaque.</param>
public readonly record struct StreamCursor(string Value)
{
    /// <summary>Whether this cursor names a position at all.</summary>
    public bool IsEmpty => string.IsNullOrEmpty(Value);

    /// <summary>The empty cursor, meaning "from the start".</summary>
    public static StreamCursor None => new(string.Empty);
}

/// <summary>
/// How one stream reconnects: how the cursor is read off a message, and how a
/// request is rewritten to carry it.
/// </summary>
/// <remarks>
/// Type-erased on purpose. A generic parameter here would force every caller to
/// name both message types at the call site and would put the generated types into
/// the runtime's signatures; the two delegates below carry the same information
/// with a typed body, so the compiler still checks that the cursor comes from a
/// response and the resume produces a request.
/// </remarks>
public interface IStreamResume
{
    /// <summary>How many reconnects this stream may make. Negative means the client's own default.</summary>
    int MaxRetries { get; }

    /// <summary>The cursor of a message the caller has applied, or an empty cursor.</summary>
    StreamCursor CursorOf(IMessage message);

    /// <summary>The request to re-open with, carrying <paramref name="cursor"/>.</summary>
    IMessage Resume(StreamCursor cursor, IMessage request);
}

/// <summary>
/// The typed form of <see cref="IStreamResume"/>, which is what a caller writes.
/// </summary>
/// <typeparam name="TRequest">The stream's request type.</typeparam>
/// <typeparam name="TResponse">The stream's response type.</typeparam>
public sealed record StreamResume<TRequest, TResponse> : IStreamResume
    where TRequest : IMessage<TRequest>, new()
    where TResponse : IMessage<TResponse>
{
    /// <summary>
    /// Builds a resume policy. Written as a constructor rather than a positional
    /// record because the two delegates are named after the members
    /// <see cref="IStreamResume"/> declares, and a positional record cannot have a
    /// generated property of the same name as a hand-written one.
    /// </summary>
    /// <param name="cursorOf">Reads the cursor off a message the caller has applied.</param>
    /// <param name="resume">Rewrites the request to carry a cursor.</param>
    public StreamResume(Func<TResponse, string> cursorOf, Func<string, TRequest, TRequest> resume)
    {
        CursorOfResponse = cursorOf ?? throw new ArgumentNullException(nameof(cursorOf));
        ResumeRequest = resume ?? throw new ArgumentNullException(nameof(resume));
    }

    /// <summary>The typed form of <see cref="IStreamResume.CursorOf"/>.</summary>
    public Func<TResponse, string> CursorOfResponse { get; }

    /// <summary>The typed form of <see cref="IStreamResume.Resume"/>.</summary>
    public Func<string, TRequest, TRequest> ResumeRequest { get; }

    /// <inheritdoc/>
    public int MaxRetries { get; init; } = -1;

    /// <inheritdoc/>
    public StreamCursor CursorOf(IMessage message) =>
        message is TResponse typed ? new StreamCursor(CursorOfResponse(typed)) : StreamCursor.None;

    /// <inheritdoc/>
    public IMessage Resume(StreamCursor cursor, IMessage request)
    {
        if (request is not TRequest typed)
        {
            // A request of the wrong type is a caller error at the seam, and the
            // message names both types rather than throwing InvalidCastException
            // with a stack trace and nothing else.
            throw ErrorMapper.Internal(string.Empty,
                $"the resume policy reads a {typeof(TRequest).Name} but was handed a {request.GetType().Name}");
        }
        return ResumeRequest(cursor.Value, typed);
    }
}

/// <summary>
/// One server stream, and the counts a caller needs about it.
/// </summary>
/// <remarks>
/// A handle rather than a bare <see cref="IAsyncEnumerable{T}"/> because two of R7's
/// clauses are about things the sequence alone cannot say: how many heartbeats the
/// server sent (<c>mock_state_stream_heartbeat</c> records two frames and one
/// message, and only the second number is visible from the sequence), and how many
/// times the stream re-opened (the whole of "resume from the cursor without
/// re-yielding"). <c>Messages</c> is still an <see cref="IAsyncEnumerable{T}"/>, so a
/// caller who does not care about either writes an ordinary <c>await foreach</c>.
/// </remarks>
public sealed class ServerStreamHandle
{
    private readonly ServerStreamReader _reader;

    internal ServerStreamHandle(ServerStreamReader reader, IAsyncEnumerable<IMessage> messages)
    {
        _reader = reader;
        Messages = messages;
    }

    /// <summary>The data messages, reconnecting from the last applied cursor when the caller asked for it (R7).</summary>
    public IAsyncEnumerable<IMessage> Messages { get; }

    /// <summary>How many heartbeat frames the server sent.</summary>
    public int Heartbeats => _reader.Heartbeats;

    /// <summary>How many times the stream re-opened from its cursor.</summary>
    public int Reconnects => _reader.Reconnects;

    /// <summary>
    /// What arrived on the wire, in order, one entry per frame.
    /// </summary>
    /// <remarks>
    /// The <c>Messages</c> sequence deliberately hides two things R7 says a caller
    /// must be able to see: heartbeats, which are liveness rather than data, and the
    /// order frames arrived in. This is where both are visible. The names are the
    /// response's oneof case names, which is the server's own vocabulary — the
    /// corpus's <c>mock_state_stream_heartbeat</c> records
    /// <c>frameKinds: ["snapshot", "heartbeat"]</c> and this is what produces that
    /// list.
    /// </remarks>
    public IReadOnlyList<string> FrameKinds => _reader.FrameKinds;
}

/// <summary>How one stream ended, when it ended badly.</summary>
/// <param name="Error">The failure, mapped to the typed hierarchy.</param>
/// <param name="Reconnects">How many times the stream re-opened before it failed.</param>
public sealed record StreamFailureSummary(LoamsError Error, int Reconnects);

/// <summary>
/// Reads one server stream's frames and hands the caller an async sequence of
/// messages, reconnecting from the cursor when the caller asked for it (R7).
/// </summary>
/// <remarks>
/// Not a public type: a caller sees an <see cref="IAsyncEnumerable{T}"/>, which is
/// the idiom design §44 §7.1 names for C#. Everything stateful — the cursor, the
/// reconnect count, the "yield without resuming" rule — is here so the module
/// method that returns the enumerable stays three lines long.
/// </remarks>
internal sealed class ServerStreamReader
{
    private readonly CallInvoker _invoker;
    private readonly CallBinding _binding;
    private readonly IMessage _request;
    private readonly CallOptions _options;
    private readonly IStreamResume? _resume;
    private readonly Google.Protobuf.Reflection.MessageDescriptor _responseType;

    internal ServerStreamReader(
        CallInvoker invoker,
        CallBinding binding,
        IMessage request,
        CallOptions options,
        IStreamResume? resume)
    {
        _invoker = invoker;
        _binding = binding;
        _request = request;
        _options = options;
        _resume = resume;
        _responseType = Descriptors.Find(binding.ResponseTypeName)
            ?? throw ErrorMapper.Internal(binding.Rpc,
                $"no generated message type named {binding.ResponseTypeName} for {binding.Name}");
    }

    /// <summary>How many times this stream re-opened. Useful in a test and in a log line.</summary>
    public int Reconnects { get; private set; }

    /// <summary>
    /// How many heartbeats the server sent. Counted rather than yielded: a
    /// heartbeat is liveness, and a stream that yields heartbeats to a UI is a
    /// stream that renders empty rows every fifteen seconds.
    /// </summary>
    public int Heartbeats { get; private set; }

    /// <summary>
    /// What arrived on the wire, in order, one entry per frame.
    /// </summary>
    /// <remarks>
    /// The sequence itself hides two things a caller has to be able to see: the
    /// heartbeats, which are liveness rather than data, and the order the frames
    /// arrived in. This is where both are visible, named in the server's own
    /// vocabulary — the corpus's <c>mock_state_stream_heartbeat</c> records
    /// <c>frameKinds: ["snapshot", "heartbeat"]</c>, and this is what produces that
    /// list.
    /// </remarks>
    public List<string> FrameKinds { get; } = [];

    /// <summary>
    /// The messages, reconnecting from the last applied cursor when the caller
    /// asked for it.
    /// </summary>
    /// <remarks>
    /// R7's two halves, and they interact:
    ///
    ///   - **Nothing yielded yet means resume is safe.** Re-opening from scratch
    ///     would duplicate nothing, because nothing was seen.
    ///   - **Something yielded means the re-open must carry a cursor.** Once the
    ///     caller holds a position in the stream, replaying from the start
    ///     duplicates everything they have already seen, and resuming from nothing
    ///     misses everything that changed in between. So a retryable failure after
    ///     the first message resumes from the last cursor and continues; a failure
    ///     the retry class does not cover — an <c>unimplemented</c> stream, say —
    ///     is reported rather than spun on.
    /// </remarks>
    public async IAsyncEnumerable<IMessage> ReadAsync(
        [EnumeratorCancellation] CancellationToken cancellationToken)
    {
        var cursor = StreamCursor.None;
        var yielded = false;

        // A policy that does not state a budget gets the client's, and a negative one
        // is the "unset" marker rather than "never reconnect": `StreamResume`'s
        // default is -1 precisely so a caller who did not think about it inherits
        // the client's default rather than silently getting zero reconnects.
        var reconnectBudget = _resume is null ? _invoker.MaxRetries
            : _resume.MaxRetries >= 0 ? _resume.MaxRetries
            : _invoker.MaxRetries;

        while (true)
        {
            var openRequest = _resume is null || !yielded
                ? _request
                : _resume.Resume(cursor, _request);

            IReadOnlyList<(EnvelopeFlags Flags, byte[] Payload)> frames;
            WireFailure? failure;
            try
            {
                (frames, failure) = await OpenAsync(openRequest, cancellationToken).ConfigureAwait(false);
            }
            catch (Exception thrown)
            {
                var mapped = ErrorMapper.FromException(thrown, _binding.Rpc);
                if (yielded && _resume is not null && Reconnects < reconnectBudget &&
                    RetryPolicy.IsRetryableCode(mapped.Code) && !cancellationToken.IsCancellationRequested)
                {
                    Reconnects++;
                    continue;
                }
                throw mapped;
            }

            foreach (var (flags, payload) in frames)
            {
                if (flags.HasFlag(EnvelopeFlags.Trailers) || flags.HasFlag(EnvelopeFlags.EndOfStream))
                {
                    continue;
                }
                if (flags.HasFlag(EnvelopeFlags.Compressed))
                {
                    // Reported rather than skipped: a compressed frame is a message
                    // this SDK cannot read, and passing it on as if it were data
                    // would be worse than saying so.
                    throw ErrorMapper.Internal(_binding.Rpc,
                        "the server sent a compressed frame; this SDK sends no compression and cannot read one");
                }

                var message = MessageCodec.Deserialize(_responseType, payload, _invoker.Codec);

                if (IsHeartbeat(message))
                {
                    Heartbeats++;
                    FrameKinds.Add("heartbeat");
                    continue;
                }
                FrameKinds.Add(KindOf(message));

                // The cursor is recorded as each message is yielded, so a
                // reconnect resumes from the last one the caller *has*, not the
                // last one the server sent: a frame that arrived and was not
                // yielded is a frame the caller never applied.
                if (_resume is not null)
                {
                    var next = _resume.CursorOf(message);
                    if (!next.IsEmpty)
                    {
                        cursor = next;
                    }
                }

                yielded = true;
                yield return message;
            }

            if (failure is null)
            {
                yield break;
            }

            var mappedFailure = ErrorMapper.Map(failure, _binding.Rpc);
            if (_resume is not null && yielded && Reconnects < reconnectBudget &&
                RetryPolicy.IsRetryableCode(mappedFailure.Code) && !cancellationToken.IsCancellationRequested)
            {
                Reconnects++;
                continue;
            }

            throw mappedFailure;
        }
    }

    private async Task<(IReadOnlyList<(EnvelopeFlags Flags, byte[] Payload)> Frames, WireFailure? Failure)>
        OpenAsync(IMessage request, CancellationToken cancellationToken)
    {
        var body = MessageCodec.Serialize(request, _invoker.Codec);
        var headers = await _invoker.HeadersAsync(_options, cancellationToken).ConfigureAwait(false);
        var transportRequest = new TransportRequest(_binding, body, headers);

        if (_invoker.Protocol == Protocol.Connect || _invoker.Protocol == Protocol.GrpcWeb)
        {
            using var response = await _invoker.Transport.SendStreamAsync(transportRequest, cancellationToken)
                .ConfigureAwait(false);
            var bytes = await response.Content.ReadAsByteArrayAsync(cancellationToken).ConfigureAwait(false);
            return ResponseReader.ReadStream(bytes, (int)response.StatusCode, _invoker.Protocol, _binding.Rpc);
        }

        // gRPC proper: the status is in HTTP trailers rather than in the body, so
        // the whole body is the frames and the failure comes from the headers.
        using (var response = await _invoker.Transport.SendStreamAsync(transportRequest, cancellationToken)
                   .ConfigureAwait(false))
        {
            var bytes = await response.Content.ReadAsByteArrayAsync(cancellationToken).ConfigureAwait(false);
            var trailerFailure = ReadGrpcTrailers(response.TrailingHeaders);
            if (trailerFailure is not null)
            {
                return ([], trailerFailure with { Rpc = _binding.Rpc });
            }
            return ResponseReader.ReadStream(bytes, (int)response.StatusCode, Protocol.GrpcWeb, _binding.Rpc);
        }
    }

    private static WireFailure? ReadGrpcTrailers(HttpHeaders headers)
    {
        var flattened = new Dictionary<string, string>(StringComparer.OrdinalIgnoreCase);
        foreach (var header in headers)
        {
            flattened[header.Key] = string.Join(",", header.Value);
        }
        return WireReader.GrpcWebFailure(flattened, 200, string.Empty);
    }

    /// <summary>
    /// The oneof case a message set, in the server's own vocabulary, or the message's
    /// own name when it has no oneof.
    /// </summary>
    private static string KindOf(IMessage message)
    {
        foreach (var oneof in message.Descriptor.Oneofs)
        {
            foreach (var field in oneof.Fields)
            {
                if (field.Accessor.HasValue(message))
                {
                    return field.JsonName;
                }
            }
        }
        return message.Descriptor.Name;
    }

    /// <summary>
    /// Whether a frame is a heartbeat, read from the message rather than from its
    /// size.
    /// </summary>
    /// <remarks>
    /// A heartbeat is the <c>heartbeat</c> case of the response's oneof, so a
    /// stream that declares one has exactly one way to say "nothing changed, I am
    /// still here" — and a message that carries a <c>heartbeat</c> field and
    /// nothing else is the liveness signal, not a row.
    ///
    /// The check is over the oneof cases rather than over the whole message,
    /// because a response that also carries a cursor (which
    /// <c>WatchApprovalsResponse</c> does, and the corpus's heartbeat frame does
    /// too) is still a heartbeat: the cursor is the position, not the data.
    /// Anything whose set case is <b>not</b> <c>heartbeat</c> is yielded, so
    /// <c>snapshot</c>, <c>upsert</c> and <c>remove</c> all reach the caller — and
    /// R7's <c>remove</c> case is the one an SDK that only handles upserts gets
    /// wrong, leaving a decided approval on screen forever.
    /// </remarks>
    private static bool IsHeartbeat(IMessage message)
    {
        foreach (var oneof in message.Descriptor.Oneofs)
        {
            foreach (var field in oneof.Fields)
            {
                if (field.Accessor.HasValue(message) &&
                    string.Equals(field.Name, "heartbeat", StringComparison.Ordinal))
                {
                    return true;
                }
            }
        }
        return false;
    }
}