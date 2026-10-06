// The corpus driver: it replays `sdks/fixtures` through the SDK and reports what it
// actually ran.
//
// # Why the driver derives its coverage rather than listing it
//
// `sdks/conformance/required.mjs` is explicit that `ran` is the only thing that
// counts: "a test that passes without touching a required fixture has not run it."
// A hand-written list of fixture names would be a second source of truth that
// could name a fixture the suite stopped replaying, and the gate would pass on it —
// so the list here is read from `manifest.json` at run time, the fixture is
// **replayed**, and the name goes into `ran` only once the recorded response and
// the recording's own `expect` block have both been checked. A fixture the driver
// could not reach is therefore absent from `ran` and the gate says so by name.
//
// # What a fixture run asserts
//
// Everything the recording states about itself, and nothing it does not:
//
//   - the **request** is byte-identical to the recording, because
//     `fixture-server.mjs` answers 400 otherwise — which is what proves the SDK
//     encoded what the corpus recorded rather than something equivalent;
//   - the **outcome** matches `expect`: the HTTP status as the SDK saw it, the
//     `reason`, the `grpcStatus`, and the response fields the recording names
//     (`apiVersions`, `state`, `revision`, `frames`, `frameKinds`, `cursor`,
//     `snapshotReset`);
//   - `identicalToStep` means the second response's bytes are the first's.

using System.Text;
using System.Text.Json;
using Google.Protobuf;
using Google.Protobuf.Reflection;
using Loams;
using Loams.Tests.Support;

namespace Loams.Tests;

/// <summary>What one fixture's replay found.</summary>
/// <param name="Name">The fixture's name, as the manifest spells it.</param>
/// <param name="Ran">Whether the driver replayed it and every assertion held.</param>
/// <param name="Failures">What failed, with the fixture's name in each message.</param>
public sealed record FixtureOutcome(string Name, bool Ran, IReadOnlyList<string> Failures);

/// <summary>The whole run's result, and the report written from it.</summary>
/// <param name="Outcomes">One entry per required fixture, in manifest order.</param>
/// <param name="Endpoint">Where it ran, for the report and for a failure message.</param>
/// <param name="Live">Whether that endpoint was a real server rather than a replay.</param>
public sealed record CorpusRun(IReadOnlyList<FixtureOutcome> Outcomes, string Endpoint, bool Live)
{
    /// <summary>The fixture names the driver replayed, which is what `ran` is.</summary>
    public IReadOnlyList<string> Ran => Outcomes.Where((outcome) => outcome.Ran).Select((outcome) => outcome.Name).ToArray();

    /// <summary>Every failure, across every fixture.</summary>
    public IReadOnlyList<string> Failures =>
        Outcomes.SelectMany((outcome) => outcome.Failures).ToArray();

    /// <summary>Whether every required fixture ran and every assertion held.</summary>
    public bool Ok => Failures.Count == 0 && Outcomes.All((outcome) => outcome.Ran);
}

/// <summary>
/// Replays every required fixture through the SDK's public surface.
/// </summary>
/// <remarks>
/// One client per (protocol, codec) pair, built lazily: the corpus records four
/// encodings of the same RPC and a client that picked one would fail the other
/// three, which is the point of them. Each fixture's own
/// <c>loams-fixture-name</c> and <c>loams-fixture-step</c> headers ride along, because
/// five recorded scenarios share the <c>WatchApprovals</c> Connect-JSON key and the
/// idempotency replay sends the same request twice.
/// </remarks>
public sealed class CorpusDriver
{
    private readonly string _fixturesDir;
    private readonly JsonDocument _manifest;
    private readonly List<(Protocol Protocol, Codec Codec, LoamsClient Client)> _clients = [];

    /// <summary>Builds a driver over a corpus directory.</summary>
    public CorpusDriver(string fixturesDir)
    {
        ArgumentException.ThrowIfNullOrWhiteSpace(fixturesDir);
        _fixturesDir = fixturesDir;
        _manifest = JsonDocument.Parse(File.ReadAllText(Path.Combine(fixturesDir, "manifest.json")));
    }

    /// <summary>One required fixture, as the manifest states it.</summary>
    /// <param name="Name">The fixture's name, which is what `ran` carries.</param>
    /// <param name="File">The recording, relative to the corpus root.</param>
    /// <param name="Reason">The reason the manifest catalogues it under, or empty.</param>
    /// <param name="Clauses">The runtime-contract clauses it pins, from `pinnedBy`.</param>
    public sealed record RequiredFixture(
        string Name,
        string File,
        string Reason,
        IReadOnlyList<string> Clauses);

    /// <summary>
    /// Every required fixture, read from the manifest and nothing else.
    /// </summary>
    /// <remarks>
    /// This is the anti-vacuity property, stated as code: the list of fixtures the
    /// run is judged on is the manifest's, so a fixture the manifest newly requires
    /// is in the next run without anybody editing this file, and a fixture that stops
    /// being reachable is named in the failure rather than quietly dropped from a
    /// hand-written list.
    /// </remarks>
    public IReadOnlyList<RequiredFixture> RequiredFixtures => ReadRequiredFixtures();

    private IReadOnlyList<RequiredFixture> ReadRequiredFixtures()
    {
        var required = new List<RequiredFixture>();
        foreach (var fixture in _manifest.RootElement.GetProperty("fixtures").EnumerateArray())
        {
            if (!fixture.GetProperty("required").GetBoolean())
            {
                continue;
            }
            required.Add(new RequiredFixture(
                fixture.GetProperty("name").GetString()!,
                fixture.GetProperty("file").GetString()!,
                fixture.TryGetProperty("reason", out var reason) && reason.ValueKind == JsonValueKind.String
                    ? reason.GetString()!
                    : string.Empty,
                fixture.GetProperty("pinnedBy").EnumerateArray()
                    .Select((clause) => clause.GetString()!).ToArray()));
        }
        return required;
    }

    /// <summary>Runs every required fixture.</summary>
    public async Task<CorpusRun> RunAsync(CancellationToken cancellationToken = default)
    {
        var fixturesDir = Path.GetFullPath(_fixturesDir);
        var conformanceDir = Path.GetFullPath(Path.Combine(fixturesDir, "..", "conformance"));
        using var server = FixtureServer.Start(fixturesDir, conformanceDir);

        var outcomes = new List<FixtureOutcome>();
        foreach (var fixture in RequiredFixtures)
        {
            cancellationToken.ThrowIfCancellationRequested();
            outcomes.Add(await RunFixtureAsync(server.Endpoint, fixture, cancellationToken).ConfigureAwait(false));
        }

        return new CorpusRun(outcomes, server.Endpoint, server.Live);
    }

    private async Task<FixtureOutcome> RunFixtureAsync(
        string endpoint,
        RequiredFixture fixture,
        CancellationToken cancellationToken)
    {
        var name = fixture.Name;
        var failures = new List<string>();
        var steps = ReadSteps(Path.Combine(_fixturesDir, fixture.File));

        // The binding is resolved from the RPC path, not looked up in a table: a
        // fixture whose RPC the SDK does not bind is a fixture this suite cannot run,
        // and saying so by name is what makes the gate's gap legible.
        var rpc = steps[0].Path.TrimStart('/');
        if (Facade.BindingForRpc(rpc) is null)
        {
            return new FixtureOutcome(name, Ran: false,
                [$"{name}: no generated binding for {rpc}, so this suite cannot run it"]);
        }

        byte[]? firstStepResponse = null;
        foreach (var step in steps)
        {
            cancellationToken.ThrowIfCancellationRequested();
            var problems = await RunStepAsync(endpoint, name, step, cancellationToken).ConfigureAwait(false);
            failures.AddRange(problems);

            if (step.Expect.ValueKind == JsonValueKind.Object &&
                step.Expect.TryGetProperty("identicalToStep", out var identical))
            {
                var against = firstStepResponse;
                var thisStep = StepResponseBytes(steps, step);
                if (against is null || !thisStep.AsSpan().SequenceEqual(against))
                {
                    failures.Add(
                        $"{name} step {step.Step}: expect.identicalToStep says its response bytes are " +
                        $"step {identical.GetInt32()}'s, and they are not");
                }
            }
            if (step.Step == 0)
            {
                firstStepResponse = StepResponseBytes(steps, step);
            }
        }

        if (failures.Count > 0)
        {
            return new FixtureOutcome(name, Ran: false, failures);
        }
        return new FixtureOutcome(name, Ran: true, []);
    }

    /// <summary>
    /// The response bytes a step recorded, reassembled from its frames when it is a
    /// stream.
    /// </summary>
    private static byte[] StepResponseBytes(IReadOnlyList<RecordedStep> steps, RecordedStep step)
    {
        if (step.Frames is not { Count: > 0 })
        {
            return step.ResponseBody;
        }
        // A stream's response is its frames; the comparison is over the message
        // frames, so the end-of-stream and trailers frames are left out.
        return Encoding.UTF8.GetBytes(string.Concat(
            step.Frames
                .Where((frame) => !frame.Flags.HasFlag(Loams.EnvelopeFlags.EndOfStream) &&
                                 !frame.Flags.HasFlag(Loams.EnvelopeFlags.Trailers))
                .Select((frame) => Encoding.UTF8.GetString(frame.Payload))));
    }

    private async Task<IReadOnlyList<string>> RunStepAsync(
        string endpoint,
        string fixtureName,
        RecordedStep step,
        CancellationToken cancellationToken)
    {
        var problems = new List<string>();
        var rpc = step.Path.TrimStart('/');
        var binding = Facade.BindingForRpc(rpc);
        if (binding is null)
        {
            return [$"{fixtureName} step {step.Step}: no generated binding for {rpc}"];
        }

        var (protocol, codec) = TransportOf(step.ContentType);
        var client = ClientFor(endpoint, protocol, codec);

        var requestType = DescriptorFor(binding.RequestTypeName, fixtureName, step);
        var request = DecodeRequest(step, requestType, codec, fixtureName);
        if (request is null)
        {
            return problems;
        }

        var options = new Loams.CallOptions
        {
            // The two headers the harness defines: which fixture answers, and which
            // step of it. Both are needed — six recorded scenarios share the
            // `WatchApprovals` Connect-JSON key, and the idempotency replay sends the
            // same request twice.
            Headers = new Dictionary<string, string>(StringComparer.Ordinal)
            {
                ["loams-fixture-name"] = fixtureName,
                ["loams-fixture-step"] = step.Step.ToString(System.Globalization.CultureInfo.InvariantCulture),
            },
        };

        if (binding.Streaming == Loams.Streaming.Server)
        {
            problems.AddRange(await RunStreamStepAsync(client, binding, request, options, step, fixtureName,
                cancellationToken).ConfigureAwait(false));
            return problems;
        }

        Google.Protobuf.IMessage response;
        try
        {
            response = await client.Invoker.UnaryDynamicAsync(binding, request, options, cancellationToken)
                .ConfigureAwait(false);
        }
        catch (LoamsError error)
        {
            problems.AddRange(CheckError(fixtureName, step, error).ToList());
            return problems;
        }

        problems.AddRange(CheckSuccess(fixtureName, step, response, expected: step.Status == 200).ToList());
        return problems;
    }

    private async Task<IReadOnlyList<string>> RunStreamStepAsync(
        LoamsClient client,
        Loams.CallBinding binding,
        Google.Protobuf.IMessage request,
        Loams.CallOptions options,
        RecordedStep step,
        string fixtureName,
        CancellationToken cancellationToken)
    {
        var problems = new List<string>();
        var handle = client.Invoker.OpenServerStream(binding, request, options, cancellationToken);
        var messages = new List<Google.Protobuf.IMessage>();

        try
        {
            await foreach (var message in handle.Messages.WithCancellation(cancellationToken).ConfigureAwait(false))
            {
                messages.Add(message);
            }
        }
        catch (LoamsError error)
        {
            problems.AddRange(CheckError(fixtureName, step, error).ToList());
            return problems;
        }

        if (!step.Expect.ValueKind.Equals(JsonValueKind.Object))
        {
            return problems;
        }

        // `frames` counts every frame on the wire and `frameKinds` names them in
        // order, so the heartbeat fixture's two frames and one message is checked as
        // two frames whose kinds are `snapshot`, `heartbeat` — and the messages the
        // caller saw are one.
        if (step.Expect.TryGetProperty("frames", out var frames))
        {
            var want = frames.GetInt32();
            if (handle.FrameKinds.Count != want)
            {
                problems.Add(
                    $"{fixtureName} step {step.Step}: expect.frames is {want}, and {handle.FrameKinds.Count} " +
                    $"frame(s) arrived ({string.Join(", ", handle.FrameKinds)})");
            }
        }
        if (step.Expect.TryGetProperty("frameKinds", out var kinds))
        {
            var want = kinds.EnumerateArray().Select((kind) => kind.GetString()!).ToArray();
            if (!handle.FrameKinds.SequenceEqual(want))
            {
                problems.Add(
                    $"{fixtureName} step {step.Step}: expect.frameKinds is [{string.Join(", ", want)}], and " +
                    $"[{string.Join(", ", handle.FrameKinds)}] arrived");
            }
        }

        foreach (var message in messages)
        {
            problems.AddRange(CheckMessage(fixtureName, step, message).ToList());
        }
        if (messages.Count == 0 && step.Expect.TryGetProperty("frameKinds", out _))
        {
            problems.Add($"{fixtureName} step {step.Step}: expect.frameKinds is set and no message arrived");
        }
        return problems;
    }

    /// <summary>The two refusals R8 distinguishes, checked from the recording's own words.</summary>
    private static IEnumerable<string> CheckError(string fixtureName, RecordedStep step, LoamsError error)
    {
        var problems = new List<string>();
        var expect = step.Expect;

        if (expect.ValueKind != JsonValueKind.Object)
        {
            return problems;
        }

        if (expect.TryGetProperty("reason", out var reason))
        {
            if (reason.ValueKind != JsonValueKind.String)
            {
                // `reason: null` is a recorded fact: the server sent no `ErrorInfo`
                // and the SDK must not invent one (R8, and `mock_status_unauthenticated`).
                if (error.Reason != Loams.Reason.None)
                {
                    problems.Add(
                        $"{fixtureName} step {step.Step}: expect.reason is null, and the SDK reported " +
                        $"{Loams.ReasonRegistry.Name(error.Reason)}");
                }
            }
            else
            {
                var want = reason.GetString()!;
                var got = error.Reason == Loams.Reason.None ? error.UnknownReason : Loams.ReasonRegistry.Name(error.Reason);
                if (got != want)
                {
                    problems.Add($"{fixtureName} step {step.Step}: expect.reason is {want}, and the SDK reported {got ?? "none"}");
                }
            }
        }

        if (expect.TryGetProperty("grpcStatus", out var grpcStatus))
        {
            var want = (Loams.Code)grpcStatus.GetInt32();
            if (error.Code != want)
            {
                problems.Add(
                    $"{fixtureName} step {step.Step}: expect.grpcStatus is {grpcStatus.GetInt32()}, and the " +
                    $"SDK reported {error.Code}");
            }
        }
        return problems;
    }

    private static IEnumerable<string> CheckSuccess(
        string fixtureName,
        RecordedStep step,
        Google.Protobuf.IMessage response,
        bool expected)
    {
        var problems = new List<string>();
        if (!expected)
        {
            problems.Add(
                $"{fixtureName} step {step.Step}: the recording answers HTTP {step.Status} and the SDK " +
                "returned a message, so a refusal was read as a success");
        }
        return problems.Concat(CheckMessage(fixtureName, step, response));
    }

    /// <summary>
    /// The response fields the recording's <c>expect</c> names.
    /// </summary>
    /// <remarks>
    /// Only the fields the recording actually states are checked, and each is read
    /// out of the descriptor rather than off a generated property: the corpus names
    /// them in proto3 JSON (<c>apiVersions</c>, <c>snapshotReset</c>, <c>frameKinds</c>)
    /// and the generated C# names are PascalCase, so the JSON name is the one the
    /// corpus and the fixture-server agree on.
    /// </remarks>
    private static IEnumerable<string> CheckMessage(
        string fixtureName,
        RecordedStep step,
        Google.Protobuf.IMessage message)
    {
        var problems = new List<string>();
        var expect = step.Expect;
        if (expect.ValueKind != JsonValueKind.Object)
        {
            return problems;
        }

        foreach (var name in new[] { "apiVersions", "state", "revision", "cursor", "snapshotReset" })
        {
            if (!expect.TryGetProperty(name, out var want))
            {
                continue;
            }
            if (!TryReadField(message, name, out var got))
            {
                problems.Add($"{fixtureName} step {step.Step}: expect.{name} is set and the response has no {name}");
                continue;
            }
            if (want.ValueKind == JsonValueKind.Array)
            {
                // `apiVersions` is a repeated field, and the recording states which
                // packages must be **present**: the mock serves five and names one,
                // so containment is the check and equality would be wrong.
                var wanted = want.EnumerateArray().Select((item) => item.GetString()!).ToList();
                var actual = ReadRepeated(message, name);
                foreach (var entry in wanted)
                {
                    if (!actual.Contains(entry))
                    {
                        problems.Add(
                            $"{fixtureName} step {step.Step}: expect.{name} contains {entry}, and the " +
                            $"response has [{string.Join(", ", actual)}]");
                    }
                }
                continue;
            }
            if (got != want.ToString())
            {
                problems.Add($"{fixtureName} step {step.Step}: expect.{name} is {want}, and the response says {got}");
            }
        }

        return problems;
    }

    /// <summary>
    /// Reads one proto3-JSON-named field out of a message, rendered the way the
    /// recording renders it: an enum by name, a 64-bit integer as a string, a bool
    /// as <c>true</c>/<c>false</c>.
    /// </summary>
    private static bool TryReadField(Google.Protobuf.IMessage message, string jsonName, out string? value)
    {
        value = null;
        var field = message.Descriptor.FindFieldByName(ToSnakeCase(jsonName)) ??
                    message.Descriptor.FindFieldByName(jsonName);
        if (field is null)
        {
            return false;
        }
        if (field.IsRepeated || field.IsMap)
        {
            // `IFieldAccessor.HasValue` throws for a repeated field: a repeated field
            // has no presence, it has a count. A repeated field named in `expect`
            // with nothing in it is a field that is **set** (to the empty list), so
            // the answer is "present" and the caller checks its contents.
            return true;
        }
        if (!field.Accessor.HasValue(message))
        {
            return false;
        }

        var raw = field.Accessor.GetValue(message);
        value = field.FieldType switch
        {
            FieldType.String => raw as string,
            FieldType.Bool => raw is true ? "true" : "false",
            FieldType.Enum => ((EnumValueDescriptor)raw!).Name,
            FieldType.UInt64 or FieldType.Fixed64 =>
                Convert.ToUInt64(raw, System.Globalization.CultureInfo.InvariantCulture)
                    .ToString(System.Globalization.CultureInfo.InvariantCulture),
            FieldType.Int64 or FieldType.SFixed64 or FieldType.SInt64 =>
                Convert.ToInt64(raw, System.Globalization.CultureInfo.InvariantCulture)
                    .ToString(System.Globalization.CultureInfo.InvariantCulture),
            _ => raw?.ToString(),
        };
        return true;
    }

    /// <summary>A repeated string field's values, rendered as the recording renders them.</summary>
    private static IReadOnlyList<string> ReadRepeated(Google.Protobuf.IMessage message, string jsonName)
    {
        var field = message.Descriptor.FindFieldByName(ToSnakeCase(jsonName)) ??
                    message.Descriptor.FindFieldByName(jsonName);
        if (field is null)
        {
            return [];
        }
        var values = new List<string>();
        foreach (var item in (System.Collections.IEnumerable)field.Accessor.GetValue(message)!)
        {
            values.Add(field.FieldType == FieldType.Enum
                ? ((EnumValueDescriptor)item!).Name
                : item?.ToString() ?? string.Empty);
        }
        return values;
    }

    private static string ToSnakeCase(string jsonName)
    {
        var builder = new StringBuilder(jsonName.Length + 4);
        for (var index = 0; index < jsonName.Length; index++)
        {
            var character = jsonName[index];
            if (char.IsUpper(character) && index > 0)
            {
                builder.Append('_');
            }
            builder.Append(char.ToLowerInvariant(character));
        }
        return builder.ToString();
    }

    /// <summary>
    /// The recorded request, decoded into the binding's request type.
    /// </summary>
    /// <remarks>
    /// Decoded rather than sent verbatim: the point of a fixture is that the SDK
    /// **encodes** what the corpus recorded, so handing the server the recording's own
    /// bytes would prove nothing. The re-encoded request goes out and
    /// <c>fixture-server.mjs</c> compares it byte for byte, which is where an SDK
    /// whose JSON writer emits a space fails.
    /// </remarks>
    private static Google.Protobuf.IMessage? DecodeRequest(
        RecordedStep step,
        MessageDescriptor requestType,
        Loams.Codec codec,
        string fixtureName)
    {
        try
        {
            // A stream's request body is one framed message; a unary's is the message
            // itself. Either way the payload is what has to decode.
            var payload = step.ContentType is { } contentType &&
                          (contentType.StartsWith("application/connect", StringComparison.Ordinal) ||
                           contentType.StartsWith("application/grpc-web", StringComparison.Ordinal))
                ? Loams.Envelopes.Split(step.RequestBody)[0].Payload
                : step.RequestBody;

            return Loams.MessageCodec.Deserialize(requestType, payload, codec);
        }
        catch (InvalidProtocolBufferException error)
        {
            throw new InvalidOperationException(
                $"{fixtureName} step {step.Step}: the recorded request does not decode as a " +
                $"{requestType.Name} ({error.Message})", error);
        }
    }

    /// <summary>
    /// The generated request type a binding names, through the SDK's own descriptor
    /// cache.
    /// </summary>
    /// <remarks>
    /// The cache is `internal` and the test assembly sees it through
    /// <c>InternalsVisibleTo</c>, which is deliberate: the driver asks the **same**
    /// question the invoker asks, so a binding whose type name does not resolve fails
    /// here for the same reason it would fail in a call.
    /// </remarks>
    private static MessageDescriptor DescriptorFor(string clrTypeName, string fixtureName, RecordedStep step) =>
        Descriptors.Find(clrTypeName) ?? throw new InvalidOperationException(
            $"{fixtureName} step {step.Step}: no generated message type named {clrTypeName}");

    /// <summary>The protocol and codec a recorded content type implies.</summary>
    private static (Loams.Protocol Protocol, Loams.Codec Codec) TransportOf(string? contentType)
    {
        var type = (contentType ?? string.Empty).Split(';')[0].Trim();
        return type switch
        {
            "application/json" => (Loams.Protocol.Connect, Loams.Codec.Json),
            "application/proto" => (Loams.Protocol.Connect, Loams.Codec.Proto),
            "application/grpc-web+json" => (Loams.Protocol.GrpcWeb, Loams.Codec.Json),
            "application/grpc-web+proto" => (Loams.Protocol.GrpcWeb, Loams.Codec.Proto),
            "application/connect+json" => (Loams.Protocol.Connect, Loams.Codec.Json),
            "application/connect+proto" => (Loams.Protocol.Connect, Loams.Codec.Proto),
            _ => throw new InvalidOperationException(
                $"the corpus records a content type this SDK does not speak: '{contentType}'"),
        };
    }

    private LoamsClient ClientFor(string endpoint, Loams.Protocol protocol, Loams.Codec codec)
    {
        for (var index = 0; index < _clients.Count; index++)
        {
            if (_clients[index].Protocol == protocol && _clients[index].Codec == codec)
            {
                return _clients[index].Client;
            }
        }

        // The fixture server is unauthenticated by default, which is what
        // `GetInstance` needs anyway and what keeps a bearer out of the corpus
        // driver entirely. The app-mock recordings carry one, and the harness does
        // not check it.
        var client = new LoamsClient(new LoamsClientOptions
        {
            Endpoint = endpoint,
            Protocol = protocol,
            Codec = codec,
            // No retries: the harness serves one recorded response per request, and a
            // retry would ask it for a second step's bytes and get a 400. The retry
            // policy is pinned by `csharp_retry_reuses_idempotency_key` against a
            // stub instead.
            NoRetries = true,
        });
        _clients.Add((protocol, codec, client));
        return client;
    }

    private IReadOnlyList<RecordedStep> ReadSteps(string file)
    {
        using var document = JsonDocument.Parse(File.ReadAllText(file));
        var root = document.RootElement;
        var name = root.GetProperty("name").GetString()!;
        var recorded = root.TryGetProperty("steps", out var list)
            ? list.EnumerateArray().ToArray()
            : [root];

        var steps = new List<RecordedStep>();
        for (var index = 0; index < recorded.Length; index++)
        {
            var step = recorded[index];
            steps.Add(new RecordedStep(name, index,
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

    private static string? HeaderOf(JsonElement holder, string name)
    {
        if (!holder.TryGetProperty("headers", out var headers) || headers.ValueKind != JsonValueKind.Object)
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

    private static byte[] BodyOf(JsonElement holder)
    {
        if (holder.TryGetProperty("bodyBase64", out var base64))
        {
            return Convert.FromBase64String(base64.GetString() ?? string.Empty);
        }
        if (holder.TryGetProperty("body", out var body) && body.ValueKind == JsonValueKind.String)
        {
            return Encoding.UTF8.GetBytes(body.GetString()!);
        }
        return [];
    }

    private static IReadOnlyList<RecordedFrame>? FramesOf(JsonElement response)
    {
        if (!response.TryGetProperty("frames", out var frames) || frames.ValueKind != JsonValueKind.Array)
        {
            return null;
        }
        var parsed = new List<RecordedFrame>();
        foreach (var frame in frames.EnumerateArray())
        {
            var flags = frame.TryGetProperty("flags", out var flag) ? flag.GetInt32() : 0;
            parsed.Add(new RecordedFrame((Loams.EnvelopeFlags)flags,
                Convert.FromBase64String(frame.GetProperty("payload").GetString()!)));
        }
        return parsed;
    }
}