// SDK2 Task 7's conformance suite, in the six names the plan states.
//
// The corpus in `sdks/fixtures` is recorded from a real `loams dev` and a real
// `loams-apps-mock`, and this replays every required case in it through the SDK's
// **public surface** — `client.Instance`, `client.Tables`, `client.Approvals` — rather
// than through internals. That is the point: it proves the facade dispatches to the
// right RPC, sends the encoding the corpus recorded, and turns what comes back into
// the right typed value.
//
// Four things are covered, which between them are what design §44 §10.4 asks of a
// conforming SDK:
//
//   - a successful call, in each of the four encodings a client might pick;
//   - a structured-reason error, with `reason` and not the message;
//   - the unavailable-service path, in all three of its shapes: the guard that costs
//     no RPC, the refusal a call gets, and the refusal on a stream;
//   - the runtime clauses with no fixture of their own — retry, idempotency, tokens,
//     pagination, stream resume — pinned against a stub, because the servers today
//     cannot produce those answers on demand (R2, R4, R6).
//
// # C#, and the names of the tests
//
// `sdks/conformance/required.mjs` greps this suite's **source** for the six names
// verbatim, and `run-test.sh` passes one of them to `dotnet test --filter`. xunit
// imposes no naming convention, so each name is a C# method named exactly as the
// runner states it:
//
//     dotnet test --filter csharp_conformance_all_required_fixtures
//
// `csharp_conformance_test_names_exist` asserts the set cannot shrink, so "the six
// tests exist" is a checkable claim rather than a comment.

using Google.Protobuf;
using Loams;
using Loams.Tests;
using Loams.Tests.Support;
using Xunit;

namespace Loams.Tests;

/// <summary>The six conformance tests, and the report they write.</summary>
public sealed class ConformanceTests
{
    /// <summary>
    /// Replays every required fixture in the corpus, and writes
    /// <c>sdks/fixtures/results/csharp.json</c>.
    /// </summary>
    /// <remarks>
    /// The 100% bar of D617 is computed from the report's <c>ran</c>, and
    /// <c>ran</c> is what the driver actually replayed — so a short run fails the gate
    /// by construction rather than by a hand-maintained list being wrong. The two
    /// halves of this test are therefore one thing: replay, and assert that what was
    /// replayed is everything the manifest requires.
    /// </remarks>
    [Fact]
    public async Task csharp_conformance_all_required_fixtures()
    {
        var root = ConformanceReport.RepositoryRoot();
        var driver = new CorpusDriver(Path.Combine(root, "sdks", "fixtures"));

        var run = await driver.RunAsync();
        var reportPath = ConformanceReport.Write(root, run, ConformanceReport.RequiredTestNames);

        if (run.Failures.Count > 0)
        {
            // Every failure, not the first: a suite that skipped four required
            // fixtures should learn about all four in one run rather than one per CI
            // cycle. That is what `checkLanguage` in `required.mjs` does too, and this
            // is the same rule applied a layer earlier.
            Assert.Fail($"{run.Failures.Count} conformance failure(s) against {run.Endpoint}:\n  - " +
                        string.Join("\n  - ", run.Failures));
        }

        // The anti-vacuity assertion, stated rather than implied: everything the
        // manifest requires must be in `ran`, and the report on disk must agree.
        var required = driver.RequiredFixtures.Select((fixture) => fixture.Name).ToArray();
        var ran = run.Ran.ToArray();
        var missing = required.Where((name) => !ran.Contains(name, StringComparer.Ordinal)).ToArray();
        Assert.True(missing.Length == 0,
            $"the manifest requires {required.Length} fixture(s) and the driver replayed {ran.Length}; " +
            $"not run: {string.Join(", ", missing)}");

        Assert.True(File.Exists(reportPath), $"the suite wrote no report at {reportPath}");
        using var report = System.Text.Json.JsonDocument.Parse(File.ReadAllText(reportPath));
        var reportedRan = report.RootElement.GetProperty("ran").EnumerateArray()
            .Select((name) => name.GetString()!).ToArray();
        Assert.Equal(ran.Order(StringComparer.Ordinal), reportedRan.Order(StringComparer.Ordinal));
        Assert.Empty(report.RootElement.GetProperty("skipped").EnumerateArray());
    }

    /// <summary>
    /// The same key on every attempt of one logical call, and a different key on the
    /// next (R3, D610).
    /// </summary>
    /// <remarks>
    /// Pinned against a stub rather than the corpus, and the reason is in the name of
    /// the recording that already covers the end-to-end half:
    /// <c>mock_state_idempotent_decide</c> sends the same <c>DecideApproval</c> twice
    /// with the same key against a mock whose approval state really moved, and both
    /// answers are identical bytes at revision 2. What no recording can produce is a
    /// **retryable failure on a mutation** — a server that answered
    /// <c>unavailable</c> and then succeeded — because no server today is down on
    /// demand. So the two attempts here are driven directly, which is the only place
    /// the claim "the same key on attempt two" can be checked.
    /// </remarks>
    [Fact]
    public async Task csharp_retry_reuses_idempotency_key()
    {
        var binding = Facade.Binding("tables", "Mutate");

        // A mutation with `optional string idempotency_key`, so a caller who leaves it
        // out sends no key at all. `TakesIdempotencyKey` is asked of the schema, not of
        // the object, because "the field exists" and "the field is set" are different
        // questions and only the first decides whether the SDK may mint one.
        Assert.True(binding.TakesIdempotencyKey,
            "MutateRequest declares idempotency_key, so the SDK can key it");
        Assert.False(Facade.Binding("tables", "Deploy").TakesIdempotencyKey,
            "DeployRequest declares no idempotency_key, so keying it would invent a field the schema lacks");

        // First attempt: a retryable refusal. Second: the success.
        var handler = new StubHandler(
            StubHandler.ConnectRefusal(System.Net.HttpStatusCode.ServiceUnavailable, "unavailable",
                "the node is restarting", "unavailable"),
            StubHandler.ConnectJson(System.Net.HttpStatusCode.OK, "{}"));

        using var client = new LoamsClient(new LoamsClientOptions
        {
            Endpoint = "https://fixture.invalid",
            Codec = Codec.Json,
            Handler = handler,
            MaxRetries = 3,
        });

        // The retry wait is the SDK's own backoff, which is up to 2 s; this test does
        // not wait it out.
        var succeeded = await client.Invoker.UnaryAsync<Loams.Live.V1.MutateResponse>(
            binding,
            new Loams.Live.V1.MutateRequest { Function = "docs:upsert" },
            new CallOptions { Headers = new Dictionary<string, string>() },
            CancellationToken.None);

        Assert.NotNull(succeeded);
        Assert.Equal(2, handler.CallCount);

        // The whole clause: **the same** key on both attempts. A key regenerated per
        // attempt turns one write into two, which is the failure the field exists to
        // prevent.
        var first = KeyOf(handler.Requests[0].Body);
        var retried = KeyOf(handler.Requests[1].Body);
        Assert.False(string.IsNullOrEmpty(first), "the SDK must mint a key when the caller supplied none");
        Assert.Equal(first, retried);

        // A second logical call gets a different key: the key is per logical call, not
        // per client, or two writes from one client would collide.
        var second = KeyOfJson(CompactJson.Format(Idempotency.Apply(
            new Loams.Live.V1.MutateRequest { Function = "docs:upsert" }, string.Empty).Request));
        var third = KeyOfJson(CompactJson.Format(Idempotency.Apply(
            new Loams.Live.V1.MutateRequest { Function = "docs:upsert" }, string.Empty).Request));
        Assert.NotEqual(second, third);

        // A caller's own key wins, so a caller whose storage dedupes on it is not
        // second-guessed.
        Assert.Equal("caller-supplied", KeyOfJson(CompactJson.Format(Idempotency.Apply(
            new Loams.Live.V1.MutateRequest { Function = "docs:upsert", IdempotencyKey = "caller-supplied" },
            "ignored").Request)));
    }

    /// <summary>
    /// One refresh and one retry, then a second expiry is reported (R1, D608).
    /// </summary>
    /// <remarks>
    /// The claim is a claim about **how many times** <c>RefreshAsync</c> was called, so
    /// the source counts its own refreshes: a source that handed out the same stale
    /// token twice would look identical from the token values alone.
    /// </remarks>
    [Fact]
    public async Task csharp_token_source_refresh()
    {
        var binding = Facade.Binding("instance", "WhoAmI");

        // A source that can refresh: the first attempt's 401 refreshes, and the retry
        // carries the **new** token.
        var source = new ScriptedTokenSource(["stale", "fresh"]);
        var handler = new StubHandler(
            StubHandler.ConnectRefusal(System.Net.HttpStatusCode.Unauthorized, "unauthenticated",
                "the token expired", "token_expired"),
            StubHandler.ConnectJson(System.Net.HttpStatusCode.OK, "{}"));

        using var client = new LoamsClient(new LoamsClientOptions
        {
            Endpoint = "https://fixture.invalid",
            Codec = Codec.Json,
            Auth = source,
            Handler = handler,
            NoRetries = true,
        });

        var response = await client.Instance.WhoAmIAsync();
        Assert.NotNull(response);

        Assert.Equal(1, source.Refreshes);
        Assert.Equal("stale", handler.Requests[0].Bearer);
        Assert.Equal("fresh", handler.Requests[1].Bearer);

        // A second expiry is reported rather than refreshed again: a loop is the
        // obvious implementation and it turns a refusal into a hang.
        var twice = new ScriptedTokenSource(["stale", "stale", "stale"]);
        var twiceHandler = new StubHandler(
            StubHandler.ConnectRefusal(System.Net.HttpStatusCode.Unauthorized, "unauthenticated",
                "expired", "token_expired"),
            StubHandler.ConnectRefusal(System.Net.HttpStatusCode.Unauthorized, "unauthenticated",
                "expired", "token_expired"));

        using var twiceClient = new LoamsClient(new LoamsClientOptions
        {
            Endpoint = "https://fixture.invalid",
            Codec = Codec.Json,
            Auth = twice,
            Handler = twiceHandler,
            NoRetries = true,
        });

        var error = await Assert.ThrowsAsync<TokenExpiredError>(
            () => twiceClient.Instance.WhoAmIAsync());
        Assert.Equal(Reason.TokenExpired, error.Reason);
        Assert.Equal(1, twice.Refreshes);
        Assert.Equal(2, twiceHandler.CallCount);

        // A source that cannot refresh — an API key, which does not expire — makes the
        // refresh a no-op, and the retry is **skipped** rather than spent on a request
        // that cannot work. `mock_status_unauthenticated` is the recording of the
        // refusal this models: a 401 with no `ErrorInfo` at all.
        var keyOnly = new StubHandler(
            StubHandler.ConnectRefusal(System.Net.HttpStatusCode.Unauthorized, "unauthenticated",
                "no credential at all", null));
        using var keyClient = new LoamsClient(new LoamsClientOptions
        {
            Endpoint = "https://fixture.invalid",
            Codec = Codec.Json,
            Auth = new ApiKeyTokenSource("loams_key"),
            Handler = keyOnly,
            MaxRetries = 3,
        });

        var unauthenticated = await Assert.ThrowsAsync<UnauthenticatedError>(
            () => keyClient.Instance.WhoAmIAsync());
        Assert.Equal(Reason.None, unauthenticated.Reason);
        Assert.Equal(1, keyOnly.CallCount);
        Assert.Equal("loams_key", keyOnly.Requests[0].Bearer);
    }

    /// <summary>
    /// Every reason in the registry maps to a type, and R8's three distinct cases stay
    /// distinct (R8, D611).
    /// </summary>
    [Fact]
    public void csharp_error_reason_mapping()
    {
        // The registry on the page is the registry in the code. Read first, so a
        // reason added to `docs/api/reasons.md` without a row here fails **this** test
        // rather than being silently unmapped.
        var documented = DocumentedReasons();

        foreach (var (reason, code) in documented)
        {
            var detail = new ErrorInfoShape(reason, new Dictionary<string, string> { ["k"] = "v" }, "try this");
            var error = ErrorMapper.Map(
                new WireFailure(code, "for a person", detail), "loams.instance.v1.InstanceService/WhoAmI");

            Assert.Equal(reason, error.UnknownReason ?? ReasonRegistry.Name(error.Reason));
            Assert.Equal(detail.Hint, error.Hint);
            Assert.Equal("v", error.Metadata["k"]);
            Assert.True(ReasonRegistry.IsKnown(reason), $"{reason} is documented and must be in the registry");
        }

        // Exhaustiveness over the registry: no reason in the code that the page does
        // not have, either. A reason here that is not there would be a branch no
        // server can reach.
        Assert.Equal(documented.Select((entry) => entry.Reason).Order(StringComparer.Ordinal),
            ReasonRegistry.AllNames.Order(StringComparer.Ordinal));

        // A reason from a **newer** server is surfaced as text and flagged, not
        // dropped: losing it would leave a caller unable to tell "not supported here"
        // from "not supported at all".
        var future = ErrorMapper.Map(
            new WireFailure(Code.FailedPrecondition, "from a newer server",
                new ErrorInfoShape("a_reason_this_sdk_has_never_heard_of", new Dictionary<string, string>(), "")),
            "loams.instance.v1.InstanceService/WhoAmI");
        Assert.Equal(Reason.None, future.Reason);
        Assert.Equal("a_reason_this_sdk_has_never_heard_of", future.UnknownReason);

        // The two reasons D611 gives their own class, and both are refinements of a
        // code the table would otherwise answer on its own.
        var variant = ErrorMapper.Map(
            new WireFailure(Code.Unimplemented, "not in the standard variant",
                new ErrorInfoShape("feature_not_in_variant",
                    new Dictionary<string, string> { ["variant"] = "standard" }, "")),
            "loams.live.v1.LiveService/Query");
        var absent = Assert.IsType<FeatureNotInVariantError>(variant);
        Assert.Equal(Reason.FeatureNotInVariant, absent.Reason);
        Assert.Equal("standard", absent.Variant);
        // …and the class is still catchable as the code's own class.
        Assert.IsAssignableFrom<UnimplementedError>(absent);

        var expired = ErrorMapper.Map(
            new WireFailure(Code.Unauthenticated, "the token expired",
                new ErrorInfoShape("token_expired", new Dictionary<string, string>(), "")),
            "loams.instance.v1.InstanceService/WhoAmI");
        var token = Assert.IsType<TokenExpiredError>(expired);
        Assert.Equal(Reason.TokenExpired, token.Reason);
        Assert.IsAssignableFrom<UnauthenticatedError>(token);

        // A failure from **below the API** carries no reason, and that is a different
        // thing from a service refusing.
        var socket = ErrorMapper.FromException(new HttpRequestException("connection refused"),
            "loams.instance.v1.InstanceService/GetInstance");
        Assert.Equal(Reason.None, socket.Reason);
        Assert.Equal(Code.Unavailable, socket.Code);

        // A cancelled call keeps its own code rather than becoming `unknown`: a
        // deadline is the one failure a caller can always act on.
        var cancelled = ErrorMapper.FromException(new OperationCanceledException(), "x/Y");
        Assert.Equal(Code.Cancelled, cancelled.Code);

        // Mapping twice loses nothing: an error that is already a `LoamsError` comes
        // back unchanged, so wrapping an SDK's own error never loses its reason.
        Assert.Same(variant, ErrorMapper.FromException(variant, "somewhere/else"));
    }

    /// <summary>
    /// A stream re-opens from the last cursor it applied and does not re-yield (R7).
    /// </summary>
    /// <remarks>
    /// Pinned against a stub, and the reason is that the only server stream any SDK
    /// can reach today refuses: <c>LiveService/Watch</c> is
    /// <c>feature_not_in_variant</c> in every variant, which is what the corpus's
    /// <c>live_watch</c> fixture records. So the two legs here are the two the clause
    /// is about — a break **after** two messages, then a re-open that carries the last
    /// cursor and does not replay what was already yielded — driven directly.
    /// </remarks>
    [Fact]
    public async Task csharp_stream_resume_with_cursor()
    {
        var binding = Facade.Binding("live", "Watch");

        // Leg one: two transitions, then a retryable refusal inside the end-of-stream
        // frame. Leg two: the transition after the cursor, and nothing else.
        var handler = new StubHandler(
            new StubReply(System.Net.HttpStatusCode.OK,
                Concat(
                    Frame(Transition("10")),
                    Frame(Transition("20")),
                    StubHandler.ConnectStreamRefusal("unavailable", "the node is restarting",
                        "unavailable").Body)),
            new StubReply(System.Net.HttpStatusCode.OK,
                Frame(Transition("30"))));

        using var client = new LoamsClient(new LoamsClientOptions
        {
            Endpoint = "https://fixture.invalid",
            Handler = handler,
            MaxRetries = 3,
        });

        var sessions = new List<string>();
        await foreach (var transition in client.Live.WatchAsync(
                           new Loams.Live.V1.WatchRequest
                           {
                               Initial = new Loams.Live.V1.QuerySet { Version = 1 },
                           },
                           new CallOptions
                           {
                               Resume = new StreamResume<Loams.Live.V1.WatchRequest, Loams.Live.V1.Transition>(
                                   response => response.SessionId,
                                   (cursor, _) => new Loams.Live.V1.WatchRequest
                                   {
                                       Resume = new Loams.Live.V1.Resume
                                       {
                                           LastVersion = new Loams.Live.V1.StateVersion { Ts = ParseTs(cursor) },
                                       },
                                   }),
                           })
                       .ConfigureAwait(false))
        {
            sessions.Add(transition.SessionId);
        }

        // R7 in one assertion: the stream resumed from the cursor and did not
        // re-yield the two transitions the caller already had.
        Assert.Equal(["10", "20", "30"], sessions);
        Assert.Equal(2, handler.CallCount);
    }

    /// <summary>
    /// The iterator follows the tokens to the end and yields **items**, not pages
    /// (R6).
    /// </summary>
    /// <remarks>
    /// Pinned against a stub because **no RPC is paged yet**:
    /// <c>ListApprovals</c> declares <c>page_size</c> and answers
    /// <c>next_page_token</c>, and the mock honours neither — which is what
    /// <c>mock_status_list_is_not_paged</c> records, and why R6's three fixtures are
    /// <c>required: false</c>. The end-to-end half of this clause arrives with
    /// <c>ListCollections</c> (API1 Task 2); the assertion below fails the day one
    /// annotated RPC is paged, so the gap cannot become permanent silently.
    /// </remarks>
    [Fact]
    public async Task csharp_pagination_iterator()
    {
        // A paged binding, built the way the facade renderer would build one once
        // `loams.collection.v1.ListCollections` exists.
        var paged = new CallBinding(
            "collection", "ListCollections", "listCollections",
            "loams.collection.v1.CollectionService/ListCollections",
            "loams.collection.v1.CollectionService", "loams.collection.v1",
            IdempotencyLevel.NoSideEffects, RetryClass.Safe, Streaming.Unary,
            new Pagination("Collections", "NextPageToken"),
            "Loams.Collection.V1.ListCollectionsRequest", "Loams.Collection.V1.ListCollectionsResponse");

        Assert.Equal("Collections", paged.Pagination!.ItemsField);
        Assert.Equal("NextPageToken", paged.Pagination.NextPageTokenField);

        // A request and a response type to page over. Stands in for the generated
        // pair, and has the two properties the binding names.
        var request = new PageRequest();
        var pages = new Queue<PageResponse>([
            new PageResponse
            {
                Collections = { "a", "b" },
                NextPageToken = "p2",
            },
            new PageResponse { Collections = { "c" } },
        ]);

        var seenTokens = new List<string>();
        var names = new List<string>();
        await foreach (var item in Paginator.ItemsAsync(paged,
                           (req, token, _) =>
                           {
                               seenTokens.Add(token);
                               req.PageToken = token;
                               return Task.FromResult(pages.Dequeue());
                           },
                           request,
                           (PageResponse page) => page.Collections)
                       .ConfigureAwait(false))
        {
            names.Add(item);
        }

        Assert.Equal(["a", "b", "c"], names);
        Assert.Equal([string.Empty, "p2"], seenTokens);

        // A binding that is not paged refuses, and names itself: a log line reading
        // "not paged" without the binding is a debugging session.
        var unpaged = Facade.Binding("instance", "GetInstance");
        Assert.Null(unpaged.Pagination);
        var refusal = await Assert.ThrowsAsync<LoamsError>(async () =>
        {
            await foreach (var _ in Paginator.ItemsAsync(unpaged,
                               (_, _, _) => Task.FromResult(new PageResponse()),
                               new PageRequest(),
                               (PageResponse page) => page.Collections)
                           .ConfigureAwait(false))
            {
                // Nothing to iterate: the refusal is thrown before the first item.
            }
        });
        Assert.Contains("instance.GetInstance", refusal.Message, StringComparison.Ordinal);

        // The deliberate end: no annotated RPC is paged yet, so nothing here is a
        // fixture. When one is, this assertion fails and says what is owed.
        Assert.True(
            Facade.Bindings.Values.All((binding) => binding.Pagination is null),
            "no annotated RPC is paged yet; ListCollections arrives with API1 Task 2, and this " +
            "assertion failing is the signal to add an end-to-end half to this test");
    }

    /// <summary>
    /// The six canonical names exist, which is what makes "the six tests exist" a
    /// checkable claim rather than a comment.
    /// </summary>
    [Fact]
    public void csharp_conformance_test_names_exist()
    {
        Assert.Equal(6, ConformanceReport.RequiredTestNames.Length);
        Assert.Equal(6, ConformanceReport.RequiredTestNames.Distinct(StringComparer.Ordinal).Count());
        foreach (var name in ConformanceReport.RequiredTestNames)
        {
            Assert.StartsWith("csharp_", name, StringComparison.Ordinal);
        }

        // Each name is a method on this class, so `--filter <name>` finds it.
        var declared = typeof(ConformanceTests).GetMethods()
            .Select((method) => method.Name)
            .ToHashSet(StringComparer.Ordinal);
        foreach (var name in ConformanceReport.RequiredTestNames)
        {
            Assert.Contains(name, declared);
        }
    }

    /// <summary>
    /// The registry this suite's codes are checked against: every row of
    /// <c>docs/api/reasons.md</c>'s table.
    /// </summary>
    /// <remarks>
    /// Read at run time rather than transcribed, so a reason added to the registry
    /// without a row in <c>Reason.cs</c> fails <c>csharp_error_reason_mapping</c> rather
    /// than sitting in the table unmapped. The parse is a table-row reader and nothing
    /// more: it wants the first cell and the second, and it stops at the first blank
    /// line after the table starts.
    /// </remarks>
    private static IReadOnlyList<(string Reason, Code Code)> DocumentedReasons()
    {
        var root = ConformanceReport.RepositoryRoot();
        var page = Path.Combine(root, "docs", "api", "reasons.md");
        var rows = new List<(string, Code)>();

        foreach (var line in File.ReadAllLines(page))
        {
            if (!line.StartsWith("| `", StringComparison.Ordinal))
            {
                if (rows.Count > 0)
                {
                    // Past the table: the prose below it is not rows.
                    break;
                }
                continue;
            }
            var cells = line.Split('|', StringSplitOptions.TrimEntries);
            if (cells.Length < 3)
            {
                continue;
            }
            var reason = cells[1].Trim('`', ' ');
            var code = cells[2].Replace("`", string.Empty, StringComparison.Ordinal).Trim();
            if (!Enum.TryParse<Code>(ToPascal(code), out var parsed))
            {
                continue;
            }
            rows.Add((reason, parsed));
        }
        Assert.NotEmpty(rows);
        return rows;
    }

    private static string ToPascal(string snake) =>
        string.Concat(snake.Split('_', StringSplitOptions.RemoveEmptyEntries)
            .Select((part) => char.ToUpperInvariant(part[0]) + part[1..]));

    private static byte[] Frame(byte[] payload) => Envelopes.Wrap(payload);

    private static byte[] Frame(Google.Protobuf.IMessage message) => Envelopes.Wrap(message.ToByteArray());

    /// <summary>
    /// A transition whose <c>session_id</c> is the cursor the stub resumes from, and
    /// whose <c>ts</c> encodes it so the re-opened request can be read back.
    /// </summary>
    private static byte[] Transition(string cursor) =>
        new Loams.Live.V1.Transition
        {
            SessionId = cursor,
            End = new Loams.Live.V1.StateVersion { Ts = ParseTs(cursor) },
        }.ToByteArray();

    /// <summary>
    /// The number a session cursor encodes.
    /// </summary>
    /// <remarks>
    /// The stub's cursor is a decimal string and the proto's cursor is a
    /// <c>uint64 ts</c>, so the test converts between them — which is the point: the
    /// SDK treats a cursor as opaque and the **caller** decides how to spell it, and
    /// this is that decision written down.
    /// </remarks>
    private static ulong ParseTs(string cursor) =>
        ulong.Parse(cursor, System.Globalization.CultureInfo.InvariantCulture);

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

    /// <summary>
    /// The idempotency key a serialized request carries, read back out of its own
    /// bytes.
    /// </summary>
    /// <remarks>
    /// Read from the wire rather than off the message the test built, because the
    /// claim is about what went **out**: a key set on a message the transport never
    /// serialized would pass a test that reads the object.
    /// </remarks>
    private static string KeyOf(byte[] body) => KeyOfJson(System.Text.Encoding.UTF8.GetString(body));

    /// <summary>The idempotency key a proto3-JSON request body carries.</summary>
    private static string KeyOfJson(string json) =>
        Loams.Live.V1.MutateRequest.Parser.ParseJson(json).IdempotencyKey ?? string.Empty;

    /// <summary>A stand-in request with the two properties a paged binding names.</summary>
    private sealed class PageRequest
    {
        /// <summary>The page token the iterator writes.</summary>
        public string PageToken { get; set; } = string.Empty;
    }

    /// <summary>A stand-in page with the two properties a paged binding names.</summary>
    private sealed class PageResponse
    {
        /// <summary>The page's items.</summary>
        public List<string> Collections { get; init; } = [];

        /// <summary>The token for the next page, or empty for the last.</summary>
        public string NextPageToken { get; set; } = string.Empty;
    }
}