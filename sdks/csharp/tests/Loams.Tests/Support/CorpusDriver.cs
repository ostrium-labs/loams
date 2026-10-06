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
// The same rule governs every other decision in here. Which call a step is driven
// through, whether its idempotency key is minted, and where a named response field
// lives are all asked of the **schema** — the RPC path, the request and response
// descriptors — and never of a fixture's name. A rule written as a list of names
// stops tracking what the driver did the moment the corpus grows.
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
//     (`apiVersions`, `state`, `revision`, `cursor`, `snapshotReset`);
//   - `identicalToStep` means the **SDK's own** two answers are the same bytes. It
//     is not a comparison of the recording against itself, which would hold for a
//     driver that never looked at either answer.

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
        var steps = CorpusRecording.ReadSteps(Path.Combine(_fixturesDir, fixture.File));

        // The binding is resolved from the RPC path, not looked up in a table: a
        // fixture whose RPC the SDK does not bind is a fixture this suite cannot run,
        // and saying so by name is what makes the gate's gap legible.
        var rpc = steps[0].Path.TrimStart('/');
        if (Facade.BindingForRpc(rpc) is null)
        {
            return new FixtureOutcome(name, Ran: false,
                [$"{name}: no generated binding for {rpc}, so this suite cannot run it"]);
        }

        // The SDK's **own** answer per step, in step order, for `identicalToStep`.
        // The recording's bytes are deliberately not collected: comparing them to
        // each other is a check that holds for a driver that never looked at what
        // the SDK made of them.
        var answers = new List<byte[]?>();
        foreach (var step in steps)
        {
            cancellationToken.ThrowIfCancellationRequested();
            var outcome = await RunStepAsync(endpoint, name, step, cancellationToken).ConfigureAwait(false);
            failures.AddRange(outcome.Problems);
            answers.Add(outcome.Answer);

            if (step.Expect.ValueKind != JsonValueKind.Object ||
                !step.Expect.TryGetProperty("identicalToStep", out var identical))
            {
                continue;
            }
            var against = identical.GetInt32();
            if (against < 0 || against >= answers.Count - 1)
            {
                failures.Add(
                    $"{name} step {step.Step}: expect.identicalToStep names step {against}, and this is " +
                    $"step {answers.Count - 1}, so there is no earlier step for it to be identical to");
                continue;
            }
            var mine = outcome.Answer;
            var theirs = answers[against];
            if (theirs is null || mine is null || !mine.AsSpan().SequenceEqual(theirs))
            {
                failures.Add(
                    $"{name} step {step.Step}: expect.identicalToStep says its response bytes are " +
                    $"step {against}'s, and the SDK's two answers are not the same");
            }
        }

        if (failures.Count > 0)
        {
            return new FixtureOutcome(name, Ran: false, failures);
        }
        return new FixtureOutcome(name, Ran: true, []);
    }

    /// <summary>
    /// One step's problems, and the answer the SDK made of it.
    /// </summary>
    /// <param name="Problems">
    /// Every disagreement with the recording, so one run learns about all of them.
    /// </param>
    /// <param name="Answer">
    /// The SDK's answer, re-encoded in this step's codec — null when the call was
    /// refused, because a refusal has no response to compare.
    /// </param>
    private sealed record StepOutcome(IReadOnlyList<string> Problems, byte[]? Answer);

    private async Task<StepOutcome> RunStepAsync(
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
            return new StepOutcome([$"{fixtureName} step {step.Step}: no generated binding for {rpc}"], null);
        }

        var (protocol, codec) = TransportOf(step.ContentType);
        var client = ClientFor(endpoint, protocol, codec);

        var requestType = DescriptorFor(binding.RequestTypeName, fixtureName, step);
        var request = DecodeRequest(step, requestType, codec, fixtureName);
        if (request is null)
        {
            return new StepOutcome(problems, null);
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
            // R3 deliberately **not** applied to a recorded keyless mutation. See
            // `KeylessMutation` for why that is decided from the schema and not
            // from a list of fixture names.
            MintIdempotencyKey = !KeylessMutation(request),
        };

        if (binding.Streaming == Loams.Streaming.Server)
        {
            var streamed = await RunStreamStepAsync(client, binding, request, options, step, fixtureName,
                cancellationToken).ConfigureAwait(false);
            return new StepOutcome(streamed.Problems, streamed.Answer);
        }

        IMessage response;
        try
        {
            response = await client.Invoker.UnaryDynamicAsync(binding, request, options, cancellationToken)
                .ConfigureAwait(false);
        }
        catch (LoamsError error)
        {
            return new StepOutcome(
                Expectations.CheckError(fixtureName, step.Step, step.Expect, error).ToList(), null);
        }

        problems.AddRange(Expectations
            .CheckSuccess(fixtureName, step.Step, step.Status, step.Expect, response, expected: step.Status == 200)
            .ToList());
        return new StepOutcome(problems, AnswerBytes(response));
    }

    /// <summary>
    /// The SDK's own answer, re-encoded so two answers can be compared byte for
    /// byte.
    /// </summary>
    /// <remarks>
    /// Always in <see cref="Codec.Proto"/>, whatever the step's own encoding is, and
    /// for a reason: <c>CompactJson</c> deliberately refuses a message holding a
    /// well-known type — a <c>Timestamp</c>, a <c>Duration</c> — because no Loams
    /// facade call *sends* one and a body the server cannot parse is worse than a
    /// refusal at the boundary. <c>DecideApprovalResponse.approval.created_at</c> is
    /// exactly such a message, so asking the JSON writer to re-encode an answer
    /// throws, and the comparison would be unavailable on precisely the fixture that
    /// needs it. Binary protobuf has no such gap and is the stricter comparison
    /// anyway: it covers every field of both messages, including the ones proto3
    /// JSON omits at their defaults.
    ///
    /// The one caveat is a map field, whose iteration order protobuf does not
    /// specify, so two messages that differ only in map order would compare unequal.
    /// <c>DecideApprovalResponse</c> has none, and the alternative — comparing two
    /// JSON encodings this writer cannot produce — is not a comparison at all.
    /// </remarks>
    private static byte[] AnswerBytes(IMessage message) => MessageCodec.Serialize(message, Codec.Proto);

    /// <summary>
    /// Whether this step is one the SDK's own keyed path cannot reproduce.
    /// </summary>
    /// <remarks>
    /// **D610 gives every mutation an idempotency key**, and six app-mock
    /// mutations were recorded <i>without</i> one — the client that recorded them
    /// did not send a key, so there was none to record. Putting one on the wire
    /// changes the request, and `fixture-server.mjs` — correctly — refuses a
    /// request that is not the recorded one, answering 400 with its own JSON error
    /// body. That answer is not a Connect error envelope, so the SDK reported
    /// `code=unknown` with no reason against a server that had said exactly which
    /// reason it meant: 18 approvals and 10 devices calls, every one of them a
    /// harness disagreement dressed up as an error-mapping bug.
    ///
    /// Decided from the **request schema** and the decoded message — the same two
    /// questions <see cref="Idempotency.Apply"/> asks — so a corpus that grows a
    /// keyed mutation is handled by the rule rather than by somebody remembering to
    /// move a name. A request whose schema declares no key field is never keyless
    /// here, because there was nothing to mint; and a request that already carries
    /// one keeps it, so `mock_state_idempotent_decide` and the two
    /// `mock_state_stream_resume*` mutations are still driven through R3's keyed
    /// path, which is the only place that clause is exercised end to end.
    /// </remarks>
    private static bool KeylessMutation(IMessage request)
    {
        var field = request.Descriptor.FindFieldByName("idempotency_key");
        if (field is null || field.FieldType != FieldType.String)
        {
            return false;
        }
        return (field.Accessor.GetValue(request) as string ?? string.Empty).Length == 0;
    }

    private async Task<StepOutcome> RunStreamStepAsync(
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
        var answer = new MemoryStream();

        try
        {
            await foreach (var message in handle.Messages.WithCancellation(cancellationToken).ConfigureAwait(false))
            {
                messages.Add(message);
                // The answer is the SDK's own re-encoding of the messages it
                // yielded, which is what `identicalToStep` compares. A stream
                // recording never uses `identicalToStep`, so this costs one
                // re-encode per message and keeps the one comparison honest.
                answer.Write(AnswerBytes(message));
            }
        }
        catch (LoamsError error)
        {
            return new StepOutcome(Expectations.CheckError(fixtureName, step.Step, step.Expect, error).ToList(), null);
        }

        if (step.Expect.ValueKind != JsonValueKind.Object)
        {
            return new StepOutcome(problems, answer.ToArray());
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
            problems.AddRange(Expectations.CheckMessage(fixtureName, step.Step, step.Expect, message).ToList());
        }
        if (messages.Count == 0 && step.Expect.TryGetProperty("frameKinds", out _))
        {
            problems.Add($"{fixtureName} step {step.Step}: expect.frameKinds is set and no message arrived");
        }
        return new StepOutcome(problems, answer.ToArray());
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
}