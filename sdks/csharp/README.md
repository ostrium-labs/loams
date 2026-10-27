# The Loams SDK for .NET (`Loams`)

> **0.1.2.** `Loams` 0.1.2 tracks Loams 0.1.2. In 0.x the API may change between
> minor versions.

One `LoamsClient` with namespaced modules over the unified Connect API,
generated from the Loams protos. `net8.0`; the only dependency is
`Google.Protobuf`.

```console
dotnet add package Loams --version 0.1.2
```

```csharp
using Loams;
using Loams.Instance.V1;

await using var client = new LoamsClient(new LoamsClientOptions
{
    Endpoint = Environment.GetEnvironmentVariable("LOAMS_ENDPOINT")!, // http://127.0.0.1:8080 for `loams dev`
    Auth = new EnvTokenSource(),                                      // reads LOAMS_API_KEY
});

// The first call any client makes: what this instance is, and no auth.
var info = await client.Instance.GetInstanceAsync(new GetInstanceRequest());
Console.WriteLine($"{info.Name} serves {string.Join(", ", info.ApiVersions)}");
```

## Build an app

Namespaces, collections, documents, search, vector and graph, with no REST
calls (SDKP). [`examples/Notes`](examples/Notes/Program.cs) is the whole notes
app; run it with `LOAMS_ENDPOINT=http://127.0.0.1:8080 dotnet run --project examples/Notes`.

```csharp
using Loams;
using Fusion = Loams.Fusion; // Loams.Collection.V1 has a message of the same name

// LOAMS_ENDPOINT, LOAMS_API_KEY (or LOAMS_TOKEN) and LOAMS_NAMESPACE.
await using var loams = new LoamsClient(LoamsClientOptions.FromEnv() with
{
    Namespace = "app",           // filled into every app request that names none (R11)
    SessionConsistency = true,   // reads see this process's writes (R4)
    Timeout = TimeSpan.FromSeconds(10), // per unary call, retries included (R12); 30 s by default
});

await loams.Namespaces.CreateAsync("app");
await loams.Collections.CreateAsync("notes", new Schema()
    .Text("title").Text("body").Keyword("tags").I64("n").Vector("embedding", 4));

var notes = loams.Collection("notes");
await notes.UpsertAsync(
[
    new Doc(1, new { title = "Refund policy", body = "Refunds within 14 days.", tags = new[] { "policy" }, n = 1 },
        new Dictionary<string, float[]> { ["embedding"] = [1f, 0f, 0f, 0f] }),
]);

// Hybrid search: text plus a vector, fused with RRF, filtered, highlighted.
var hits = await loams.Search.QueryAsync(Search.On("notes")
    .Text("refund", "body")
    .Vector("embedding", [1f, 0f, 0f, 0f])
    .Fuse(Fusion.Rrf(60))            // or Fusion.Dbsf(), Fusion.Weighted(0.7f, 0.3f)
    .Filter(Filter.Eq("tags", "policy") & Filter.Range("n", gte: 1))
    .Highlight("body")
    .Select(include: ["title"])
    .Limit(5));

var count = await notes.CountAsync(Filter.Eq("tags", "policy"));
await foreach (var doc in notes.ScrollAllAsync(limit: 100)) { /* every document, in id order */ }

// Any number of documents, in chunks, one idempotency key per chunk (R13).
var batch = await notes.WriteBatchAsync(manyDocs, chunkRows: 10_000);

// Graph (experimental): GQL with parameters.
await loams.Graph.CreateAsync("notes_graph");
await loams.Graph.QueryAsync("notes_graph", "INSERT (:Note {id: $id})",
    new Dictionary<string, object?> { ["id"] = 1L });
```

| Module | Calls |
|---|---|
| `Namespaces` | `CreateAsync`, `ListAsync`, `ListAllAsync` |
| `Collections` | `CreateAsync`, `ListAsync`, `ListAllAsync`, `GetAsync`, `DropAsync`, `AddFieldsAsync`, `ListVersionsAsync`, `UpdateAliasesAsync`, `ScanAsync`, `SetHotAsync`, `WarmAsync` |
| `Documents` | `WriteAsync`, `UpsertAsync`, `DeleteAsync`, `PatchAsync`, `WriteBatchAsync`, `GetAsync`, `ScrollAsync`, `ScrollAllAsync`, `CountAsync`, `DeleteByFilterAsync`, `PatchByFilterAsync` |
| `Search` | `QueryAsync`, `QueryAllAsync` (needs `Sort.Pk()` as a tiebreaker), `TextAsync`, `HybridAsync` |
| `Vector` | `SearchAsync` |
| `Graph` (experimental) | `CreateAsync`, `GetAsync`, `ListAsync`, `UpdateAsync`, `DeleteAsync`, `SchemaAsync`, `QueryAsync`, `QueryBatchAsync`, `QueryStreamAsync`, `ExplainAsync` |

Every failure is a typed `LoamsError` (`NotFoundError`, `InvalidArgumentError`,
`AlreadyExistsError`, `DeadlineExceededError`, …) with the server's `Reason`.
A write's `Token` is a `ConsistencyToken` (`Parse`, `Merge`, `ToString`) that a
read takes as `Consistent.AtLeast(token)`. SQL and streams arrive with
`loams.sql.v1` and `loams.stream.v1` on the main port; `Subscription` is the
resumable fetch loop `Streams.SubscribeAsync` will use. Durable has no module:
use the [Resonate SDK](https://docs.resonatehq.io) with `LOAMS_ENDPOINT/durable/` as its URL.

The wire is Connect over `System.Net.Http` (D650); why there is no
`Grpc.Net.Client` dependency is in [DEPENDENCIES.md](https://github.com/ostrium-labs/loams/blob/dev/sdks/csharp/DEPENDENCIES.md).

Apache-2.0. Source and issues: <https://github.com/ostrium-labs/loams/tree/dev/sdks/csharp>.
