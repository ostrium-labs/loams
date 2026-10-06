// The wire: encodings, framing, and the parts of a response the SDK has to read
// before it can say anything about a call (design §44 §4, R10).
//
// One port serves the Connect protocol, gRPC and gRPC-Web (D600), so the only
// question a C# client has is which of those it would rather speak. All three
// are spoken here over `HttpClient`, and the choice is two enums: a
// <see cref="Protocol"/> and a <see cref="Codec"/>. There is no separate gRPC
// transport type and no HTTP/2-only path, because C# has no official Connect
// library (D612) and `Grpc.Net.Client` speaks neither Connect nor gRPC-Web over
// HTTP/1.1 — see DEPENDENCIES.md and D641.
//
// # What is shared and what is not
//
// Framing is shared: Connect streaming and gRPC-Web both put a 5-byte header in
// front of every message, and the header means the same thing in both. So
// <see cref="Envelope"/> is one type and one reader, and the difference between
// the two protocols is entirely in how the **end** of a response arrives —
//
//   - Connect streaming: a final frame with the end-of-stream flag, whose
//     payload is a JSON <c>EndStreamResponse</c> whether the codec is proto or
//     JSON (this is specified, and the corpus depends on it);
//   - gRPC-Web: a trailers-only frame with the high flag bit, carrying
//     `grpc-status`, `grpc-message` and `grpc-status-details-bin`.
//
// Everything else — the error envelope, `ErrorInfo`, the codes — is shared too,
// and lives in `Errors.cs`.
//
// # Where a failure can hide
//
// The clause this file exists for is R10, and its sharpest edge is that **a
// refusal is often not an HTTP status**. All three of these are refusals that a
// status-code-only client reads as success:
//
//   - Connect unary: HTTP 501 with a JSON error body.
//   - gRPC-Web: HTTP 200, `grpc-status: 12` in a trailers frame.
//   - Connect streaming: HTTP 200, an error inside the end-of-stream frame.
//
// `IsRetryableStatus` therefore refuses to treat a bare 200 as proof of
// anything, and every path below parses the body before it decides.

using System.Buffers;
using System.Text;
using Google.Protobuf;

namespace Loams;

/// <summary>The wire protocol a client speaks. All three are served on one port.</summary>
public enum Protocol
{
    /// <summary>
    /// The Connect protocol. The default: it is the only one that works
    /// identically over HTTP/1.1 and HTTP/2, its unary form is an HTTP POST with
    /// a JSON body (which is what <c>curl</c> sends, design §44 §4), and its
    /// server streaming is a plain chunked HTTP response.
    /// </summary>
    Connect = 0,

    /// <summary>
    /// gRPC over HTTP/2. Served on the same port, and what a proxy that only
    /// speaks gRPC needs. Requires HTTP/2: the status is in HTTP trailers, which
    /// HTTP/1.1 cannot carry.
    /// </summary>
    Grpc = 1,

    /// <summary>
    /// gRPC-Web: gRPC's framing with the trailers moved into the body, which is
    /// what makes it work over HTTP/1.1 and from a browser with no proxy (R10).
    /// </summary>
    GrpcWeb = 2,
}

/// <summary>The message encoding on the wire.</summary>
public enum Codec
{
    /// <summary>
    /// Binary protobuf. What an SDK sends by default, and what the corpus calls
    /// the encoding "an SDK sends by default".
    /// </summary>
    Proto = 0,

    /// <summary>
    /// The proto3 JSON mapping, which is what <c>curl</c> sends and what a
    /// browser SDK asks for. Also what an error body always is, whatever the
    /// codec.
    /// </summary>
    Json = 1,
}

/// <summary>
/// The content types this SDK sends and accepts (R10, and
/// <c>sdks/conformance/encodings.mjs</c> for the families the corpus keys on).
/// </summary>
/// <remarks>
/// They are spelled out rather than composed from a base and a suffix, because
/// the corpus is keyed on the exact strings and a helper that composes them would
/// be one more thing to keep in step with <c>fixture-server.mjs</c>'s
/// <c>family()</c>. Note the asymmetry the protocol itself has: Connect
/// <b>unary</b> uses <c>application/json</c> and <c>application/proto</c>, while
/// Connect <b>streaming</b> uses <c>application/connect+json</c> and
/// <c>application/connect+proto</c>. gRPC-Web uses one type for both.
/// </remarks>
public static class ContentTypes
{
    /// <summary>Connect unary, JSON.</summary>
    public const string ConnectUnaryJson = "application/json";

    /// <summary>Connect unary, binary protobuf.</summary>
    public const string ConnectUnaryProto = "application/proto";

    /// <summary>Connect server-streaming, JSON.</summary>
    public const string ConnectStreamJson = "application/connect+json";

    /// <summary>Connect server-streaming, binary protobuf.</summary>
    public const string ConnectStreamProto = "application/connect+proto";

    /// <summary>gRPC and gRPC-Web, binary protobuf.</summary>
    public const string GrpcProto = "application/grpc-web+proto";

    /// <summary>gRPC and gRPC-Web, JSON.</summary>
    public const string GrpcJson = "application/grpc-web+json";
}

/// <summary>
/// The 5-byte header in front of every message on a Connect stream or a gRPC-Web
/// call: one flag byte and a big-endian length.
/// </summary>
/// <param name="Flags">The flag byte.</param>
/// <param name="Length">The payload length in bytes.</param>
public readonly record struct Envelope(EnvelopeFlags Flags, int Length)
{
    /// <summary>The header's own size, so a reader can skip it.</summary>
    public const int HeaderLength = 5;
}

/// <summary>The flags in an envelope's first byte.</summary>
[Flags]
public enum EnvelopeFlags : byte
{
    /// <summary>An ordinary message frame. What a caller sees as data.</summary>
    None = 0,

    /// <summary>
    /// The payload is compressed. Nothing in this SDK compresses a request and
    /// nothing in the corpus compresses a response, so a compressed frame is
    /// reported rather than silently mis-parsed.
    /// </summary>
    Compressed = 0b0000_0001,

    /// <summary>
    /// The end-of-stream frame. Its payload is a JSON <c>EndStreamResponse</c>
    /// whatever the codec is — the protocol defines it that way, and the corpus's
    /// <c>live_watch</c> case is exactly a proto stream whose end frame is JSON.
    /// </summary>
    EndOfStream = 0b0000_0010,

    /// <summary>
    /// The trailers frame, gRPC-Web only. The high bit, so it cannot collide with
    /// a Connect flag.
    /// </summary>
    Trailers = 0b1000_0000,
}

/// <summary>
/// Reads and writes the 5-byte envelope. Both directions, and no state: a
/// caller owns the stream and decides when the next frame starts.
/// </summary>
public static class Envelopes
{
    /// <summary>Frames a payload as one message.</summary>
    public static byte[] Wrap(ReadOnlySpan<byte> payload, EnvelopeFlags flags = EnvelopeFlags.None)
    {
        var framed = new byte[Envelope.HeaderLength + payload.Length];
        framed[0] = (byte)flags;
        WriteBigEndian(framed.AsSpan(1, 4), payload.Length);
        payload.CopyTo(framed.AsSpan(Envelope.HeaderLength));
        return framed;
    }

    /// <summary>Concatenates several payloads into one framed buffer.</summary>
    public static byte[] WrapAll(IEnumerable<(ReadOnlyMemory<byte> Payload, EnvelopeFlags Flags)> frames)
    {
        var buffer = new ArrayBufferWriter<byte>();
        foreach (var (payload, flags) in frames)
        {
            buffer.Write(Wrap(payload.Span, flags));
        }
        return buffer.WrittenSpan.ToArray();
    }

    /// <summary>
    /// Splits a body into its frames. A truncated tail is reported rather than
    /// dropped: a stream that lost its last frame is a stream whose last message
    /// is unknown, and answering as if it had ended cleanly would report a
    /// success the caller never got.
    /// </summary>
    /// <exception cref="InvalidDataException">The body ends mid-frame.</exception>
    public static IReadOnlyList<(EnvelopeFlags Flags, byte[] Payload)> Split(ReadOnlySpan<byte> body)
    {
        var frames = new List<(EnvelopeFlags, byte[])>();
        var at = 0;
        while (at + Envelope.HeaderLength <= body.Length)
        {
            var flags = (EnvelopeFlags)body[at];
            var length = ReadBigEndian(body.Slice(at + 1, 4));
            if (length < 0 || at + Envelope.HeaderLength + length > body.Length)
            {
                throw new InvalidDataException(
                    $"the stream's frame at offset {at} declares {length} bytes and only " +
                    $"{body.Length - at - Envelope.HeaderLength} are left");
            }
            frames.Add((flags, body.Slice(at + Envelope.HeaderLength, length).ToArray()));
            at += Envelope.HeaderLength + length;
        }
        if (at != body.Length)
        {
            throw new InvalidDataException(
                $"the stream ended with {body.Length - at} bytes that are not a whole frame");
        }
        return frames;
    }

    /// <summary>Reads a trailers frame's text into a dictionary, last value wins.</summary>
    public static IReadOnlyDictionary<string, string> ParseTrailers(byte[] payload)
    {
        var trailers = new Dictionary<string, string>(StringComparer.OrdinalIgnoreCase);
        foreach (var line in Encoding.UTF8.GetString(payload).Split("\r\n", StringSplitOptions.RemoveEmptyEntries))
        {
            var colon = line.IndexOf(':');
            // A line with no colon is not a trailer. Skipping it is right: the
            // frame is text, and refusing to read a frame because of one line
            // would turn a malformed trailer into a lost status.
            if (colon > 0)
            {
                trailers[line[..colon].Trim()] = line[(colon + 1)..].Trim();
            }
        }
        return trailers;
    }

    private static void WriteBigEndian(Span<byte> at, int value)
    {
        at[0] = (byte)(value >> 24);
        at[1] = (byte)(value >> 16);
        at[2] = (byte)(value >> 8);
        at[3] = (byte)value;
    }

    private static int ReadBigEndian(ReadOnlySpan<byte> at) =>
        (at[0] << 24) | (at[1] << 16) | (at[2] << 8) | at[3];
}

/// <summary>
/// The <c>EndStreamResponse</c> a Connect stream's last frame carries. It is JSON
/// **whatever the codec is**, which is why this is read with
/// <c>System.Text.Json</c> and not with a generated message.
/// </summary>
/// <param name="Error">The refusal, when the stream ended in one.</param>
public sealed record EndStreamResponse(ConnectErrorBody? Error);

/// <summary>
/// A Connect protocol error body: what a unary refusal and a stream's end frame
/// both carry.
/// </summary>
/// <remarks>
/// This is the shape the corpus's Connect encodings produce
/// (<c>{"code", "message", "details": [{"type", "value"}]}</c>) and the same one
/// <c>connect-rust</c> and <c>connect-go</c> write. <c>value</c> is base64 of the
/// serialized detail, and <c>type</c> is the detail's type name — both present,
/// because an SDK that reads <c>details[0].type</c> to decide what it is holding
/// has to find the type there.
/// </remarks>
/// <param name="Code">The Connect code, as its <c>snake_case</c> wire name.</param>
/// <param name="Message">The human-readable message, which no branch reads.</param>
/// <param name="Details">The detail list, as it arrived.</param>
public sealed record ConnectErrorBody(string Code, string Message, IReadOnlyList<ConnectDetail> Details);

/// <summary>One detail of a Connect error body.</summary>
/// <param name="Type">The detail's type name, e.g. <c>loams.errors.v1.ErrorInfo</c>.</param>
/// <param name="Value">Base64 of the serialized detail.</param>
public sealed record ConnectDetail(string Type, string Value);

/// <summary>
/// Turns the shapes above into the <see cref="WireFailure"/> the mapper reads.
/// </summary>
/// <remarks>
/// Split out from the transport so the parsing is testable without a socket, and
/// so there is exactly one place that knows a refusal can arrive three ways.
/// </remarks>
public static class WireReader
{
    /// <summary>
    /// Parses a Connect unary error body (HTTP 4xx or 5xx with a JSON body).
    /// </summary>
    /// <remarks>
    /// A body that is not a Connect error envelope becomes a
    /// <see cref="Code.Unknown"/> failure carrying the raw text, rather than an
    /// exception: a proxy that answers <c>502 Bad Gateway</c> with an HTML page
    /// is a failure the caller must see as a failure, and its code really is
    /// unknown.
    /// </remarks>
    public static WireFailure UnaryFailure(int httpStatus, string body, string rpc)
    {
        if (TryParseConnectErrorBody(body, out var parsed))
        {
            var info = ErrorInfoFrom(parsed.Details);
            return new WireFailure(CodeFromWire(parsed.Code), parsed.Message, info, httpStatus, rpc);
        }
        return new WireFailure(Code.Unknown, Trim(body), null, httpStatus, rpc);
    }

    /// <summary>Parses a Connect error body, or returns false when it is not one.</summary>
    public static bool TryParseConnectErrorBody(string body, out ConnectErrorBody parsed)
    {
        parsed = default!;
        if (string.IsNullOrWhiteSpace(body))
        {
            return false;
        }
        try
        {
            using var document = System.Text.Json.JsonDocument.Parse(body);
            var root = document.RootElement;
            if (root.ValueKind != System.Text.Json.JsonValueKind.Object ||
                !root.TryGetProperty("code", out var code) ||
                code.ValueKind != System.Text.Json.JsonValueKind.String)
            {
                return false;
            }
            var message = root.TryGetProperty("message", out var m) && m.ValueKind == System.Text.Json.JsonValueKind.String
                ? m.GetString() ?? string.Empty
                : string.Empty;
            var details = new List<ConnectDetail>();
            if (root.TryGetProperty("details", out var list) &&
                list.ValueKind == System.Text.Json.JsonValueKind.Array)
            {
                foreach (var item in list.EnumerateArray())
                {
                    if (item.ValueKind != System.Text.Json.JsonValueKind.Object)
                    {
                        continue;
                    }
                    var type = item.TryGetProperty("type", out var t) ? t.GetString() ?? string.Empty : string.Empty;
                    var value = item.TryGetProperty("value", out var v) ? v.GetString() ?? string.Empty : string.Empty;
                    details.Add(new ConnectDetail(type, value));
                }
            }
            parsed = new ConnectErrorBody(code.GetString()!, message, details);
            return true;
        }
        catch (System.Text.Json.JsonException)
        {
            return false;
        }
    }

    /// <summary>
    /// The <c>ErrorInfo</c> a Connect detail list carries, looked up <b>by
    /// type</b> and never by position.
    /// </summary>
    public static ErrorInfoShape? ErrorInfoFrom(IReadOnlyList<ConnectDetail> details)
    {
        foreach (var detail in details)
        {
            if (!detail.Type.EndsWith(ErrorInfoCodec.ErrorInfoType, StringComparison.Ordinal))
            {
                continue;
            }
            Span<byte> bytes;
            try
            {
                bytes = Convert.FromBase64String(detail.Value);
            }
            catch (FormatException)
            {
                // Base64 the server did not write. "No ErrorInfo" is the honest
                // answer; the alternative is a Loams failure with no reason.
                continue;
            }
            var decoded = ErrorInfoCodec.Decode(bytes);
            if (decoded is not null)
            {
                return decoded;
            }
        }
        return null;
    }

    /// <summary>
    /// The failure a gRPC-Web trailers frame describes, or null when it says the
    /// call succeeded.
    /// </summary>
    /// <remarks>
    /// A trailers frame with no <c>grpc-status</c> is treated as success, which is
    /// what gRPC-Web itself does: the absence of a status on a 200 is a status of
    /// zero. A frame whose status is non-zero and whose details do not decode
    /// still produces a typed failure with the code and the message — the class is
    /// the part a caller can always act on, and losing it would turn a refusal
    /// into an unknown.
    /// </remarks>
    public static WireFailure? GrpcWebFailure(IReadOnlyDictionary<string, string> trailers, int httpStatus, string rpc)
    {
        if (!trailers.TryGetValue("grpc-status", out var raw))
        {
            return null;
        }
        if (!int.TryParse(raw, out var status))
        {
            return new WireFailure(Code.Unknown, raw, null, httpStatus, rpc);
        }
        if (status == 0)
        {
            return null;
        }
        var message = trailers.TryGetValue("grpc-message", out var m) ? Uri.UnescapeDataString(m) : string.Empty;
        var detail = trailers.TryGetValue("grpc-status-details-bin", out var bin) &&
                     !string.IsNullOrEmpty(bin)
            ? ErrorInfoCodec.FromStatusBytes(Convert.FromBase64String(bin))
            : null;
        return new WireFailure(CodeFromNumber(status), message, detail, httpStatus, rpc);
    }

    /// <summary>The failure a Connect stream's end-of-stream frame describes.</summary>
    public static WireFailure? StreamFailure(EndStreamResponse end, int httpStatus, string rpc)
    {
        if (end.Error is null)
        {
            return null;
        }
        return new WireFailure(CodeFromWire(end.Error.Code), end.Error.Message,
            ErrorInfoFrom(end.Error.Details), httpStatus, rpc);
    }

    /// <summary>
    /// Parses the end-of-stream frame, whose <c>{"error": {…}}</c> nests the
    /// envelope one level down from a unary error body.
    /// </summary>
    /// <remarks>
    /// An empty payload, or one with no <c>error</c> member, is a clean end: the
    /// protocol allows a server to close a stream with no error at all, and the
    /// corpus's recorded streams are bounded prefixes that a server closed that
    /// way. What it does not allow is an error the SDK cannot read, so a payload
    /// that names a code but whose details do not parse still yields a typed
    /// failure with that code.
    /// </remarks>
    public static EndStreamResponse ParseEndStreamFrame(byte[] payload)
    {
        if (payload.Length == 0)
        {
            return new EndStreamResponse(null);
        }
        try
        {
            using var document = System.Text.Json.JsonDocument.Parse(payload);
            var root = document.RootElement;
            if (root.ValueKind != System.Text.Json.JsonValueKind.Object ||
                !root.TryGetProperty("error", out var error) ||
                error.ValueKind != System.Text.Json.JsonValueKind.Object)
            {
                return new EndStreamResponse(null);
            }
            var code = error.TryGetProperty("code", out var c) ? c.GetString() ?? string.Empty : string.Empty;
            var message = error.TryGetProperty("message", out var m) && m.ValueKind == System.Text.Json.JsonValueKind.String
                ? m.GetString() ?? string.Empty
                : string.Empty;
            var details = new List<ConnectDetail>();
            if (error.TryGetProperty("details", out var list) &&
                list.ValueKind == System.Text.Json.JsonValueKind.Array)
            {
                foreach (var item in list.EnumerateArray())
                {
                    if (item.ValueKind != System.Text.Json.JsonValueKind.Object)
                    {
                        continue;
                    }
                    var type = item.TryGetProperty("type", out var t) ? t.GetString() ?? string.Empty : string.Empty;
                    var value = item.TryGetProperty("value", out var v) ? v.GetString() ?? string.Empty : string.Empty;
                    details.Add(new ConnectDetail(type, value));
                }
            }
            return new EndStreamResponse(new ConnectErrorBody(code, message, details));
        }
        catch (System.Text.Json.JsonException)
        {
            return new EndStreamResponse(null);
        }
    }

    /// <summary>The Connect code a wire name names, or <see cref="Code.Unknown"/>.</summary>
    public static Code CodeFromWire(string wire) => wire switch
    {
        "canceled" or "cancelled" => Code.Cancelled,
        "invalid_argument" => Code.InvalidArgument,
        "deadline_exceeded" => Code.DeadlineExceeded,
        "not_found" => Code.NotFound,
        "already_exists" => Code.AlreadyExists,
        "permission_denied" => Code.PermissionDenied,
        "resource_exhausted" => Code.ResourceExhausted,
        "failed_precondition" => Code.FailedPrecondition,
        "aborted" => Code.Aborted,
        "out_of_range" => Code.OutOfRange,
        "unimplemented" => Code.Unimplemented,
        "internal" => Code.Internal,
        "unavailable" => Code.Unavailable,
        "data_loss" => Code.DataLoss,
        "unauthenticated" => Code.Unauthenticated,
        _ => Code.Unknown,
    };

    /// <summary>The Connect code a gRPC status number names.</summary>
    public static Code CodeFromNumber(int status) => status switch
    {
        1 => Code.Cancelled,
        3 => Code.InvalidArgument,
        4 => Code.DeadlineExceeded,
        5 => Code.NotFound,
        6 => Code.AlreadyExists,
        7 => Code.PermissionDenied,
        8 => Code.ResourceExhausted,
        9 => Code.FailedPrecondition,
        10 => Code.Aborted,
        11 => Code.OutOfRange,
        12 => Code.Unimplemented,
        13 => Code.Internal,
        14 => Code.Unavailable,
        15 => Code.DataLoss,
        16 => Code.Unauthenticated,
        _ => Code.Unknown,
    };

    private static string Trim(string body)
    {
        var text = body.Trim();
        return text.Length <= 512 ? text : text[..512] + "…";
    }
}

/// <summary>
/// Serializes and deserializes messages in the codec a client chose.
/// </summary>
/// <remarks>
/// The JSON side is the one place the SDK does not use <c>Google.Protobuf</c>'s
/// own <c>JsonFormatter</c>, and it is deliberate. The formatter emits
/// <c>{ "field": value }</c> with spaces after the braces and colons, while the
/// corpus records the compact proto3 JSON mapping — <c>{"approvalId":"apr_…",
/// "revision":"1"}</c> — and <c>fixture-server.mjs</c> compares the request bytes
/// it receives against the recorded ones <b>byte for byte</b>. A space is a
/// different request, and a replay that sends one is answered 400. So the JSON
/// writer below emits the compact form, in field-number order, with default
/// values omitted — the same bytes every other Loams SDK sends, and the same ones
/// the recording was made with.
/// </remarks>
public static class MessageCodec
{
    /// <summary>Serializes a message in the given codec.</summary>
    public static byte[] Serialize(IMessage message, Codec codec)
    {
        ArgumentNullException.ThrowIfNull(message);
        return codec == Codec.Proto ? message.ToByteArray() : Encoding.UTF8.GetBytes(CompactJson.Format(message));
    }

    /// <summary>
    /// Deserializes into a fresh instance of the generated type.
    /// </summary>
    /// <remarks>
    /// Generic over the message rather than taking one to fill, because
    /// <c>Google.Protobuf</c> exposes no "parse into this instance" overload: a
    /// parse either constructs the message or throws. That is the right shape
    /// here anyway, since every caller has the generated type statically and the
    /// alternative — reflection over the descriptor — would buy nothing.
    /// </remarks>
    public static T Deserialize<T>(ReadOnlySpan<byte> body, Codec codec) where T : IMessage<T>, new()
    {
        if (codec == Codec.Proto)
        {
            return new MessageParser<T>(() => new T()).ParseFrom(body);
        }
        return JsonParser.Default.Parse<T>(Encoding.UTF8.GetString(body));
    }

    /// <summary>
    /// Deserializes into the type a message descriptor names, for the one place
    /// that has a descriptor rather than a type: the dynamic driver the
    /// conformance suite replays a corpus through.
    /// </summary>
    public static IMessage Deserialize(Google.Protobuf.Reflection.MessageDescriptor descriptor,
        ReadOnlySpan<byte> body, Codec codec)
    {
        ArgumentNullException.ThrowIfNull(descriptor);
        var parser = descriptor.Parser;
        return codec == Codec.Proto ? parser.ParseFrom(body.ToArray()) : parser.ParseJson(Encoding.UTF8.GetString(body));
    }
}