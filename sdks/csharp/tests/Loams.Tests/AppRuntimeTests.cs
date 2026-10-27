// SDKP Part A (docs/plans/2026-10-19-sdk-parity.md §5, L5): the runtime additions
// R4 (token merge and the session store), R11 (default namespace), R12
// (timeouts), R13 (write_batch) and R14 (subscribe), against stubs.
//
// The token cases are `crates/loams-collection/src/token.rs`'s own tests; when
// `sdks/fixtures/tokens.json` lands (P0d) the same test reads it.

using System.Net;
using System.Text;
using System.Text.Json;
using Google.Protobuf;
using Loams.Collection.V1;
using Loams.Tests.Support;
using Xunit;

namespace Loams.Tests;

public sealed class AppRuntimeTests
{
    internal static LoamsClient ClientOver(HttpMessageHandler handler, string? ns = "app", bool session = false,
        TimeSpan? timeout = null) => new(new LoamsClientOptions
    {
        Endpoint = "https://fixture.invalid",
        Codec = Codec.Json,
        Handler = handler,
        MaxRetries = 3,
        Namespace = ns,
        SessionConsistency = session,
        Timeout = timeout,
    });

    internal static T Body<T>(RecordedRequest request, MessageParser<T> parser) where T : IMessage<T> =>
        parser.ParseJson(Encoding.UTF8.GetString(request.Body));

    private static readonly string[] Malformed =
    [
        "v2:s1/p0@1", "v1:s1/p0", "v1:s01/p0@1", "v1:s1/p0@1,", "", "v1", "s1/p0@1", "v1:,s1/p0@1",
        "v1:s1/p00@1", "v1:s1/p0@01", "v1:s1/p4294967296@1", "v1:s18446744073709551616/p0@1", "v1:s+1/p0@1",
        "v1:s1/p0@-1", "v1:s1p0@1", "v1:S1/P0@1", "v1: s1/p0@1", "v1:s1/p0@1@2", "v1:s/p0@1",
    ];

    [Fact]
    public void csharp_consistency_token_merge()
    {
        var corpus = Path.Combine(ConformanceReport.RepositoryRoot(), "sdks", "fixtures", "tokens.json");
        if (File.Exists(corpus))
        {
            TokenCorpus(corpus);
        }

        // Display and parse.
        var one = ConsistencyToken.Parse("v1:s7/p3@918274");
        Assert.Equal(new TokenPosition(7, 3, 918274), Assert.Single(one.Positions));
        Assert.Equal("v1:s7/p3@918274", one.ToString());
        Assert.Equal("v1:s0/p0@0,s7/p3@918274", ConsistencyToken.Parse("v1:s0/p0@0,s7/p3@918274").ToString());
        var max = $"v1:s{ulong.MaxValue}/p{uint.MaxValue}@{ulong.MaxValue}";
        Assert.Equal(max, ConsistencyToken.Parse(max).ToString());
        Assert.Equal("v1:", ConsistencyToken.Parse("v1:").ToString());
        Assert.Equal(ConsistencyToken.Empty, ConsistencyToken.Parse("v1:"));

        // Merge keeps the max offset per (stream, partition), sorted.
        var merged = ConsistencyToken.Parse("v1:s7/p3@10,s1/p0@5")
            .Merge(ConsistencyToken.Parse("v1:s7/p3@4,s7/p1@2,s1/p0@9"));
        Assert.Equal("v1:s1/p0@9,s7/p1@2,s7/p3@10", merged.ToString());
        Assert.Equal(10UL, merged.Offset(7, 3));
        Assert.Null(merged.Offset(1, 1));
        Assert.Equal("v1:s1/p5@3,s2/p0@8", ConsistencyToken.Of([new(2, 0, 1), new(1, 5, 3), new(2, 0, 8), new(1, 5, 2)]).ToString());
        Assert.Equal(ConsistencyToken.Empty, ConsistencyToken.Empty.Merge(ConsistencyToken.Empty));

        // Malformed tokens are invalid_argument, client-side.
        foreach (var bad in Malformed)
        {
            Assert.False(ConsistencyToken.TryParse(bad, out _), bad);
            var error = Assert.Throws<InvalidArgumentError>(() => ConsistencyToken.Parse(bad));
            Assert.Equal(Code.InvalidArgument, error.Code);
        }
    }

    // The P0d corpus: `{"cases": [{"name", "parse"?, "print"?, "merge"?: [a, b], "expect"?, "error"?}]}`.
    private static void TokenCorpus(string path)
    {
        using var doc = JsonDocument.Parse(File.ReadAllText(path));
        if (!doc.RootElement.TryGetProperty("cases", out var cases))
        {
            return;
        }
        foreach (var c in cases.EnumerateArray())
        {
            var name = c.TryGetProperty("name", out var n) ? n.GetString() : "?";
            if (c.TryGetProperty("error", out var error) && error.ValueKind != JsonValueKind.Null
                && c.TryGetProperty("parse", out var badText))
            {
                Assert.False(ConsistencyToken.TryParse(badText.GetString(), out _), name);
                continue;
            }
            if (c.TryGetProperty("merge", out var merge))
            {
                var tokens = merge.EnumerateArray().Select((t) => ConsistencyToken.Parse(t.GetString()!)).ToList();
                var merged = tokens.Aggregate(ConsistencyToken.Empty, (a, b) => a.Merge(b));
                Assert.Equal(c.GetProperty("expect").GetString(), merged.ToString());
                continue;
            }
            if (c.TryGetProperty("parse", out var text) && c.TryGetProperty("expect", out var expect))
            {
                Assert.Equal(expect.GetString(), ConsistencyToken.Parse(text.GetString()!).ToString());
            }
        }
    }

    [Fact]
    public async Task csharp_session_read_your_writes()
    {
        var handler = new StubHandler(
            StubHandler.ConnectJson(HttpStatusCode.OK, "{\"token\":\"v1:s4/p0@7\"}"),
            StubHandler.ConnectJson(HttpStatusCode.OK, "{\"token\":\"v1:s4/p1@2,s4/p0@3\"}"),
            StubHandler.ConnectJson(HttpStatusCode.OK, "{\"documents\":[],\"readToken\":\"v1:s4/p0@9\"}"),
            StubHandler.ConnectJson(HttpStatusCode.OK, "{}"));
        using var client = ClientOver(handler, session: true);

        await client.Documents.UpsertAsync("notes", [new Doc(1, new { title = "a" })]);
        await client.Documents.DeleteAsync("notes", [2]);
        Assert.Equal("v1:s4/p0@7,s4/p1@2", client.Session!.Current);

        await client.Documents.GetAsync("notes", [1]);
        var get = Body(handler.Requests[2], GetDocumentsRequest.Parser);
        Assert.Equal("v1:s4/p0@7,s4/p1@2", get.Consistency.AtLeast);
        Assert.Equal("v1:s4/p0@9,s4/p1@2", client.Session.Current);

        // An explicit consistency wins over the session.
        await client.Documents.GetAsync("notes", [1], Consistent.Strong());
        var strong = Body(handler.Requests[3], GetDocumentsRequest.Parser);
        Assert.Equal(Freshness.Strong, strong.Consistency.Freshness);
        Assert.Equal(string.Empty, strong.Consistency.AtLeast);

        // Off by default: no session, nothing filled in.
        var plain = new StubHandler(StubHandler.ConnectJson(HttpStatusCode.OK, "{\"readToken\":\"v1:s4/p0@7\"}"));
        using var off = ClientOver(plain);
        await off.Documents.GetAsync("notes", [1]);
        Assert.Null(off.Session);
        Assert.Null(Body(plain.Requests[0], GetDocumentsRequest.Parser).Consistency);
    }

    [Fact]
    public async Task csharp_default_namespace()
    {
        var handler = new StubHandler(StubHandler.ConnectJson(HttpStatusCode.OK, "{\"name\":\"notes\"}"));
        using var client = ClientOver(handler, ns: "app");

        await client.Collections.GetAsync("notes");
        await client.Collections.GetAsync("notes", "other");
        await client.Collections.GetAsync("notes", options: new CallOptions { Namespace = "per-call" });
        Assert.Equal(["app", "other", "per-call"],
            handler.Requests.Select((r) => Body(r, GetCollectionRequest.Parser).Namespace));

        // The caller's request is not mutated.
        var request = new GetCollectionRequest { Collection = "notes" };
        await client.Collections.GetAsync(request);
        Assert.Equal(string.Empty, request.Namespace);

        // No default and no argument: invalid_argument before any network call.
        var none = new StubHandler(StubHandler.ConnectJson(HttpStatusCode.OK, "{}"));
        using var bare = ClientOver(none, ns: null);
        var error = await Assert.ThrowsAsync<InvalidArgumentError>(() => bare.Collections.GetAsync("notes"));
        Assert.Equal("namespace", error.Metadata["field"]);
        Assert.Equal(0, none.CallCount);

        // The older packages are untouched.
        await bare.Instance.GetInstanceAsync();
        Assert.Equal(1, none.CallCount);
    }

    [Fact]
    public async Task csharp_timeout_deadline()
    {
        var handler = new HangingHandler();
        using var client = ClientOver(handler, timeout: TimeSpan.FromMilliseconds(200));

        var error = await Assert.ThrowsAsync<DeadlineExceededError>(() => client.Collections.GetAsync("notes"));
        Assert.Equal(Deadline.ClientDeadlineExceeded, error.UnknownReason);
        Assert.Equal(Code.DeadlineExceeded, error.Code);
        Assert.Equal(1, handler.Calls);
        Assert.True(int.Parse(handler.TimeoutHeader!, System.Globalization.CultureInfo.InvariantCulture) is > 0 and <= 200);

        // A per-call override.
        var quick = await Assert.ThrowsAsync<DeadlineExceededError>(() =>
            client.Collections.GetAsync("notes", options: new CallOptions { Timeout = TimeSpan.FromMilliseconds(50) }));
        Assert.Equal("50", quick.Metadata["timeout_ms"]);

        // A caller's own cancellation is a cancellation, not a deadline.
        using var cts = new CancellationTokenSource(TimeSpan.FromMilliseconds(50));
        var cancelled = await Assert.ThrowsAnyAsync<Exception>(() =>
            client.Collections.GetAsync("notes", cancellationToken: cts.Token));
        Assert.IsNotType<DeadlineExceededError>(cancelled);

        // The default is 30 s.
        Assert.Equal(TimeSpan.FromSeconds(30), CallInvoker.DefaultTimeout);
    }

    [Fact]
    public async Task csharp_write_batch_chunks()
    {
        var handler = new StubHandler(
            StubHandler.ConnectJson(HttpStatusCode.OK, "{\"token\":\"v1:s1/p0@10\"}"),
            StubHandler.ConnectRefusal(HttpStatusCode.ServiceUnavailable, "unavailable", "restarting", "unavailable"),
            StubHandler.ConnectJson(HttpStatusCode.OK, "{\"token\":\"v1:s1/p1@4\"}"),
            StubHandler.ConnectJson(HttpStatusCode.OK, "{\"token\":\"v1:s1/p0@25\"}"));
        using var client = ClientOver(handler);
        var docs = Enumerable.Range(0, 25).Select((i) => new Doc((ulong)i, new { n = i })).ToList();

        var result = await client.Documents.WriteBatchAsync("notes", docs, chunkRows: 10);

        Assert.Equal(3, result.Chunks);
        Assert.Equal(25, result.Ops);
        Assert.Equal("v1:s1/p0@25,s1/p1@4", result.Token);
        var bodies = handler.Requests.Select((r) => Body(r, WriteDocumentsRequest.Parser)).ToList();
        Assert.Equal([10, 10, 10, 5], bodies.Select((b) => b.Ops.Count));
        // The retried chunk reuses its key; every chunk has its own.
        Assert.Equal(bodies[1].IdempotencyKey, bodies[2].IdempotencyKey);
        Assert.Equal(3, bodies.Select((b) => b.IdempotencyKey).Distinct().Count());
        Assert.All(bodies, (b) => Assert.NotEqual(string.Empty, b.IdempotencyKey));

        // Bytes cut a chunk too.
        Assert.Equal(25, WriteBatch.Chunks(docs.Select((d) => d.ToOp()), 10_000, 1).Count());

        // The first failed chunk stops the batch and reports what was applied.
        var failing = new StubHandler(
            StubHandler.ConnectJson(HttpStatusCode.OK, "{\"token\":\"v1:s1/p0@10\"}"),
            StubHandler.ConnectRefusal(HttpStatusCode.BadRequest, "invalid_argument", "bad doc", "invalid_argument"));
        using var other = ClientOver(failing);
        var error = await Assert.ThrowsAsync<BatchError>(() => other.Documents.WriteBatchAsync("notes", docs, chunkRows: 10));
        Assert.Equal(1, error.AppliedChunks);
        Assert.Equal(10, error.AppliedOps);
        Assert.Equal(BatchError.BatchPartiallyApplied, error.UnknownReason);
        Assert.Equal("v1:s1/p0@10", error.Token);
        Assert.Equal(2, failing.CallCount);
    }

    [Fact]
    public async Task csharp_streams_subscribe_resumes()
    {
        var pages = new Queue<IReadOnlyList<StreamRecord>>(
        [
            [Record(0), Record(1)],
            [],
            [],
            [Record(2), Record(3)],
        ]);
        var asked = new List<ulong>();
        var waits = new List<TimeSpan>();
        Task<IReadOnlyList<StreamRecord>> Fetch(ulong offset, CancellationToken ct)
        {
            asked.Add(offset);
            return Task.FromResult(pages.Count > 0 ? pages.Dequeue() : (IReadOnlyList<StreamRecord>)[]);
        }

        var subscription = new Subscription(Fetch, 0, (wait, _) =>
        {
            waits.Add(wait);
            return Task.CompletedTask;
        });
        var seen = new List<ulong>();
        using var cts = new CancellationTokenSource();
        await foreach (var record in subscription.WithCancellation(cts.Token))
        {
            seen.Add(record.Offset);
            if (seen.Count == 3)
            {
                cts.Cancel();
            }
        }

        Assert.Equal([0UL, 1, 2], seen);
        Assert.Equal(3UL, subscription.Offset);
        Assert.Equal([0UL, 2, 2, 2], asked);
        Assert.Equal([TimeSpan.FromMilliseconds(100), TimeSpan.FromMilliseconds(200)], waits);

        // Resuming from Offset picks up where the last one stopped.
        var resumed = new Subscription((offset, _) =>
            Task.FromResult<IReadOnlyList<StreamRecord>>(offset == 3 ? [Record(3)] : []), subscription.Offset,
            (_, _) => Task.CompletedTask);
        await foreach (var record in resumed)
        {
            Assert.Equal(3UL, record.Offset);
            break;
        }
        Assert.Equal(4UL, resumed.Offset);
    }

    private static StreamRecord Record(ulong offset) => new(0, offset, Encoding.UTF8.GetBytes($"{{\"n\":{offset}}}"));

    /// <summary>A handler that never answers until the request is cancelled.</summary>
    private sealed class HangingHandler : HttpMessageHandler
    {
        public int Calls;
        public string? TimeoutHeader;

        protected override async Task<HttpResponseMessage> SendAsync(HttpRequestMessage request,
            CancellationToken cancellationToken)
        {
            Interlocked.Increment(ref Calls);
            if (request.Headers.TryGetValues("connect-timeout-ms", out var values))
            {
                TimeoutHeader = values.First();
            }
            await Task.Delay(Timeout.InfiniteTimeSpan, cancellationToken).ConfigureAwait(false);
            throw new InvalidOperationException("unreachable");
        }
    }
}
