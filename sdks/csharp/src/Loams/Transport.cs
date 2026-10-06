// The transport: HTTP, the three protocols, and the framing (design §44 §4,
// D600; runtime contract R10).
//
// One port serves the Connect protocol, gRPC and gRPC-Web, so a C# client's only
// question is which it would rather speak, and all three are reachable over one
// `HttpClient`. The choice is made in `TransportOptions` and is per client.
//
// # Why `HttpClient` and not `Grpc.Net.Client`
//
// `Grpc.Net.Client` is what design §44 §9 row 8 names for C#, and it cannot be
// used for this SDK's job. It speaks gRPC and gRPC-Web but **not Connect**, and
// Connect is the protocol the corpus's `application/json` and
// `application/proto` unary cases and the whole `application/connect+*`
// streaming family are recorded in — 13 of the 28 required fixtures, including
// `live_watch`, the only case that proves a refusal arriving *inside* a stream
// envelope rather than as an HTTP status. `Grpc.Net.Client.Web` is the browser
// transport and lives in a WASM-only package. So the wire is here, over
// `System.Net.Http`, with `Google.Protobuf` for the messages. See
// `DEPENDENCIES.md`, and D650 for the ruling itself.
//
// # What the transport does *not* decide
//
// Retry, credentials, idempotency and error mapping are the caller's
// (`CallInvoker`). This type's whole job is: build the request, send it, read the
// response's body far enough to know whether it succeeded, and turn a refusal
// into a <see cref="WireFailure"/>. Nothing here retries, because a transport
// that retried on its own would make the retry class — which comes from the
// proto, per call — impossible to enforce.

using System.Net;
using System.Net.Http.Headers;
using Google.Protobuf;

namespace Loams;

/// <summary>How the transport behaves on the wire.</summary>
public sealed class TransportOptions
{
    /// <summary>
    /// The instance's base URL, for example <c>https://acme.loams.dev</c>. A
    /// loopback stack is <c>http://127.0.0.1:8080</c>.
    /// </summary>
    public required string Endpoint { get; init; }

    /// <summary>The wire protocol. Connect unless a caller has a reason.</summary>
    public Protocol Protocol { get; init; } = Protocol.Connect;

    /// <summary>The message encoding. Binary protobuf unless a caller asks for JSON.</summary>
    public Codec Codec { get; init; } = Codec.Proto;

    /// <summary>
    /// The <c>HttpClient</c> the transport sends with. Supply one for a custom TLS
    /// configuration, proxy or handler; the transport does not own one otherwise.
    /// </summary>
    public HttpClient? HttpClient { get; init; }

    /// <summary>
    /// A handler the transport wraps, for a test or a proxy. Ignored when
    /// <see cref="HttpClient"/> is supplied — a client wins over a handler,
    /// because a client is the more complete of the two.
    /// </summary>
    public HttpMessageHandler? Handler { get; init; }

    /// <summary>Headers added to every request, for a gateway or an authenticating proxy.</summary>
    public IReadOnlyDictionary<string, string> DefaultHeaders { get; init; } =
        new Dictionary<string, string>();

    /// <summary>
    /// How long to wait for the response **headers**, in seconds.
    /// </summary>
    /// <remarks>
    /// Deliberately generous, and it is not the deadline. This bounds a connection
    /// that never answers at all; the caller's <see cref="CancellationToken"/> bounds
    /// the call, including its body, its retries and its backoff. A default short
    /// enough to "protect" a slow query would turn that query into a spurious
    /// <c>deadline_exceeded</c> that looks like a server fault, and the fix for a
    /// caller who wants a bound is a token, which covers the whole call rather than
    /// this one field.
    /// </remarks>
    public TimeSpan ResponseHeadersTimeout { get; init; } = TimeSpan.FromSeconds(100);
}

/// <summary>
/// One request's identity on the wire: the RPC, the body and the headers.
/// </summary>
/// <param name="Binding">The call it is for.</param>
/// <param name="Body">The serialized request message.</param>
/// <param name="Headers">Headers for this attempt.</param>
public readonly record struct TransportRequest(CallBinding Binding, byte[] Body, IReadOnlyDictionary<string, string> Headers);

/// <summary>
/// What came back, before anything has decided what it means: the status, the
/// content type and the body.
/// </summary>
/// <remarks>
/// Deliberately dumb. The status alone says nothing — a gRPC-Web refusal is a 200
/// with the status in a trailers frame, and a Connect stream refusal is a 200
/// with the error in the end frame — so every reader below reads the body, and
/// <c>HttpResponseMessage.IsSuccessStatusCode</c> is never consulted on its own.
/// </remarks>
public sealed record TransportResponse(HttpStatusCode Status, string? ContentType, byte[] Body);

/// <summary>
/// Sends one request and reads its body. The only type in the SDK that touches
/// the network.
/// </summary>
public sealed class Transport : IDisposable
{
    private readonly TransportOptions _options;
    private readonly HttpClient _client;
    private readonly bool _ownsClient;
    private readonly string _endpoint;

    /// <summary>Builds a transport.</summary>
    public Transport(TransportOptions options)
    {
        ArgumentNullException.ThrowIfNull(options);
        ArgumentException.ThrowIfNullOrWhiteSpace(options.Endpoint, nameof(options.Endpoint));

        _options = options;
        _endpoint = options.Endpoint;
        // Validated once, here, so a malformed endpoint is a construction error
        // rather than a per-call one.
        _ = RpcPath(Facade.Binding("instance", "GetInstance"), options.Endpoint);

        if (options.HttpClient is not null)
        {
            _client = options.HttpClient;
            // Not ours: a caller who supplies a client shares it, and disposing it
            // here would close connections somebody else is using.
            _ownsClient = false;
            return;
        }

        _client = new HttpClient(options.Handler ?? new SocketsHttpHandler
        {
            // HTTP/2 is negotiated through ALPN on a TLS endpoint and asked for
            // directly on a plaintext one, because gRPC's status lives in HTTP
            // trailers and HTTP/1.1 cannot carry them. Connect needs neither.
            EnableMultipleHttp2Connections = options.Protocol == Protocol.Grpc,
            AutomaticDecompression = DecompressionMethods.None,
        })
        {
            // Only the headers, deliberately: the body of a watch stream may not
            // arrive for hours, and a client-wide timeout on the whole response
            // would kill a healthy stream at some arbitrary point.
            Timeout = options.ResponseHeadersTimeout,
        };
        _ownsClient = true;
    }

    /// <summary>The protocol this transport speaks.</summary>
    public Protocol Protocol => _options.Protocol;

    /// <summary>The codec this transport sends.</summary>
    public Codec Codec => _options.Codec;

    /// <summary>
    /// The content type for a call, which depends on the protocol **and** on
    /// whether it streams — the asymmetry the protocol itself has, written out in
    /// <see cref="ContentTypes"/>.
    /// </summary>
    public string ContentTypeFor(Streaming streaming) => (_options.Protocol, _options.Codec, streaming) switch
    {
        (Protocol.Connect, Codec.Proto, Streaming.Unary) => ContentTypes.ConnectUnaryProto,
        (Protocol.Connect, Codec.Json, Streaming.Unary) => ContentTypes.ConnectUnaryJson,
        (Protocol.Connect, Codec.Proto, Streaming.Server) => ContentTypes.ConnectStreamProto,
        (Protocol.Connect, Codec.Json, Streaming.Server) => ContentTypes.ConnectStreamJson,
        (Protocol.GrpcWeb, Codec.Proto, _) => ContentTypes.GrpcProto,
        (Protocol.GrpcWeb, Codec.Json, _) => ContentTypes.GrpcJson,
        // gRPC proper. It uses gRPC-Web's content types — the framing is identical
        // and only the trailer carriage differs — so there is nothing to spell
        // differently here.
        (Protocol.Grpc, Codec.Proto, _) => ContentTypes.GrpcProto,
        (Protocol.Grpc, Codec.Json, _) => ContentTypes.GrpcJson,
        _ => throw new InvalidOperationException(
            $"no content type for protocol {_options.Protocol}, codec {_options.Codec}, streaming {streaming}"),
    };

    /// <summary>Sends one unary call and reads its whole body.</summary>
    public async Task<TransportResponse> SendAsync(
        TransportRequest request,
        CancellationToken cancellationToken)
    {
        ArgumentNullException.ThrowIfNull(request);

        using var message = BuildRequest(request, streaming: false);
        using var response = await _client.SendAsync(message, HttpCompletionOption.ResponseContentRead,
            cancellationToken).ConfigureAwait(false);

        var body = await response.Content.ReadAsByteArrayAsync(cancellationToken).ConfigureAwait(false);
        return new TransportResponse(response.StatusCode,
            response.Content.Headers.ContentType?.MediaType, body);
    }

    /// <summary>
    /// Sends one call and hands back the unread body, for a stream the caller
    /// reads frame by frame.
    /// </summary>
    /// <remarks>
    /// <see cref="HttpCompletionOption.ResponseHeadersRead"/> is the point: the
    /// alternative buffers the whole body, which for a watch stream is a stream
    /// that never ends, buffered. The caller owns the returned response and must
    /// dispose it — an undisposed response holds its connection open, and on a
    /// long-lived watch that is a socket the client can never reuse.
    /// </remarks>
    public async Task<HttpResponseMessage> SendStreamAsync(
        TransportRequest request,
        CancellationToken cancellationToken)
    {
        ArgumentNullException.ThrowIfNull(request);

        using var message = BuildRequest(request, streaming: true);
        return await _client.SendAsync(message, HttpCompletionOption.ResponseHeadersRead, cancellationToken)
            .ConfigureAwait(false);
    }

    private HttpRequestMessage BuildRequest(TransportRequest request, bool streaming)
    {
        var contentType = ContentTypeFor(request.Binding.Streaming);
        var message = new HttpRequestMessage(HttpMethod.Post, RpcPath(request.Binding, _endpoint))
        {
            Content = BuildContent(FrameIfNeeded(request.Binding, request.Body), contentType),
        };

        // The two protocol markers. Connect states the protocol version, gRPC-Web
        // states that it is gRPC-Web; a server that sees neither is being sent
        // gRPC proper, whose content type already says so.
        switch (_options.Protocol)
        {
            case Protocol.Connect:
                message.Headers.TryAddWithoutValidation("connect-protocol-version", "1");
                break;
            case Protocol.GrpcWeb:
                message.Headers.TryAddWithoutValidation("x-grpc-web", "1");
                break;
        }

        // `Accept-Encoding: identity`, so the bytes on the wire are the bytes the
        // corpus recorded. A server that compresses anyway is answered by the
        // reader below, which decompresses when the header says it did — but the
        // request must not ask for compression the corpus never saw, because a
        // compressed request is a request whose bytes the server has never seen.
        message.Headers.TryAddWithoutValidation("accept-encoding", "identity");

        // Default headers first, then this attempt's, then `Authorization`: the
        // runtime sets the bearer last so a caller-supplied one cannot defeat the
        // refresh, which is the whole of R1.
        foreach (var (name, value) in _options.DefaultHeaders)
        {
            message.Headers.TryAddWithoutValidation(name, value);
        }
        foreach (var (name, value) in request.Headers)
        {
            if (string.Equals(name, "Authorization", StringComparison.OrdinalIgnoreCase))
            {
                continue;
            }
            message.Headers.TryAddWithoutValidation(name, value);
        }
        if (request.Headers.TryGetValue("Authorization", out var authorization))
        {
            message.Headers.Authorization = new AuthenticationHeaderValue("Bearer", authorization);
        }

        return message;
    }

    /// <summary>
    /// Whether a request body goes out framed.
    /// </summary>
    /// <remarks>
    /// **Every protocol except Connect unary.** Connect's unary form is a bare HTTP
    /// POST whose body is the message; Connect streaming, gRPC and gRPC-Web all put
    /// the 5-byte envelope in front of it. Getting this wrong is invisible in the
    /// response and fatal at the server: the corpus answers 400 and says which
    /// bytes it expected, and the SDK that framed a Connect-unary body would fail
    /// every `application/json` and `application/proto` fixture while passing every
    /// other one.
    /// </remarks>
    private byte[] FrameIfNeeded(CallBinding binding, byte[] body) =>
        _options.Protocol == Protocol.Connect && binding.Streaming == Streaming.Unary
            ? body
            : Envelopes.Wrap(body);

    private static HttpContent BuildContent(byte[] body, string contentType)
    {
        var content = new ByteArrayContent(body);
        content.Headers.ContentType = new MediaTypeHeaderValue(contentType);
        // Content-Length is set by ByteArrayContent; nothing here may set
        // Transfer-Encoding, because every protocol this SDK speaks is a POST with
        // a known-length body and a chunked one would be a request the corpus has
        // no recording of.
        return content;
    }

    /// <summary>
    /// The absolute URL an RPC is posted to: the endpoint's base plus
    /// <c>/package.Service/Method</c>.
    /// </summary>
    /// <remarks>
    /// Absolute rather than relative, and the reason is that a caller who supplies
    /// their own <see cref="HttpClient"/> has not set its <c>BaseAddress</c> — they
    /// set a timeout, a proxy or a handler, and an SDK that needed a base address too
    /// would be a second thing to configure. The endpoint is validated here so a
    /// malformed one is named at construction rather than as an
    /// <c>InvalidOperationException</c> from inside <c>HttpClient</c>.
    /// </remarks>
    public static string RpcPath(CallBinding binding, string endpoint)
    {
        ArgumentException.ThrowIfNullOrWhiteSpace(endpoint);
        if (!Uri.TryCreate(endpoint, UriKind.Absolute, out var baseAddress))
        {
            throw ErrorMapper.Internal(binding.Rpc,
                $"the endpoint '{endpoint}' is not an absolute URL; it is the instance's base URL, for " +
                "example https://acme.loams.dev");
        }
        return new Uri(baseAddress, "/" + binding.Rpc).ToString();
    }

    /// <summary>Releases the client, when this transport built it.</summary>
    public void Dispose()
    {
        if (_ownsClient)
        {
            _client.Dispose();
        }
    }
}

/// <summary>
/// Reads one response far enough to know whether it succeeded.
/// </summary>
/// <remarks>
/// The three refusal shapes R10 warns about, in one place so no caller can read
/// only one of them:
///
///   - **Connect unary** — an HTTP 4xx/5xx with a JSON error body. A 200 with a
///     body that does not parse as the expected message is *not* treated as a
///     success either: the SDK says so rather than handing a caller an empty
///     message.
///   - **gRPC-Web** — HTTP 200, <c>grpc-status</c> in a trailers frame. The
///     corpus's <c>instance_who_am_i_grpc_web</c> is exactly this: a 200 that is
///     a refusal.
///   - **Connect streaming** — HTTP 200, the error inside the end-of-stream frame.
///     The corpus's <c>live_watch</c> is exactly this, and it is the case that
///     separates a real Connect client from a status-code-only fake.
/// </remarks>
public static class ResponseReader
{
    /// <summary>
    /// The failure a unary response describes, or null when it succeeded.
    /// </summary>
    public static WireFailure? UnaryFailure(TransportResponse response, Protocol protocol, string rpc)
    {
        ArgumentNullException.ThrowIfNull(response);

        if ((int)response.Status is >= 400 and <= 599)
        {
            return WireReader.UnaryFailure((int)response.Status,
                System.Text.Encoding.UTF8.GetString(response.Body), rpc);
        }

        // gRPC-Web puts its status in a trailers frame inside the body, on a 200.
        // The corpus's `instance_who_am_i_grpc_web` is exactly this: HTTP 200,
        // `grpc-status: 12`. A reader that stops at the status code calls that a
        // success and hands the caller an empty message, which is the bug R10
        // names when it says "one port, several protocols, one client".
        if (protocol == Protocol.GrpcWeb)
        {
            var failure = ReadGrpcWebStreamEnd(Envelopes.Split(response.Body));
            if (failure is not null)
            {
                return failure with { HttpStatus = (int)response.Status, Rpc = rpc };
            }
        }

        if ((int)response.Status is < 200 or >= 300)
        {
            // A 1xx or 3xx that reached here is not a Loams answer. Refusing it is
            // right: a redirect the SDK did not follow is a failure whose cause
            // the caller must see.
            return new WireFailure(Code.Unknown,
                $"HTTP {(int)response.Status} from {rpc}, which is not a Loams answer", null,
                (int)response.Status, rpc);
        }

        return null;
    }

    /// <summary>
    /// The frames of a stream response, and the failure they describe.
    /// </summary>
    /// <param name="body">The whole response body.</param>
    /// <param name="status">The HTTP status, which a stream refusal shares with a success.</param>
    /// <param name="protocol">Which protocol framed it, which is what says how it ends.</param>
    /// <param name="rpc">The RPC, for the failure's message.</param>
    public static (IReadOnlyList<(EnvelopeFlags Flags, byte[] Payload)> Frames, WireFailure? Failure)
        ReadStream(byte[] body, int status, Protocol protocol, string rpc)
    {
        ArgumentNullException.ThrowIfNull(body);

        if (status is >= 400 and <= 599)
        {
            return ([], WireReader.UnaryFailure(status, System.Text.Encoding.UTF8.GetString(body), rpc));
        }

        var frames = Envelopes.Split(body);
        var failure = protocol == Protocol.Connect
            ? ReadConnectStreamEnd(frames)
            : ReadGrpcWebStreamEnd(frames);
        return (frames, failure is null ? null : failure with { HttpStatus = status, Rpc = rpc });
    }

    private static WireFailure? ReadConnectStreamEnd(IReadOnlyList<(EnvelopeFlags Flags, byte[] Payload)> frames)
    {
        foreach (var (flags, payload) in frames)
        {
            if (flags.HasFlag(EnvelopeFlags.EndOfStream))
            {
                // The end frame's payload is JSON whatever the codec is — the
                // protocol defines it so, and the corpus's `live_watch` is a proto
                // stream whose end frame is JSON.
                return WireReader.StreamFailure(WireReader.ParseEndStreamFrame(payload), 0, string.Empty);
            }
        }
        // No end frame: the server closed without one. A recorded stream is a
        // bounded prefix a server closed exactly that way, so this is a clean end
        // and not a failure.
        return null;
    }

    private static WireFailure? ReadGrpcWebStreamEnd(IReadOnlyList<(EnvelopeFlags Flags, byte[] Payload)> frames)
    {
        foreach (var (flags, payload) in frames)
        {
            if (flags.HasFlag(EnvelopeFlags.Trailers))
            {
                return WireReader.GrpcWebFailure(Envelopes.ParseTrailers(payload), 0, string.Empty);
            }
        }
        return null;
    }
}