// The conformance fixture server (design §44 §10.4, SDK1 Task 4).
//
// The corpus in `sdks/fixtures` is recorded from a real `loams dev` and
// `loams-apps-mock`, and every SDK's suite replays it so thirteen clients can be
// compared against the same bytes. This file is how a C# test gets an endpoint.
//
// Three sources, in the order they are tried:
//
//  1. `LOAMS_TEST_ENDPOINT` — a live server, or the shared fixture server
//     `sdks/conformance/run.sh` and `run-all.sh` boot. This short-circuits
//     everything else, which is how CI runs the same suite against a recording and a
//     developer runs it against a real `loams dev`.
//  2. `node sdks/conformance/fixture-server.mjs` — the shared server, spawned.
//     Preferred when Node is on PATH, because then this suite really does run
//     against the same server the other twelve do.
//  3. An in-process replay of `sdks/fixtures/recorded`, with the same matching rules.
//     A `dotnet test` should not need a JavaScript runtime to run, and the corpus —
//     the recorded bytes — is the part that matters.
//
// All three agree on the corpus, and `csharp_conformance_all_required_fixtures`
// reads `sdks/fixtures/manifest.json` and fails if a required fixture is not run,
// so a suite cannot quietly stop covering something.

using System.Diagnostics;
using System.Net;
using System.Text;
using System.Text.Json;

namespace Loams.Tests.Support;

/// <summary>One recorded step of one fixture.</summary>
/// <param name="Name">The fixture's name.</param>
/// <param name="Step">The zero-based step within it.</param>
/// <param name="Method">The request method.</param>
/// <param name="Path">The request path.</param>
/// <param name="ContentType">The content type, which is the family the corpus keys on.</param>
/// <param name="RequestBody">The request body, undecoded.</param>
/// <param name="Status">The recorded HTTP status.</param>
/// <param name="ResponseContentType">The recorded response content type.</param>
/// <param name="ResponseBody">The recorded response body, undecoded.</param>
/// <param name="Frames">The recorded frames, when the recording carries them separately from the body.</param>
/// <param name="Expect">The recording's own expectation, checked below.</param>
public sealed record RecordedStep(
    string Name,
    int Step,
    string Method,
    string Path,
    string? ContentType,
    byte[] RequestBody,
    int Status,
    string? ResponseContentType,
    byte[] ResponseBody,
    IReadOnlyList<RecordedFrame>? Frames,
    JsonElement Expect);

/// <summary>One recorded frame: the flag byte and the payload.</summary>
public sealed record RecordedFrame(Loams.EnvelopeFlags Flags, byte[] Payload);

/// <summary>A running endpoint and how to stop it.</summary>
public sealed class FixtureServer : IDisposable
{
    private readonly IDisposable? _owned;
    private bool _disposed;

    private FixtureServer(string endpoint, bool live, IDisposable? owned)
    {
        Endpoint = endpoint;
        Live = live;
        _owned = owned;
    }

    /// <summary>The base URL a client is built with.</summary>
    public string Endpoint { get; }

    /// <summary>
    /// Whether this is a real server rather than a replay, which only changes what a
    /// skip is allowed to claim.
    /// </summary>
    public bool Live { get; }

    /// <summary>Brings up whichever endpoint is available.</summary>
    /// <param name="fixturesDir">The corpus root, for the in-process replay.</param>
    /// <param name="conformanceDir">The harness directory, for the Node server.</param>
    public static FixtureServer Start(string fixturesDir, string conformanceDir)
    {
        ArgumentException.ThrowIfNullOrWhiteSpace(fixturesDir);
        ArgumentException.ThrowIfNullOrWhiteSpace(conformanceDir);

        var live = Environment.GetEnvironmentVariable("LOAMS_TEST_ENDPOINT");
        if (!string.IsNullOrWhiteSpace(live))
        {
            return new FixtureServer(live.TrimEnd('/'), live: true, owned: null);
        }

        if (TryStartNode(conformanceDir, out var spawned) && spawned is not null)
        {
            return spawned;
        }

        return ReplayServer.Start(fixturesDir);
    }

    private static bool TryStartNode(string conformanceDir, out FixtureServer? server)
    {
        server = null;
        var script = Path.Combine(conformanceDir, "fixture-server.mjs");
        if (!File.Exists(script))
        {
            return false;
        }

        var start = new ProcessStartInfo("node")
        {
            RedirectStandardOutput = true,
            RedirectStandardError = true,
            UseShellExecute = false,
            CreateNoWindow = true,
        };
        start.ArgumentList.Add(script);
        start.ArgumentList.Add("--port");
        start.ArgumentList.Add("0");
        start.ArgumentList.Add("--fixtures");
        start.ArgumentList.Add(Path.GetFullPath(Path.Combine(conformanceDir, "..", "fixtures")));

        Process? process;
        try
        {
            process = Process.Start(start);
        }
        catch (System.ComponentModel.Win32Exception)
        {
            // No Node on PATH. The in-process replay is the fallback, not a failure.
            return false;
        }
        if (process is null)
        {
            return false;
        }

        // The server prints `{"url":"http://127.0.0.1:PORT"}` on stdout once it is
        // listening, then serves until it is killed. A 30 s ceiling on that read, so
        // a server that starts and then hangs cannot hang the suite.
        var deadline = DateTime.UtcNow.AddSeconds(30);
        string? endpoint = null;
        while (DateTime.UtcNow < deadline && endpoint is null)
        {
            var line = process.StandardOutput.ReadLine();
            if (line is null)
            {
                break;
            }
            if (TryReadUrl(line, out var parsed))
            {
                endpoint = parsed;
            }
        }

        if (endpoint is null)
        {
            TryKill(process);
            return false;
        }

        server = new FixtureServer(endpoint, live: false, owned: new ProcessOwner(process));
        return true;
    }

    private static bool TryReadUrl(string line, out string? endpoint)
    {
        endpoint = null;
        try
        {
            using var document = JsonDocument.Parse(line);
            if (document.RootElement.TryGetProperty("url", out var url) &&
                url.ValueKind == JsonValueKind.String)
            {
                endpoint = url.GetString();
            }
        }
        catch (JsonException)
        {
            // The harness prints only JSON on stdout, but a stray line must not fail
            // the suite: keep reading.
        }
        return endpoint is not null;
    }

    private static void TryKill(Process process)
    {
        try
        {
            process.Kill(entireProcessTree: true);
        }
        catch (InvalidOperationException)
        {
            // Already gone, which is the outcome that was wanted.
        }
        process.Dispose();
    }

    /// <summary>Stops the server this owns. A caller-supplied endpoint is left alone.</summary>
    public void Dispose()
    {
        if (_disposed)
        {
            return;
        }
        _disposed = true;
        _owned?.Dispose();
    }

    /// <summary>Kills the spawned Node process on dispose.</summary>
    private sealed class ProcessOwner(Process process) : IDisposable
    {
        public void Dispose() => TryKill(process);
    }

    /// <summary>
    /// A replay server over `sdks/fixtures/recorded`, with the same matching rules
    /// as `fixture-server.mjs`.
    /// </summary>
    /// <remarks>
    /// Three rules, and each is a way a replay that ignored the request would pass an
    /// SDK that frames a gRPC-Web message wrongly:
    ///
    ///   - the key is <c>method + path + content family</c>, so a client that picks a
    ///     different encoding than the recording gets a <b>404 naming the gap</b> and
    ///     not a silent success;
    ///   - the **request body** is compared too, byte for byte;
    ///   - a key with more than one candidate is a <b>409 naming the candidates</b>
    ///     unless the request says which with <c>loams-fixture-name</c>, because six
    ///     recorded scenarios are all <c>WatchApprovals</c> over Connect JSON and a
    ///     silent pick would hand a suite the bytes of a fixture it did not ask for.
    /// </remarks>
    private sealed class ReplayServer : IDisposable
    {
        private readonly HttpListener _listener;
        private readonly List<RecordedStep> _steps = [];
        private bool _disposed;

        private ReplayServer(HttpListener listener, IReadOnlyList<RecordedStep> steps, int port)
        {
            _listener = listener;
            _steps.AddRange(steps);
            Port = port;
            _listener.Start();
            _ = Task.Run(ServeAsync);
        }

        /// <summary>The port the listener bound to.</summary>
        public int Port { get; }

        /// <summary>Builds and starts a replay over a corpus directory.</summary>
        public static FixtureServer Start(string fixturesDir)
        {
            var steps = LoadSteps(fixturesDir);
            if (steps.Count == 0)
            {
                throw new InvalidOperationException(
                    $"no recorded fixtures under {fixturesDir}/recorded, so there is nothing to replay");
            }

            // `HttpListener` has no "any free port": it takes a prefix and reports
            // no bound endpoint, so a port has to be chosen first. The probe below
            // binds one, reads what the kernel gave it and lets it go, which leaves
            // a window in which something else could take it. That window is why
            // `LOAMS_TEST_ENDPOINT` is checked first and why the Node server is
            // preferred when Node is on PATH: both let the kernel hold the socket
            // that serves, and only this last-resort replay races for it.
            var port = FreePort();
            var listener = new HttpListener();
            listener.Prefixes.Add($"http://127.0.0.1:{port}/");
            var replay = new ReplayServer(listener, steps, port);
            return new FixtureServer($"http://127.0.0.1:{port}", live: false, owned: replay);
        }

        /// <summary>A port the kernel says is free, released again immediately.</summary>
        private static int FreePort()
        {
            var probe = new System.Net.Sockets.TcpListener(System.Net.IPAddress.Loopback, 0);
            probe.Start();
            try
            {
                return ((System.Net.IPEndPoint)probe.LocalEndpoint).Port;
            }
            finally
            {
                probe.Stop();
            }
        }

        private static IReadOnlyList<RecordedStep> LoadSteps(string fixturesDir)
        {
            var steps = new List<RecordedStep>();
            foreach (var directory in new[]
                     {
                         Path.Combine(fixturesDir, "recorded"),
                         Path.Combine(fixturesDir, "recorded", "apps-mock"),
                     })
            {
                if (!Directory.Exists(directory))
                {
                    continue;
                }
                foreach (var file in Directory.GetFiles(directory, "*.json").Order(StringComparer.Ordinal))
                {
                    steps.AddRange(ReadFixture(file));
                }
            }
            return steps;
        }

        /// <summary>
        /// Every step of one recording, read through <see cref="CorpusRecording"/>.
        /// </summary>
        /// <remarks>
        /// The in-process replay and the corpus driver read the corpus through the
        /// **same** reader. They had two copies of it, and the copies disagreed about
        /// a recorded body that is a JSON object rather than a string: this one read
        /// only the string form and served an **empty** body for every recorded Connect
        /// refusal, so a suite running without Node on PATH would have seen a recorded
        /// `failed_precondition` as an empty 400. A disagreement between what a suite
        /// thinks the corpus says and what it was served is the most expensive kind of
        /// bug in a harness, so there is one reader.
        /// </remarks>
        private static IReadOnlyList<RecordedStep> ReadFixture(string file) => CorpusRecording.ReadSteps(file);

        private async Task ServeAsync()
        {
            while (!_disposed)
            {
                HttpListenerContext context;
                try
                {
                    context = await _listener.GetContextAsync().ConfigureAwait(false);
                }
                catch (HttpListenerException)
                {
                    // The listener was stopped, which is the shutdown path.
                    return;
                }
                catch (ObjectDisposedException)
                {
                    return;
                }
                _ = Task.Run(() => Answer(context));
            }
        }

        private async Task Answer(HttpListenerContext context)
        {
            try
            {
                var request = context.Request;
                byte[] sent;
                using (var buffer = new MemoryStream())
                {
                    await request.InputStream.CopyToAsync(buffer).ConfigureAwait(false);
                    sent = buffer.ToArray();
                }

                var key = $"{request.HttpMethod} {request.Url!.AbsolutePath} {Family(request.ContentType)}";
                var candidates = _steps.Where((step) =>
                        $"{step.Method} {step.Path} {Family(step.ContentType)}" == key)
                    .ToArray();

                if (candidates.Length == 0)
                {
                    await Fail(context, 404, new
                    {
                        error = $"no recorded fixture for {key}",
                        recorded = _steps.Select((step) => $"{step.Method} {step.Path} {Family(step.ContentType)}")
                            .Distinct().Order(StringComparer.Ordinal).ToArray(),
                    }).ConfigureAwait(false);
                    return;
                }

                var resolution = Resolve(candidates, key, request.Headers["loams-fixture-name"],
                    request.Headers["loams-fixture-step"]);
                if (resolution.Conflict is not null)
                {
                    await Fail(context, 409, resolution.Conflict).ConfigureAwait(false);
                    return;
                }
                if (resolution.Step is not { } step)
                {
                    await Fail(context, 404, new { error = resolution.Error ?? $"nothing answers {key}" })
                        .ConfigureAwait(false);
                    return;
                }

                if (!sent.AsSpan().SequenceEqual(step.RequestBody))
                {
                    await Fail(context, 400, new
                    {
                        error = $"the request does not match the recorded one for {step.Name} step {step.Step}",
                        expected = Convert.ToBase64String(step.RequestBody),
                        sent = Convert.ToBase64String(sent),
                    }).ConfigureAwait(false);
                    return;
                }


                var response = context.Response;
                response.StatusCode = step.Status;
                if (step.ResponseContentType is { Length: > 0 })
                {
                    response.ContentType = step.ResponseContentType;
                }

                // A recording that carries its frames separately is a stream, and it
                // is written frame by frame so the client observes them arriving one
                // at a time — which is the only way a heartbeat or a cursor is
                // observable at all.
                if (step.Frames is { Count: > 0 })
                {
                    foreach (var frame in step.Frames)
                    {
                        await response.OutputStream.WriteAsync(Loams.Envelopes.Wrap(frame.Payload, frame.Flags))
                            .ConfigureAwait(false);
                    }
                    response.Close();
                    return;
                }

                await response.OutputStream.WriteAsync(step.ResponseBody).ConfigureAwait(false);
                response.Close();
            }
            catch (HttpListenerException)
            {
                // The client hung up mid-answer, which a cancellation looks like.
            }
            catch (ObjectDisposedException)
            {
                // Shut down underneath a request in flight.
            }
        }

        /// <summary>The resolution of one request: a step, a 409's body, or an error.</summary>
        /// <param name="Step">The step that answers, when there is one.</param>
        /// <param name="Conflict">The body of a 409, when the key is ambiguous.</param>
        /// <param name="Error">The message of a 404, when nothing answers.</param>
        private readonly record struct Resolution(RecordedStep? Step, object? Conflict, string? Error);

        /// <summary>
        /// Which recording answers a request, mirroring `fixture-server.mjs`.
        /// </summary>
        /// <remarks>
        /// Three cases, in this order:
        ///
        ///   - **one fixture** answers the key: take its step, inferring which by the
        ///     step header or by which recorded step has this request's key. That
        ///     inference is what lets a scenario walk its steps without counting
        ///     them, and it is why the header is only genuinely needed when one
        ///     fixture has two steps on the same key — the idempotency replay, which
        ///     sends the same request twice.
        ///   - **several fixtures** answer and the request names one: take it.
        ///   - **several fixtures** answer and the request names none: a **409**.
        ///     Five recorded scenarios are all <c>WatchApprovals</c> over Connect JSON,
        ///     and a silent pick would hand a suite the bytes of a fixture it did not
        ///     ask for.
        /// </remarks>
        private static Resolution Resolve(RecordedStep[] candidates, string key, string? named, string? wantedStep)
        {
            var byFixture = candidates.GroupBy((step) => step.Name, StringComparer.Ordinal).ToArray();
            RecordedStep[] chosen;
            if (byFixture.Length == 1)
            {
                chosen = byFixture[0].ToArray();
            }
            else if (!string.IsNullOrEmpty(named))
            {
                var match = byFixture.FirstOrDefault((group) => group.Key == named);
                if (match is null)
                {
                    return new Resolution(null, new
                    {
                        error = $"no fixture named {named} for {key}",
                        candidates = byFixture.Select((group) => group.Key).Order(StringComparer.Ordinal).ToArray(),
                    }, null);
                }
                chosen = match.ToArray();
            }
            else
            {
                return new Resolution(null, new
                {
                    error = $"{byFixture.Length} fixtures answer {key}; say which with loams-fixture-name",
                    header = "loams-fixture-name",
                    candidates = byFixture.Select((group) => group.Key).Order(StringComparer.Ordinal).ToArray(),
                }, null);
            }

            // One candidate and it is not step 0: a scenario whose later step is the
            // only one with this key (the mock's `GetInstance` answers JSON then
            // proto), so the key alone already selects the step.
            if (chosen.Length == 1)
            {
                return new Resolution(chosen[0], null, null);
            }

            if (int.TryParse(wantedStep, out var at))
            {
                return chosen.FirstOrDefault((step) => step.Step == at) is { } named_
                    ? new Resolution(named_, null, null)
                    : new Resolution(null, null,
                        $"{chosen[0].Name} has {chosen.Length} step(s); step {wantedStep} is not one of them");
            }

            // Inferred: the first step whose recorded request is this one. A step
            // whose request differs in encoding (the mock's GetInstance answers JSON
            // then proto) is skipped rather than served with the wrong body.
            foreach (var step in chosen)
            {
                if ($"{step.Method} {step.Path} {Family(step.ContentType)}" == key)
                {
                    return new Resolution(step, null, null);
                }
            }
            return new Resolution(null, null, $"no step of {chosen[0].Name} answers {key}");
        }

        private static async Task Fail(HttpListenerContext context, int status, object body)
        {
            var response = context.Response;
            response.StatusCode = status;
            response.ContentType = "application/json";
            await response.OutputStream.WriteAsync(Encoding.UTF8.GetBytes(JsonSerializer.Serialize(body)))
                .ConfigureAwait(false);
            response.Close();
        }

        /// <summary>
        /// The content-type family a recorded case is keyed on, mirroring `family()`
        /// in `sdks/conformance/encodings.mjs`.
        /// </summary>
        private static string Family(string? contentType)
        {
            var type = (contentType ?? string.Empty).Split(';')[0].Trim();
            return type switch
            {
                "application/grpc-web+json" => "grpc_web_json",
                _ when type.StartsWith("application/grpc-web", StringComparison.Ordinal) => "grpc_web",
                "application/connect+json" => "connect_json",
                _ when type.StartsWith("application/connect", StringComparison.Ordinal) => "connect",
                "application/json" => "json",
                "application/proto" => "proto",
                "" => "none",
                _ => type,
            };
        }

        public void Dispose()
        {
            if (_disposed)
            {
                return;
            }
            _disposed = true;
            try
            {
                _listener.Stop();
                _listener.Close();
            }
            catch (ObjectDisposedException)
            {
                // Already closed.
            }
        }
    }
}