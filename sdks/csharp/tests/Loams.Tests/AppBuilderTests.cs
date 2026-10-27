// SDKP Part A and B (docs/plans/2026-10-19-sdk-parity.md §5, L5): the Schema,
// Filter and Search builders against the builder cases of P0d, the collection
// handle, query_all's tiebreak rule, and the app modules' bindings.
//
// Each case is the request's proto3 JSON. When `sdks/fixtures/builders/*.json`
// lands (P0d) `csharp_builders_match_corpus` also checks every corpus case's
// `expect` decodes to the same message as the case here of the same name.

using System.Net;
using System.Text.Json;
using Google.Protobuf;
using Google.Protobuf.WellKnownTypes;
using Loams.Collection.V1;
using Loams.Tests.Support;
using Xunit;
using static Loams.Tests.AppRuntimeTests;
using Fusion = Loams.Fusion;

namespace Loams.Tests;

public sealed class AppBuilderTests
{
    private static readonly float[] Embedding = [0.1f, 0.2f, 0.3f, 0.4f];
    private static readonly float[] Unit = [1f, 0f, 0f, 0f];

    /// <summary>The builder cases, by the corpus's names.</summary>
    internal static IReadOnlyDictionary<string, (IMessage Built, string Expect)> Cases() => new Dictionary<string, (IMessage, string)>
    {
        ["schema_notes"] = (new Schema().Text("title").Text("body").Keyword("tags").I64("n").Vector("embedding", 4).Build(),
            """
            {"fields":[
              {"name":"title","source_path":"title","kind":{"text":{"analyzer":"standard","positions":true}},"indexed":true,"fast":false},
              {"name":"body","source_path":"body","kind":{"text":{"analyzer":"standard","positions":true}},"indexed":true,"fast":false},
              {"name":"tags","source_path":"tags","kind":"keyword","indexed":true,"fast":true},
              {"name":"n","source_path":"n","kind":"i64","indexed":true,"fast":true}],
             "vectors":[{"name":"embedding","dim":4,"distance":"cosine"}],
             "sparse_vectors":[],"dynamic":"ignore","max_fields":1000}
            """),
        ["filter_term"] = (Filter.Eq("tags", "policy"), """{"term":{"field":"tags","value":"policy"}}"""),
        ["filter_bool_nested"] = (Filter.And(Filter.Eq("tags", "policy"),
                Filter.Or(Filter.Range("n", gte: 2), ~Filter.Exists("archived"))),
            """
            {"bool":{"must":[{"term":{"field":"tags","value":"policy"}},
              {"bool":{"should":[{"range":{"field":"n","gte":2}},{"bool":{"mustNot":[{"exists":{"field":"archived"}}]}}]}}]}}
            """),
        ["filter_range_open"] = (Filter.Range("n", gt: 1), """{"range":{"field":"n","gt":1}}"""),
        ["filter_ids"] = (Filter.Ids(1, "k-str", DocumentId.OfUuid("0190f5c4-6c1e-7b3a-9d2e-4f5a6b7c8d9e")),
            """{"ids":[{"uint":"1"},"k-str",{"uuid":"0190f5c4-6c1e-7b3a-9d2e-4f5a6b7c8d9e"}]}"""),
        ["search_text_filter_highlight_select"] = (Search.On("notes").Text("refund", "body")
                .Filter(Filter.Eq("tags", "policy")).Highlight("body").Select(include: ["title"])
                .Sort(Sort.Score(), Sort.Pk()).Limit(2).Build(),
            """
            {"collection":"notes","retrievers":[{"text":{"query":{"match":{"field":"body","text":"refund"}}}}],
             "filter":{"term":{"field":"tags","value":"policy"}},
             "sort":[{"score":{}},{"pk":{}}],
             "limit":2,"select":{"source":{"include":["title"]}},
             "highlight":{"fields":[{"field":"body"}]}}
            """),
        ["search_vector_ann_params"] = (Search.On("notes")
                .Vector("embedding", Unit, k: 3, ann: new AnnParams { Nprobes = 8, RefineFactor = 2 }).Limit(3).Build(),
            """
            {"collection":"notes","retrievers":[{"vector":{"field":"embedding","query":[1,0,0,0],"k":3,"params":{"nprobes":8,"refineFactor":2}}}],"limit":3}
            """),
        ["search_hybrid_rrf"] = (Hybrid().Fuse(Fusion.Rrf(60)).Build(), HybridJson("""{"rrf":{"k":60}}""")),
        ["search_hybrid_dbsf"] = (Hybrid().Fuse(Fusion.Dbsf()).Build(), HybridJson("""{"dbsf":{}}""")),
        ["search_hybrid_weighted"] = (Hybrid().Fuse(Fusion.Weighted(0.7f, 0.3f)).Build(),
            HybridJson("""{"weightedSum":{"weights":[0.7,0.3]}}""")),
        ["search_after_page"] = (Search.On("notes").Text("refund", "body")
                .Filter(Filter.Eq("tags", "policy")).Highlight("body").Select(include: ["title"]).Limit(2)
                .Sort(Sort.Score(), Sort.Pk()).After([Value.ForNumber(0.3438858389854431), Value.ForNumber(3)]).Build(),
            """
            {"collection":"notes","retrievers":[{"text":{"query":{"match":{"field":"body","text":"refund"}}}}],
             "filter":{"term":{"field":"tags","value":"policy"}},"highlight":{"fields":[{"field":"body"}]},
             "select":{"source":{"include":["title"]}},"limit":2,"sort":[{"score":{}},{"pk":{}}],
             "searchAfter":[0.3438858389854431,3]}
            """),
        ["search_expand"] = (Search.On("notes").Text("refund", "body").Expand("notes_graph", "id", hops: 1).Build(),
            """
            {"collection":"notes","retrievers":[{"text":{"query":{"match":{"field":"body","text":"refund"}}}}],
             "expandSpec":{"graph":"notes_graph","fromField":"id","hops":1}}
            """),
    };

    private static Search Hybrid() => Search.On("notes").Text("refund", "body").Vector("embedding", Unit, k: 5).Limit(3);

    private static string HybridJson(string fusion) =>
        """{"collection":"notes","retrievers":[{"text":{"query":{"match":{"field":"body","text":"refund"}}}},"""
        + """{"vector":{"field":"embedding","query":[1,0,0,0],"k":5}}],"fusion":""" + fusion + ""","limit":3}""";

    private static IMessage Parse(IMessage like, string json) =>
        JsonParser.Default.Parse(json, like.Descriptor);

    [Fact]
    public void csharp_builders_match_corpus()
    {
        foreach (var (name, (built, expect)) in Cases())
        {
            Assert.True(Parse(built, expect).Equals(built), $"{name}: {JsonFormatter.Default.Format(built)}");
        }

        var dir = Path.Combine(ConformanceReport.RepositoryRoot(), "sdks", "fixtures", "builders");
        if (!Directory.Exists(dir))
        {
            return;
        }
        var cases = Cases();
        foreach (var file in Directory.GetFiles(dir, "*.json"))
        {
            using var doc = JsonDocument.Parse(File.ReadAllText(file));
            var root = doc.RootElement;
            var name = root.TryGetProperty("name", out var n) ? n.GetString()! : Path.GetFileNameWithoutExtension(file);
            if (!cases.TryGetValue(name, out var mine))
            {
                continue;
            }
            Assert.True(Parse(mine.Built, root.GetProperty("expect").GetRawText()).Equals(mine.Built), $"corpus case {name}");
        }
    }

    [Fact]
    public void csharp_filter_ir_matches_the_engine_mapping()
    {
        // `connect_query_filters.rs`: unit arms are bare strings, field names snake_case.
        var ir = Filter.ToIr(Filter.And(Filter.All(), Filter.Not(Filter.IsNull("x")), Filter.Ids(1, "a")));
        Assert.Equal(
            """{ "bool": { "must": [ "match_all", { "bool": { "must_not": [ { "is_null": { "field": "x" } } ] } }, { "ids": [ 1, "a" ] } ] } }""",
            JsonFormatter.Default.Format(ir));
    }

    [Fact]
    public async Task csharp_collection_handle_delegates()
    {
        var handler = new StubHandler(StubHandler.ConnectJson(HttpStatusCode.OK, "{}"));
        using var client = ClientOver(handler, ns: "app");
        var notes = client.Collection("notes", "other");

        await notes.UpsertAsync([new Doc(1, new Dictionary<string, object?> { ["title"] = "a" })]);
        await notes.GetAsync([1]);
        await notes.DeleteAsync([1]);
        await notes.PatchAsync(1, set: new { title = "b" }, unset: ["body"]);
        await notes.CountAsync(Filter.Eq("tags", "x"));
        await notes.SearchAsync(Search.On("ignored").Text("refund", "body"));
        await notes.VectorSearchAsync(Embedding, "embedding");
        await notes.InfoAsync();
        await notes.DropAsync();

        string P(string service, string method) => $"/loams.collection.v1.{service}/{method}";
        Assert.Equal(
        [
            P("DocumentService", "WriteDocuments"), P("DocumentService", "GetDocuments"),
            P("DocumentService", "WriteDocuments"), P("DocumentService", "WriteDocuments"),
            P("DocumentService", "CountDocuments"), P("QueryService", "Search"), P("QueryService", "Search"),
            P("CollectionService", "GetCollection"), P("CollectionService", "DropCollection"),
        ], handler.Requests.Select((r) => r.Path));

        foreach (var request in handler.Requests)
        {
            using var body = JsonDocument.Parse(request.Body);
            Assert.Equal("other", body.RootElement.GetProperty("namespace").GetString());
            Assert.Equal("notes", body.RootElement.GetProperty("collection").GetString());
        }
        var patch = Body(handler.Requests[3], WriteDocumentsRequest.Parser).Ops[0].Patch;
        Assert.Equal("b", patch.Source.Fields["title"].StringValue);
        Assert.Equal(["body"], patch.DeleteKeys);
        var count = Body(handler.Requests[4], CountDocumentsRequest.Parser);
        Assert.Equal("x", count.Filter.StructValue.Fields["term"].StructValue.Fields["value"].StringValue);

        // The handle holds no state: the default namespace still applies without one.
        await client.Collection("notes").InfoAsync();
        Assert.Equal("app", Body(handler.Requests[^1], GetCollectionRequest.Parser).Namespace);
    }

    [Fact]
    public async Task csharp_search_query_all_requires_tiebreak()
    {
        var handler = new StubHandler(StubHandler.ConnectJson(HttpStatusCode.OK, "{}"));
        using var client = ClientOver(handler);

        var error = await Assert.ThrowsAsync<InvalidArgumentError>(async () =>
        {
            await foreach (var _ in client.Search.QueryAllAsync(Search.On("notes").Text("refund", "body").Sort(Sort.Score())))
            {
            }
        });
        Assert.Equal("sort", error.Metadata["field"]);
        Assert.Equal(0, handler.CallCount);

        // With a tiebreak it follows search_after until a short page.
        var paged = new StubHandler(
            StubHandler.ConnectJson(HttpStatusCode.OK, """{"hits":[{"pk":{"uint":"1"},"sortValues":[2,1]},{"pk":{"uint":"2"},"sortValues":[1,2]}]}"""),
            StubHandler.ConnectJson(HttpStatusCode.OK, """{"hits":[{"pk":{"uint":"3"},"sortValues":[0.5,3]}]}"""));
        using var other = ClientOver(paged);
        var ids = new List<ulong>();
        await foreach (var hit in other.Search.QueryAllAsync(Search.On("notes").Text("refund", "body")
                           .Sort(Sort.Score(), Sort.Pk()).Limit(2)))
        {
            ids.Add(hit.Pk.Uint);
        }
        Assert.Equal([1UL, 2, 3], ids);
        Assert.Equal(2, paged.CallCount);
        var second = Body(paged.Requests[1], SearchRequest.Parser);
        Assert.Equal([1.0, 2.0], second.SearchAfter.Select((v) => v.NumberValue));
    }

    [Fact]
    public void csharp_app_bindings()
    {
        // Retry classes per SDKP §2.2–§2.5.
        var safe = new (string, string)[]
        {
            ("namespaces", "Create"), ("namespaces", "List"), ("collections", "Create"), ("collections", "List"),
            ("collections", "Get"), ("collections", "Drop"), ("collections", "AddFields"),
            ("collections", "ListVersions"), ("collections", "UpdateAliases"), ("collections", "SetHot"),
            ("collections", "Warm"), ("documents", "Get"), ("documents", "Scroll"), ("documents", "Count"),
            ("search", "Query"), ("vector", "Search"), ("graph", "Get"), ("graph", "List"), ("graph", "Schema"),
            ("graph", "Explain"),
        };
        var manual = new (string, string)[]
        {
            ("collections", "Scan"), ("documents", "Write"), ("documents", "DeleteByFilter"),
            ("documents", "PatchByFilter"), ("graph", "Create"), ("graph", "Update"), ("graph", "Delete"),
            ("graph", "Query"), ("graph", "QueryBatch"), ("graph", "QueryStream"),
        };
        foreach (var (module, name) in safe)
        {
            Assert.Equal(RetryClass.Safe, Facade.Binding(module, name).Retry);
        }
        foreach (var (module, name) in manual)
        {
            Assert.Equal(RetryClass.Manual, Facade.Binding(module, name).Retry);
        }
        Assert.True(Facade.Binding("documents", "Write").TakesIdempotencyKey);
        Assert.Equal(Facade.Binding("search", "Query").Rpc, Facade.Binding("vector", "Search").Rpc);
        Assert.Equal("loams.collection.v1.QueryService/Search", Facade.Binding("vector", "Search").Rpc);
        Assert.Equal("Namespaces", Facade.Binding("namespaces", "List").Pagination!.ItemsField);
        Assert.Equal(Streaming.Server, Facade.Binding("graph", "QueryStream").Streaming);
        Assert.True(Facade.Modules.Single((m) => m.Name == "graph").Unstable);
        using var client = new LoamsClient(new LoamsClientOptions { Endpoint = "https://fixture.invalid" });
        foreach (var module in new[] { "namespaces", "collections", "documents", "search", "vector", "graph" })
        {
            Assert.True(client.Modules.ContainsKey(module), module);
        }
    }

    [Fact]
    public async Task csharp_graph_read_only_query_is_unkeyed_and_retried()
    {
        var handler = new StubHandler(
            StubHandler.ConnectRefusal(HttpStatusCode.ServiceUnavailable, "unavailable", "restarting", "unavailable"),
            StubHandler.ConnectJson(HttpStatusCode.OK, """{"rows":{"columns":["n"]}}"""));
        using var client = ClientOver(handler);

        var answer = await client.Graph.QueryAsync("g", "MATCH (n) RETURN n", readOnly: true);
        Assert.Equal(["n"], answer.Rows.Columns);
        Assert.Equal(2, handler.CallCount);
        var body = Body(handler.Requests[0], Loams.Graph.V1.ExecuteRequest.Parser);
        Assert.Equal(string.Empty, body.IdempotencyKey);
        Assert.Equal(Loams.Graph.V1.QueryLanguage.Gql, body.Language);
        Assert.Equal("app", body.Namespace);

        var write = new StubHandler(StubHandler.ConnectJson(HttpStatusCode.OK, "{}"));
        using var writer = ClientOver(write);
        await writer.Graph.QueryAsync("g", "INSERT (:Note {id: $id})",
            new Dictionary<string, object?> { ["id"] = 1L });
        var keyed = Body(write.Requests[0], Loams.Graph.V1.ExecuteRequest.Parser);
        Assert.NotEqual(string.Empty, keyed.IdempotencyKey);
        Assert.Equal(1L, keyed.Parameters["id"].Int64);
    }
}
