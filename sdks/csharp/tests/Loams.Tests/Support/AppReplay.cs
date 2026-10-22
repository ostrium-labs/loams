// The app scenario replay (SDKP P0d): `sdks/fixtures/recorded/app/*.json`
// through the SDK's public facade, as `sdks/fixtures/app-scenario.md` says.
//
// Each step's request is the recorded body decoded into the call's request message
// (or, when the step names a `builder` case, built with the builders), sent through
// the facade with `loams-fixture-name` and `loams-fixture-step`, and the fixture
// server compares what arrived with the recording. The answer is decoded and
// compared with the recorded one (volatile fields masked, numbers to ~1e-6
// relative); a refusal is compared by its typed error's code and reason.
//
// Calls whose facade takes arguments rather than a request (`streams.*`,
// `graph.delete`) are made with the decoded request's fields as those arguments,
// so the replay proves what the facade itself builds from them.

using System.Collections.Concurrent;
using System.Globalization;
using System.Text.Json;
using System.Text.Json.Nodes;
using Google.Protobuf;
using Google.Protobuf.Reflection;
using Google.Protobuf.WellKnownTypes;
using Loams.Collection.V1;
using Loams.Graph.V1;
using Loams.Instance.V1;
using Loams.Sql.V1;
using Loams.Stream.V1;

namespace Loams.Tests.Support;

/// <summary>Replays one app scenario and answers its failures.</summary>
internal static class AppReplay
{
    private static readonly JsonParser Parser =
        new(JsonParser.Settings.Default.WithIgnoreUnknownFields(true));

    private sealed record AppCall(
        MessageDescriptor Request,
        MessageDescriptor Answer,
        Func<LoamsClient, IMessage, CallOptions, Task<IMessage>> Invoke);

    private static AppCall Unary<TReq, TRes>(Func<LoamsClient, TReq, CallOptions, Task<TRes>> call)
        where TReq : IMessage<TReq>, new()
        where TRes : IMessage<TRes>, new() =>
        new(new TReq().Descriptor, new TRes().Descriptor,
            async (client, request, options) => await call(client, (TReq)request, options).ConfigureAwait(false));

    /// <summary>The facade calls the scenario makes, by the recording's <c>module.call</c>.</summary>
    private static readonly IReadOnlyDictionary<string, AppCall> Calls = new Dictionary<string, AppCall>(StringComparer.Ordinal)
    {
        ["instance.get_instance"] = Unary<GetInstanceRequest, GetInstanceResponse>(
            (c, r, o) => c.Instance.GetInstanceAsync(r, o)),
        ["namespaces.create"] = Unary<CreateNamespaceRequest, CreateNamespaceResponse>(
            (c, r, o) => c.Namespaces.CreateAsync(r, o)),
        ["collections.create"] = Unary<CreateCollectionRequest, CollectionInfo>(
            (c, r, o) => c.Collections.CreateAsync(r, o)),
        ["collections.get"] = Unary<GetCollectionRequest, CollectionInfo>(
            (c, r, o) => c.Collections.GetAsync(r, o)),
        ["collections.list"] = Unary<ListCollectionsRequest, ListCollectionsResponse>(
            (c, r, o) => c.Collections.ListAsync(r, o)),
        ["collections.drop"] = Unary<DropCollectionRequest, DropCollectionResponse>(
            (c, r, o) => c.Collections.DropAsync(r, o)),
        ["documents.write"] = Unary<WriteDocumentsRequest, WriteDocumentsResponse>(
            (c, r, o) => c.Documents.WriteAsync(r, o)),
        ["documents.get"] = Unary<GetDocumentsRequest, GetDocumentsResponse>(
            (c, r, o) => c.Documents.GetAsync(r, o)),
        ["documents.count"] = Unary<CountDocumentsRequest, CountDocumentsResponse>(
            (c, r, o) => c.Documents.CountAsync(r, o)),
        ["documents.scroll"] = Unary<ScrollDocumentsRequest, ScrollDocumentsResponse>(
            (c, r, o) => c.Documents.ScrollAsync(r, o)),
        ["search.query"] = Unary<SearchRequest, SearchResponse>(
            (c, r, o) => c.Search.QueryAsync(r, o)),
        ["vector.search"] = Unary<SearchRequest, SearchResponse>(
            (c, r, o) => c.Vector.SearchAsync(r, o)),
        ["graph.create"] = Unary<CreateGraphRequest, Loams.Graph.V1.Graph>(
            (c, r, o) => c.Graph.CreateAsync(r, o)),
        ["graph.query"] = Unary<ExecuteRequest, ExecuteResponse>(
            (c, r, o) => c.Graph.QueryAsync(r, o)),
        ["graph.delete"] = Unary<DeleteGraphRequest, Loams.Operations.V1.Operation>(
            (c, r, o) => c.Graph.DeleteAsync(r.Name, r.Namespace,
                r.IdempotencyKey.Length > 0 ? o with { IdempotencyKey = r.IdempotencyKey } : o)),
        ["sql.query"] = Unary<QueryRequest, QueryResponse>(
            (c, r, o) => c.Sql.QueryAsync(r, o)),
        ["sql.query_arrow"] = Unary<QueryArrowRequest, QueryArrowResponse>(
            (c, r, o) => c.Sql.QueryArrowAsync(r, o)),
        ["streams.create"] = Unary<CreateStreamRequest, CreateStreamResponse>(
            (c, r, o) => c.Streams.CreateAsync(r.Name, r.Partitions, r.Namespace, o)),
        ["streams.get"] = Unary<DescribeStreamRequest, DescribeStreamResponse>(
            (c, r, o) => c.Streams.GetAsync(r.Stream, r.Namespace, o)),
        ["streams.list"] = Unary<ListStreamsRequest, ListStreamsResponse>(
            (c, r, o) => c.Streams.ListAsync(r, o)),
        ["streams.produce"] = Unary<ProduceRequest, ProduceResponse>(
            (c, r, o) => c.Streams.ProduceAsync(r.Stream, r.Partition,
                r.Records.Select((record) => (object)new ProducerRecord(
                    record.HasKey ? record.Key.ToByteArray() : null,
                    record.Value.ToByteArray(),
                    record.Headers.Select((h) => new KeyValuePair<string, object>(h.Key, h.Value.ToByteArray())).ToList()))
                    .ToList(),
                r.HasProducerId ? r.ProducerId : null,
                r.HasProducerSeq ? r.ProducerSeq : null,
                r.Namespace, o)),
        ["streams.fetch"] = Unary<FetchRequest, FetchResponse>(
            (c, r, o) => c.Streams.FetchAsync(r.Stream, r.Partition, r.Offset,
                r.HasMaxRecords ? r.MaxRecords : null,
                r.HasMaxBytes ? r.MaxBytes : null,
                r.HasMaxWaitMs ? r.MaxWaitMs : null,
                r.Namespace, o)),
        ["streams.produce_events"] = Unary<ProduceCloudEventsRequest, ProduceCloudEventsResponse>(
            (c, r, o) => c.Streams.ProduceEventsAsync(r, o)),
        ["streams.fetch_events"] = Unary<FetchCloudEventsRequest, FetchCloudEventsResponse>(
            (c, r, o) => c.Streams.FetchEventsAsync(r, o)),
    };

    /// <summary>Whether the recording exists.</summary>
    public static bool Available(string fixture) => File.Exists(FixturePath(fixture));

    private static string FixturePath(string fixture) =>
        System.IO.Path.Combine(ConformanceReport.RepositoryRoot(), "sdks", "fixtures", "recorded", "app", fixture + ".json");

    /// <summary>Replays every step of <paramref name="fixture"/> against <paramref name="endpoint"/>.</summary>
    public static async Task<IReadOnlyList<string>> RunAsync(string endpoint, string fixture)
    {
        using var document = JsonDocument.Parse(await File.ReadAllTextAsync(FixturePath(fixture)).ConfigureAwait(false));
        var root = document.RootElement;
        var volatileFields = root.TryGetProperty("volatile", out var v)
            ? v.EnumerateArray().Select((e) => e.GetString()!).ToHashSet(StringComparer.Ordinal)
            : [];

        using var client = new LoamsClient(new LoamsClientOptions
        {
            Endpoint = endpoint,
            Codec = Codec.Json,
            Namespace = "app",
            NoRetries = true,
        });

        var failures = new List<string>();
        var index = 0;
        foreach (var step in root.GetProperty("steps").EnumerateArray())
        {
            var at = index++;
            var name = step.GetProperty("call").GetString()!;
            var label = $"{fixture} step {at} ({name})";
            if (!Calls.TryGetValue(name, out var call))
            {
                failures.Add($"{label}: no replay for the call");
                continue;
            }

            IMessage request;
            try
            {
                request = Request(call, step);
            }
            catch (Exception e) when (e is InvalidOperationException or InvalidProtocolBufferException or InvalidJsonException)
            {
                failures.Add($"{label}: {e.Message}");
                continue;
            }

            var options = new CallOptions
            {
                Headers = new Dictionary<string, string>(StringComparer.Ordinal)
                {
                    ["loams-fixture-name"] = fixture,
                    ["loams-fixture-step"] = at.ToString(CultureInfo.InvariantCulture),
                },
            };
            var expect = step.GetProperty("expect");
            var status = expect.GetProperty("status").GetInt32();
            var response = step.GetProperty("response");

            try
            {
                var answer = await call.Invoke(client, request, options).ConfigureAwait(false);
                if (status != 200)
                {
                    failures.Add($"{label}: answered, but the recording is a {status}");
                    continue;
                }
                var want = Parser.Parse(response.GetProperty("body").GetRawText(), call.Answer);
                var problem = Compare(
                    Mask(JsonNode.Parse(JsonFormatter.Default.Format(want)), volatileFields),
                    Mask(JsonNode.Parse(JsonFormatter.Default.Format(answer)), volatileFields), "$");
                if (problem is not null)
                {
                    failures.Add($"{label}: {problem}");
                }
            }
            catch (LoamsError error)
            {
                if (status == 200)
                {
                    failures.Add($"{label}: refused ({error.Code}, {error.Message}) where the recording answered");
                    continue;
                }
                var recordedCode = response.TryGetProperty("body", out var body) &&
                                   body.ValueKind == JsonValueKind.Object &&
                                   body.TryGetProperty("code", out var c)
                    ? c.GetString()
                    : null;
                if (recordedCode is not null && error.Code != WireReader.CodeFromWire(recordedCode))
                {
                    failures.Add($"{label}: refused with {error.Code}, the recording with {recordedCode}");
                }
                var reason = error.UnknownReason ?? ReasonRegistry.Name(error.Reason);
                var wantReason = expect.TryGetProperty("reason", out var r) ? r.GetString() : null;
                if (wantReason is not null && reason != wantReason)
                {
                    failures.Add($"{label}: reason {reason}, the recording {wantReason}");
                }
            }
        }
        return failures;
    }

    /// <summary>The step's request: the recorded body, with its builder case's part built by the builders.</summary>
    private static IMessage Request(AppCall call, JsonElement step)
    {
        var body = step.GetProperty("request").GetProperty("body").GetString();
        var request = Parser.Parse(string.IsNullOrEmpty(body) ? "{}" : body, call.Request);
        if (!step.TryGetProperty("builder", out var builder) || builder.ValueKind != JsonValueKind.String)
        {
            return request;
        }
        var name = builder.GetString()!;
        if (!AppBuilderTests.Cases().TryGetValue(name, out var built))
        {
            throw new InvalidOperationException($"no builder case {name}");
        }
        switch (request, built.Built)
        {
            case (CreateCollectionRequest create, Struct schema):
                create.Schema = schema.Clone();
                return create;
            case (CountDocumentsRequest count, Query where):
                count.Where = Struct.Parser.ParseJson(JsonFormatter.Default.Format(where));
                return count;
            case (SearchRequest, SearchRequest search):
                // The builder writes no namespace: the client's default fills it (R11).
                return search.Clone();
            default:
                throw new InvalidOperationException(
                    $"the builder case {name} ({built.Built.Descriptor.Name}) does not build a {request.Descriptor.Name}");
        }
    }

    private static JsonNode? Mask(JsonNode? node, IReadOnlySet<string> volatileFields)
    {
        switch (node)
        {
            case JsonObject obj:
                foreach (var key in obj.Select((p) => p.Key).ToList())
                {
                    if (volatileFields.Contains(key))
                    {
                        obj.Remove(key);
                    }
                    else
                    {
                        Mask(obj[key], volatileFields);
                    }
                }
                break;
            case JsonArray array:
                foreach (var item in array)
                {
                    Mask(item, volatileFields);
                }
                break;
        }
        return node;
    }

    /// <summary>Null when equal; numbers compare to ~1e-6 relative.</summary>
    private static string? Compare(JsonNode? want, JsonNode? got, string at)
    {
        switch (want)
        {
            case JsonObject w:
                if (got is not JsonObject g)
                {
                    return $"{at}: {Show(got)} is not an object";
                }
                foreach (var key in w.Select((p) => p.Key).Union(g.Select((p) => p.Key)))
                {
                    var problem = Compare(w[key], g[key], $"{at}.{key}");
                    if (problem is not null)
                    {
                        return problem;
                    }
                }
                return null;
            case JsonArray w:
                if (got is not JsonArray ga || ga.Count != w.Count)
                {
                    return $"{at}: {Show(got)} is not {w.Count} items";
                }
                for (var i = 0; i < w.Count; i++)
                {
                    var problem = Compare(w[i], ga[i], $"{at}[{i}]");
                    if (problem is not null)
                    {
                        return problem;
                    }
                }
                return null;
            case JsonValue wv when wv.GetValueKind() == JsonValueKind.Number:
                if (got is JsonValue gv && gv.GetValueKind() == JsonValueKind.Number)
                {
                    var a = wv.GetValue<double>();
                    var b = gv.GetValue<double>();
                    return Math.Abs(a - b) <= 1e-6 * Math.Max(1.0, Math.Max(Math.Abs(a), Math.Abs(b)))
                        ? null
                        : $"{at}: {b} is not {a}";
                }
                return $"{at}: {Show(got)} is not {Show(want)}";
            default:
                return JsonNode.DeepEquals(want, got) ? null : $"{at}: {Show(got)} is not {Show(want)}";
        }
    }

    private static string Show(JsonNode? node) => node?.ToJsonString() ?? "absent";
}

/// <summary>
/// The app fixtures this process replayed and checked, which
/// <see cref="ConformanceReport"/> adds to the report's <c>ran</c>.
/// </summary>
internal static class AppReplays
{
    private static readonly ConcurrentDictionary<string, bool> Replayed = new(StringComparer.Ordinal);

    /// <summary>The replayed app fixtures, sorted.</summary>
    public static IReadOnlyList<string> Ran => Replayed.Keys.Order(StringComparer.Ordinal).ToArray();

    /// <summary>Records a fixture as replayed and checked, and adds it to the report on disk.</summary>
    public static void Record(string fixture)
    {
        Replayed[fixture] = true;
        ConformanceReport.AddRan(ConformanceReport.RepositoryRoot(), fixture);
    }
}
