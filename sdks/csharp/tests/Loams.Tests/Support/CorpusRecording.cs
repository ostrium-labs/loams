// Reading a recorded fixture off disk — the one place the corpus file format is
// interpreted.
//
// # Why both roles share it
//
// Two things read a recording: the **fixture server**, which replays the bytes it
// finds, and the **corpus driver**, which replays a call and compares the answer
// to what the recording says. They agreed before, by having two copies of the same
// reader, and they stopped agreeing the moment the corpus's `body` stopped being
// always a string: the server read only the string form and silently served an
// **empty** body for every recorded Connect refusal, so the driver compared a
// `failed_precondition` the server never sent. A disagreement between what a suite
// thinks the corpus says and what it was served is the most expensive kind of bug
// in a conformance harness, so there is one reader.
//
// # The corpus's `body` is a string *or* an object
//
// A recorded body is the bytes the server actually sent, and the recorder wrote it
// as parsed JSON rather than as a string when it was JSON. Both forms are in
// `sdks/fixtures`: `mock_error_approval_expired` records `request.body` as the
// string `{"approvalId":"…"}` and `response.body` as an object, while
// `live_watch` records both as `bodyBase64`. An object body is therefore
// re-serialized to recover its bytes, and a body that is neither is empty rather
// than an error — a recording with no body is a recording of an empty body.

using System.Text;
using System.Text.Json;

namespace Loams.Tests.Support;

/// <summary>One recorded body: <c>bodyBase64</c> decoded, or <c>body</c> as written.</summary>
public static class CorpusRecording
{
    /// <summary>
    /// A recorded body's bytes.
    /// </summary>
    /// <remarks>
    /// A JSON <c>body</c> may be a **string** or an **object** (or an array), and all
    /// three forms appear in the corpus. Reading only the string form silently turns
    /// every recorded Connect refusal into an empty body, which is how a recorded
    /// <c>failed_precondition</c> becomes an <c>unknown</c> with no reason and no
    /// message — a harness disagreement wearing the costume of an SDK bug.
    ///
    /// An object body is re-serialized with <c>GetRawText</c>, which is the exact
    /// text the recorder wrote rather than a round trip through a parser: the bytes
    /// a fixture server compares against are the ones in the file, and a
    /// re-serialization that reordered or re-escaped a character would make every
    /// recorded refusal a mismatch.
    /// </remarks>
    public static byte[] BodyOf(JsonElement holder)
    {
        if (holder.TryGetProperty("bodyBase64", out var base64))
        {
            return Convert.FromBase64String(base64.GetString() ?? string.Empty);
        }
        if (!holder.TryGetProperty("body", out var body))
        {
            return [];
        }
        return body.ValueKind switch
        {
            JsonValueKind.String => Encoding.UTF8.GetBytes(body.GetString() ?? string.Empty),
            JsonValueKind.Object or JsonValueKind.Array => Encoding.UTF8.GetBytes(body.GetRawText()),
            _ => [],
        };
    }

    /// <summary>
    /// One recorded header, by name, case-insensitively, or null.
    /// </summary>
    /// <remarks>
    /// The corpus spells headers lower case, and HTTP header names are
    /// case-insensitive, so the lookup cannot be.
    /// </remarks>
    public static string? HeaderOf(JsonElement holder, string name)
    {
        if (holder.ValueKind != JsonValueKind.Object ||
            !holder.TryGetProperty("headers", out var headers) ||
            headers.ValueKind != JsonValueKind.Object)
        {
            return null;
        }
        foreach (var header in headers.EnumerateObject())
        {
            if (string.Equals(header.Name, name, StringComparison.OrdinalIgnoreCase))
            {
                return header.Value.GetString();
            }
        }
        return null;
    }

    /// <summary>
    /// The frames a recorded response carries separately from its body, or null.
    /// </summary>
    /// <remarks>
    /// Only the gRPC and Connect stream recordings carry a <c>frames</c> array; a
    /// unary one files its single frame under <c>bodyBase64</c>, and null is the
    /// answer for both because the callers treat "no frames array" as "not a frame
    /// recording" rather than as "a stream with no frames".
    /// </remarks>
    public static IReadOnlyList<RecordedFrame>? FramesOf(JsonElement response)
    {
        if (!response.TryGetProperty("frames", out var frames) || frames.ValueKind != JsonValueKind.Array)
        {
            return null;
        }
        var parsed = new List<RecordedFrame>();
        foreach (var frame in frames.EnumerateArray())
        {
            var flags = frame.TryGetProperty("flags", out var f) ? f.GetInt32() : 0;
            parsed.Add(new RecordedFrame((Loams.EnvelopeFlags)flags,
                Convert.FromBase64String(frame.GetProperty("payload").GetString()!)));
        }
        return parsed;
    }

    /// <summary>
    /// Every step of one recording, in file order.
    /// </summary>
    /// <remarks>
    /// A fixture is either a single step at the top level or a <c>steps</c> array;
    /// both are recorded and the corpus uses both. The step number is the index in
    /// that array, because the fixture server's <c>loams-fixture-step</c> header
    /// names it and two scenarios send byte-identical requests twice.
    /// </remarks>
    public static IReadOnlyList<RecordedStep> ReadSteps(string file)
    {
        using var document = JsonDocument.Parse(File.ReadAllText(file));
        var root = document.RootElement;
        var name = root.GetProperty("name").GetString()!;
        var recorded = root.TryGetProperty("steps", out var list)
            ? list.EnumerateArray().ToArray()
            : [root];

        var steps = new List<RecordedStep>(recorded.Length);
        for (var index = 0; index < recorded.Length; index++)
        {
            var step = recorded[index];
            steps.Add(new RecordedStep(
                name,
                index,
                step.GetProperty("request").GetProperty("method").GetString()!,
                step.GetProperty("request").GetProperty("path").GetString()!,
                HeaderOf(step.GetProperty("request"), "content-type"),
                BodyOf(step.GetProperty("request")),
                step.GetProperty("response").GetProperty("status").GetInt32(),
                HeaderOf(step.GetProperty("response"), "content-type"),
                BodyOf(step.GetProperty("response")),
                FramesOf(step.GetProperty("response")),
                step.TryGetProperty("expect", out var expect) ? expect.Clone() : default));
        }
        return steps;
    }
}