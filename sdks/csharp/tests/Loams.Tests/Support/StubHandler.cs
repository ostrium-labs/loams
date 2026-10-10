// A scripted `HttpMessageHandler`, so a test can drive the SDK's **real** transport.
//
// # Why not a stubbed transport interface
//
// The SDK's transport is `HttpMessageHandler`-shaped all the way down, so a test can
// substitute a handler and exercise the genuine path: the request is really
// serialized by `MessageCodec`, really framed, really put in headers, and the
// response is really parsed back. A stub at the SDK's own transport seam would skip
// the codec, the framing and the header logic — which is most of what a conformance
// fixture is about.
//
// What the handler replaces is the socket and nothing else.

using System.Net;
using System.Text;
using Google.Protobuf;
using Loams;

namespace Loams.Tests.Support;

/// <summary>One scripted reply.</summary>
/// <param name="Status">The HTTP status.</param>
/// <param name="Body">The body, as a byte array already framed if it is a stream.</param>
/// <param name="ContentType">The response content type.</param>
/// <param name="Trailers">
/// HTTP trailers, for gRPC proper. gRPC-Web puts them in a frame and Connect puts
/// them in the end frame, neither of which is an HTTP trailer.
/// </param>
public sealed record StubReply(
    HttpStatusCode Status,
    byte[] Body,
    string? ContentType = null,
    IReadOnlyDictionary<string, string>? Trailers = null);

/// <summary>
/// One request the SDK made, as the handler saw it.
/// </summary>
/// <param name="Path">The request path, e.g. <c>/loams.instance.v1.InstanceService/GetInstance</c>.</param>
/// <param name="ContentType">The content type the SDK chose, which is the thing the corpus is keyed on.</param>
/// <param name="Body">The body bytes, undecoded.</param>
/// <param name="Headers">Every header, with the bearer under <c>Authorization</c>.</param>
public sealed record RecordedRequest(
    string Path,
    string? ContentType,
    byte[] Body,
    IReadOnlyDictionary<string, string> Headers)
{
    /// <summary>The bearer the SDK sent, without the <c>Bearer </c> prefix.</summary>
    public string? Bearer =>
        Headers.TryGetValue("Authorization", out var value) && value.StartsWith("Bearer ", StringComparison.Ordinal)
            ? value["Bearer ".Length..]
            : Headers.TryGetValue("Authorization", out var bare) ? bare : null;
}

/// <summary>
/// A handler that answers from a script and records what it was asked.
/// </summary>
/// <remarks>
/// Thread-safe on purpose: a test that opens a hundred streams at once would
/// otherwise get a torn list of requests, and the failures would be about the test's
/// concurrency rather than about the SDK.
/// </remarks>
public sealed class StubHandler : HttpMessageHandler
{
    private readonly List<StubReply> _replies;
    private readonly object _gate = new();
    private readonly List<RecordedRequest> _requests = [];

    /// <summary>Builds a handler over a fixed script.</summary>
    /// <remarks>
    /// The last reply repeats once the script runs out, because a test that asked for
    /// two attempts and got three has a bug the test should see as a message rather
    /// than as an <c>IndexOutOfRangeException</c> from the handler.
    /// </remarks>
    public StubHandler(params StubReply[] replies)
    {
        _replies = [.. replies];
        if (_replies.Count == 0)
        {
            throw new ArgumentException("a stub handler needs at least one reply", nameof(replies));
        }
    }

    /// <summary>Every request the SDK made, in order.</summary>
    public IReadOnlyList<RecordedRequest> Requests
    {
        get
        {
            lock (_gate)
            {
                return [.. _requests];
            }
        }
    }

    /// <summary>How many requests the SDK made.</summary>
    public int CallCount
    {
        get
        {
            lock (_gate)
            {
                return _requests.Count;
            }
        }
    }

    /// <inheritdoc/>
    protected override async Task<HttpResponseMessage> SendAsync(
        HttpRequestMessage request,
        CancellationToken cancellationToken)
    {
        ArgumentNullException.ThrowIfNull(request);

        var body = request.Content is null
            ? []
            : await request.Content.ReadAsByteArrayAsync(cancellationToken).ConfigureAwait(false);

        var headers = new Dictionary<string, string>(StringComparer.OrdinalIgnoreCase);
        foreach (var header in request.Headers)
        {
            headers[header.Key] = string.Join(",", header.Value);
        }
        if (request.Content is not null)
        {
            foreach (var header in request.Content.Headers)
            {
                headers[header.Key] = string.Join(",", header.Value);
            }
        }

        StubReply reply;
        lock (_gate)
        {
            _requests.Add(new RecordedRequest(
                request.RequestUri?.AbsolutePath ?? string.Empty,
                request.Content?.Headers.ContentType?.MediaType,
                body,
                headers));
            reply = _replies[Math.Min(_requests.Count - 1, _replies.Count - 1)];
        }

        var response = new HttpResponseMessage(reply.Status)
        {
            Content = new ByteArrayContent(reply.Body),
            RequestMessage = request,
        };
        if (reply.ContentType is { Length: > 0 })
        {
            response.Content.Headers.ContentType = new System.Net.Http.Headers.MediaTypeHeaderValue(reply.ContentType);
        }
        if (reply.Trailers is not null)
        {
            foreach (var (name, value) in reply.Trailers)
            {
                response.TrailingHeaders.TryAddWithoutValidation(name, value);
            }
        }
        return response;
    }

    // ---- builders, so a test reads as the scenario it is describing ----

    /// <summary>A Connect unary JSON body, as a server would answer it.</summary>
    public static StubReply ConnectJson(HttpStatusCode status, string json) =>
        new(status, Encoding.UTF8.GetBytes(json), Loams.ContentTypes.ConnectUnaryJson);

    /// <summary>
    /// A Connect unary refusal: an HTTP status with the JSON error envelope, which is
    /// what the corpus's Connect cases are.
    /// </summary>
    /// <remarks>
    /// The <c>details[]</c> entry carries the type name **and** base64 protobuf, both
    /// present, because an SDK that reads one of them to decide what it is holding has
    /// to find that one there.
    /// </remarks>
    public static StubReply ConnectRefusal(HttpStatusCode status, string code, string message, string? reason,
        IReadOnlyDictionary<string, string>? metadata = null) =>
        ConnectJson(status, ErrorBody(code, message, reason, metadata));

    /// <summary>The Connect JSON error envelope, as <c>connect-rust</c> shapes one.</summary>
    public static string ErrorBody(string code, string message, string? reason,
        IReadOnlyDictionary<string, string>? metadata = null)
    {
        var details = reason is null
            ? string.Empty
            : $",\"details\":[{{\"type\":\"{Loams.ErrorInfoCodec.ErrorInfoType}\",\"value\":\"{ErrorInfoBase64(reason, metadata)}\"}}]";
        return $"{{\"code\":\"{code}\",\"message\":\"{message.Replace("\"", "\\\"", StringComparison.Ordinal)}\"{details}}}";
    }

    /// <summary>A serialized <c>loams.errors.v1.ErrorInfo</c>, base64 of the bytes.</summary>
    public static string ErrorInfoBase64(string reason, IReadOnlyDictionary<string, string>? metadata = null)
    {
        var info = new Loams.Errors.V1.ErrorInfo { Reason = reason };
        if (metadata is not null)
        {
            // Copied in: `MapField.Add` takes an `IDictionary`, and a caller's
            // `IReadOnlyDictionary` is not one.
            foreach (var (key, value) in metadata)
            {
                info.Metadata[key] = value;
            }
        }
        return Convert.ToBase64String(info.ToByteArray());
    }

    /// <summary>A gRPC-Web trailers-only frame: HTTP 200 with the status in the body.</summary>
    public static StubReply GrpcWebTrailers(int status, string message, string? reason,
        IReadOnlyDictionary<string, string>? metadata = null)
    {
        var trailers = new List<string> { $"grpc-status: {status}", $"grpc-message: {message}" };
        if (reason is not null)
        {
            trailers.Add($"grpc-status-details-bin: {StatusDetailsBase64(status, message, reason, metadata)}");
        }
        return new StubReply(HttpStatusCode.OK,
            Envelopes.Wrap(Encoding.UTF8.GetBytes(string.Join("\r\n", trailers) + "\r\n"), EnvelopeFlags.Trailers),
            Loams.ContentTypes.GrpcProto);
    }

    /// <summary>
    /// A base64 <c>google.rpc.Status</c> carrying one <c>ErrorInfo</c> in an
    /// <c>Any</c>, which is what <c>grpc-status-details-bin</c> holds.
    /// </summary>
    /// <remarks>
    /// Written out by hand rather than through a generated <c>Status</c> type, because
    /// no proto in this repository declares one and the three fields the SDK reads —
    /// <c>code</c>, <c>message</c>, <c>details</c> — are a dozen lines of protobuf
    /// wire format.
    /// </remarks>
    public static string StatusDetailsBase64(int status, string message, string reason,
        IReadOnlyDictionary<string, string>? metadata = null)
    {
        var info = new Loams.Errors.V1.ErrorInfo { Reason = reason };
        if (metadata is not null)
        {
            foreach (var (key, value) in metadata)
            {
                info.Metadata[key] = value;
            }
        }
        var any = Concat(
            Tag(1, 2), Varint((ulong)Loams.ErrorInfoCodec.ErrorInfoTypeUrl.Length),
            Encoding.UTF8.GetBytes(Loams.ErrorInfoCodec.ErrorInfoTypeUrl),
            Tag(2, 2), Varint((ulong)info.ToByteArray().Length), info.ToByteArray());

        var code = Varint((ulong)status);
        var text = Encoding.UTF8.GetBytes(message);
        return Convert.ToBase64String(Concat(
            Tag(1, 0), code,
            Tag(2, 2), Varint((ulong)text.Length), text,
            Tag(3, 2), Varint((ulong)any.Length), any));
    }

    /// <summary>
    /// A Connect stream whose last frame is the end-of-stream error.
    /// </summary>
    /// <remarks>
    /// Built by composing the error body with <c>fixture-server.mjs</c>'s shape
    /// rather than by string surgery: the envelope nests the error one level down
    /// from a unary body, and getting that nesting wrong produces a frame that
    /// parses as clean and therefore a stream that reports a refusal as a success.
    /// </remarks>
    public static StubReply ConnectStreamRefusal(string code, string message, string? reason,
        IReadOnlyDictionary<string, string>? metadata = null)
    {
        var inner = new System.Text.StringBuilder();
        inner.Append('{').Append("\"code\":\"").Append(code).Append('"')
            .Append(",\"message\":\"").Append(message.Replace("\"", "\\\"", StringComparison.Ordinal)).Append('"');
        if (reason is not null)
        {
            inner.Append(",\"details\":[{\"type\":\"").Append(Loams.ErrorInfoCodec.ErrorInfoType)
                .Append("\",\"value\":\"").Append(ErrorInfoBase64(reason, metadata)).Append("\"}]");
        }
        inner.Append('}');

        var frame = $"{{\"error\":{inner}}}";
        return new StubReply(HttpStatusCode.OK,
            Envelopes.Wrap(Encoding.UTF8.GetBytes(frame), EnvelopeFlags.EndOfStream),
            Loams.ContentTypes.ConnectStreamProto);
    }

    /// <summary>One framed message, as a stream sends it.</summary>
    public static byte[] Frame(byte[] payload, EnvelopeFlags flags = EnvelopeFlags.None) =>
        Envelopes.Wrap(payload, flags);

    private static byte[] Tag(int field, int wire) => [(byte)((field << 3) | wire)];

    private static byte[] Varint(ulong value)
    {
        var bytes = new List<byte>();
        while (value >= 0x80)
        {
            bytes.Add((byte)(value | 0x80));
            value >>= 7;
        }
        bytes.Add((byte)value);
        return [.. bytes];
    }

    private static byte[] Concat(params byte[][] parts)
    {
        var total = parts.Sum((part) => part.Length);
        var joined = new byte[total];
        var at = 0;
        foreach (var part in parts)
        {
            part.CopyTo(joined, at);
            at += part.Length;
        }
        return joined;
    }
}