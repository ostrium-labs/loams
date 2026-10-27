// A notes app on Loams, with nothing but the C# SDK: a namespace, a collection
// with a schema, five notes, reads that see their own writes, text, vector and
// hybrid search with a filter, a count, a scroll, and a graph over the notes.
//
//     LOAMS_ENDPOINT=http://127.0.0.1:8080 LOAMS_NAMESPACE=notes-demo dotnet run --project examples/Notes
//
// LOAMS_API_KEY (or LOAMS_TOKEN) is sent as the bearer when it is set.

using Loams;
using Loams.Collection.V1;
using Fusion = Loams.Fusion;

var options = LoamsClientOptions.FromEnv() with { SessionConsistency = true };
var ns = options.Namespace ?? "notes-demo";
await using var loams = new LoamsClient(options with { Namespace = ns });

var instance = await loams.Instance.GetInstanceAsync();
Console.WriteLine($"connected to {instance.Name} {instance.ServerVersion}");

// 1. A namespace (created once; a repeat is already_exists) and a collection.
try
{
    await loams.Namespaces.CreateAsync(ns);
}
catch (AlreadyExistsError)
{
}

var schema = new Schema()
    .Text("title")
    .Text("body")
    .Keyword("tags")
    .I64("n")
    .Vector("embedding", 4);
await loams.Collections.CreateAsync("notes", schema);
var notes = loams.Collection("notes");

// 2. Five notes, as one write. Its token goes into the session, so the reads
//    below see it (read-your-writes).
Doc Note(ulong id, string title, string body, string[] tags, float[] embedding) =>
    new(id, new Dictionary<string, object?> { ["title"] = title, ["body"] = body, ["tags"] = tags, ["n"] = (long)id },
        new Dictionary<string, float[]> { ["embedding"] = embedding });

var write = await notes.UpsertAsync(
[
    Note(1, "Refund policy", "Refunds are issued within 14 days.", ["policy", "billing"], [1f, 0f, 0f, 0f]),
    Note(2, "Shipping", "Orders ship in two business days.", ["shipping"], [0f, 1f, 0f, 0f]),
    Note(3, "Refund window", "The refund window closes after 30 days.", ["policy"], [0.9f, 0.1f, 0f, 0f]),
    Note(4, "Support hours", "Support answers on weekdays.", ["support"], [0f, 0f, 1f, 0f]),
    Note(5, "Returns", "Returns need a receipt and a refund form.", ["policy", "returns"], [0.7f, 0f, 0.3f, 0f]),
]);
Console.WriteLine($"wrote 5 notes, token {write.Token}");

var got = await notes.GetAsync([1, 3]);
Console.WriteLine($"read back {got.Documents.Count} notes");

await notes.PatchAsync(2, set: new { title = "Shipping times" });
await notes.DeleteAsync([4]);
Console.WriteLine($"{await notes.CountAsync(Filter.Eq("tags", "policy"))} notes tagged policy");

await foreach (var doc in notes.ScrollAllAsync(limit: 2))
{
    Console.WriteLine($"  note {doc.Id.Text}: {doc.Source.Fields["title"].StringValue}");
}

// 3. Search: text with a filter, highlights and a projection; a vector; and
//    hybrid with each fusion method.
var text = await loams.Search.QueryAsync(Search.On("notes")
    .Text("refund", "body")
    .Filter(Filter.Eq("tags", "policy"))
    .Highlight("body", pre: "[", post: "]")
    .Select(include: ["title"])
    .Sort(Sort.Score(), Sort.Pk())
    .Limit(2));
foreach (var hit in text.Hits)
{
    Console.WriteLine($"  text hit {hit.Pk.Text} score {hit.Score:F3} {hit.Source?.Fields["title"].StringValue}");
}

var near = await loams.Vector.SearchAsync("notes", [1f, 0f, 0f, 0f], field: "embedding", limit: 3);
Console.WriteLine($"vector: {string.Join(", ", near.Hits.Select((h) => h.Pk.Text))}");

foreach (var (name, fusion) in new[]
         {
             ("rrf", Fusion.Rrf(60)), ("dbsf", Fusion.Dbsf()), ("weighted", Fusion.Weighted(0.7f, 0.3f)),
         })
{
    var hybrid = await loams.Search.QueryAsync(Search.On("notes")
        .Text("refund", "body")
        .Vector("embedding", [1f, 0f, 0f, 0f])
        .Fuse(fusion)
        .Limit(3));
    Console.WriteLine($"hybrid {name}: {string.Join(", ", hybrid.Hits.Select((h) => h.Pk.Text))}");
}

// 4. A graph over the notes (experimental; skipped on a build without it).
try
{
    try
    {
        await loams.Graph.CreateAsync("notes_graph");
    }
    catch (AlreadyExistsError)
    {
    }
    await loams.Graph.QueryAsync("notes_graph",
        "INSERT (:Note {id: $id, title: $title})-[:TAGGED]->(:Tag {name: $tag})",
        new Dictionary<string, object?> { ["id"] = 1L, ["title"] = "Refund policy", ["tag"] = "policy" });
    var rows = await loams.Graph.QueryAsync("notes_graph",
        "MATCH (n:Note)-[:TAGGED]->(t:Tag) RETURN n.title, t.name", readOnly: true);
    Console.WriteLine($"graph: {rows.Rows.Rows.Count} row(s)");
    await loams.Graph.DeleteAsync("notes_graph");
}
catch (FeatureNotInVariantError)
{
    Console.WriteLine("graph: not in this build");
}

// 5. SQL with $n parameters over the same collection, and a stream of events.
var sql = await loams.Sql.QueryAsync("SELECT n, title FROM notes WHERE n > $1 ORDER BY n", [1]);
foreach (var row in sql.Records())
{
    Console.WriteLine($"  sql row {row["n"]}: {row["title"]}");
}
await loams.Streams.CreateAsync("events", partitions: 1);
var produced = await loams.Streams.ProduceAsync("events", 0,
[
    new ProducerRecord("1", new Dictionary<string, object?> { ["kind"] = "created", ["note"] = 1 }),
    new ProducerRecord("2", new Dictionary<string, object?> { ["kind"] = "patched", ["note"] = 2 }),
    new ProducerRecord("4", new Dictionary<string, object?> { ["kind"] = "deleted", ["note"] = 4 }),
], producerId: "notes-demo", producerSeq: 1);
Console.WriteLine($"events at {produced.BaseOffset}..{produced.LastOffset}{(produced.Duplicate ? " (already produced)" : "")}");
var subscription = loams.Streams.Subscribe("events", 0, fromOffset: produced.BaseOffset);
await foreach (var record in subscription)
{
    Console.WriteLine($"  event {record.Offset}: {record.Text}");
    if (record.Offset >= produced.LastOffset)
    {
        break;
    }
}

// 6. A missing collection is a typed error; then clean up.
try
{
    await loams.Collections.GetAsync("missing");
}
catch (NotFoundError error)
{
    Console.WriteLine($"missing collection: {error.Reason}");
}
await notes.DropAsync();
Console.WriteLine("done");
