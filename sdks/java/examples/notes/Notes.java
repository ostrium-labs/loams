// A notes app on Loams with nothing but the Java SDK: a namespace, a collection
// with a schema, five notes, reads that see their own writes, text, vector and
// hybrid search with a filter, a count, a scroll, and a graph over the notes.
//
//     ./build.sh example     # with LOAMS_ENDPOINT and LOAMS_NAMESPACE set
//
// LOAMS_API_KEY (or LOAMS_TOKEN) is sent as the bearer when it is set.

import dev.loams.AlreadyExistsException;
import dev.loams.CallOptions;
import dev.loams.Client;
import dev.loams.CollectionHandle;
import dev.loams.Doc;
import dev.loams.Filter;
import dev.loams.Fusion;
import dev.loams.NotFoundException;
import dev.loams.Options;
import dev.loams.ProducerRecord;
import dev.loams.Schema;
import dev.loams.Search;
import dev.loams.Sort;
import dev.loams.SqlModule;
import dev.loams.UnimplementedException;
import dev.loams.Values;
import dev.loams.gen.loams.collection.v1.Document;
import dev.loams.gen.loams.collection.v1.GetDocumentsResponse;
import dev.loams.gen.loams.collection.v1.Hit;
import dev.loams.gen.loams.collection.v1.SearchResponse;
import dev.loams.gen.loams.collection.v1.WriteDocumentsResponse;
import dev.loams.gen.loams.graph.v1.ExecuteResponse;
import dev.loams.gen.loams.instance.v1.GetInstanceRequest;
import java.util.List;
import java.util.Map;
import java.util.stream.Collectors;

public final class Notes {
    private Notes() {}

    private static Doc note(long id, String title, String body, List<String> tags, float[] embedding) {
        return new Doc(id, Map.of("title", title, "body", body, "tags", tags, "n", id), Map.of("embedding", embedding));
    }

    private static String ids(SearchResponse response) {
        return response.getHitsList().stream().map(Hit::getPk).map(Values::idText).collect(Collectors.joining(", "));
    }

    public static void main(String[] args) {
        Options.Builder builder = Options.fromEnv().sessionConsistency(true);
        String ns = System.getenv().getOrDefault("LOAMS_NAMESPACE", "notes-demo");
        Client loams = Client.of(builder.namespace(ns).build());
        var instance = loams.instance().getInstance(GetInstanceRequest.getDefaultInstance());
        System.out.println("connected to " + instance.getName() + " " + instance.getServerVersion());

        // 1. A namespace (a repeat is already_exists) and a collection.
        try {
            loams.namespaces().create(ns);
        } catch (AlreadyExistsException ignored) {
            // created by an earlier run
        }
        loams.collections().create("notes", new Schema().text("title").text("body").keyword("tags").i64("n").vector("embedding", 4));
        CollectionHandle notes = loams.collection("notes");

        // 2. Five notes, as one write; the session gives the reads below read-your-writes.
        WriteDocumentsResponse write = notes.upsert(List.of(
                note(1, "Refund policy", "Refunds are issued within 14 days.", List.of("policy", "billing"), new float[] {1, 0, 0, 0}),
                note(2, "Shipping", "Orders ship in two business days.", List.of("shipping"), new float[] {0, 1, 0, 0}),
                note(3, "Refund window", "The refund window closes after 30 days.", List.of("policy"), new float[] {0.9f, 0.1f, 0, 0}),
                note(4, "Support hours", "Support answers on weekdays.", List.of("support"), new float[] {0, 0, 1, 0}),
                note(5, "Returns", "Returns need a receipt and a refund form.", List.of("policy", "returns"), new float[] {0.7f, 0, 0.3f, 0})));
        System.out.println("wrote 5 notes, token " + write.getToken());
        GetDocumentsResponse got = notes.get(List.of(1, 3));
        System.out.println("read back " + got.getDocumentsCount() + " notes");

        notes.patch(2, Map.of("title", "Shipping times"), null);
        notes.delete(List.of(4));
        System.out.println(notes.count(Filter.eq("tags", "policy")) + " notes tagged policy");
        for (Document doc : notes.scrollAll(null, 2)) {
            System.out.println("  note " + Values.idText(doc.getId()) + ": " + doc.getSource().getFieldsOrThrow("title").getStringValue());
        }

        // 3. Search: text with a filter, highlights and a projection; a vector; hybrid.
        SearchResponse text = loams.search().query(Search.on("notes").text("refund", "body")
                .filter(Filter.eq("tags", "policy")).highlight("body", "[", "]", null, null)
                .select(List.of("title"), null).sort(Sort.score(), Sort.pk()).limit(2));
        System.out.println("text: " + ids(text));
        System.out.println("vector: " + ids(loams.vector().search("notes", new float[] {1, 0, 0, 0}, "embedding", 3, null)));
        for (var fusion : Map.of("rrf", Fusion.rrf(60), "dbsf", Fusion.dbsf(), "weighted", Fusion.weighted(0.7f, 0.3f)).entrySet()) {
            SearchResponse hybrid = loams.search().query(Search.on("notes").text("refund", "body")
                    .vector("embedding", new float[] {1, 0, 0, 0}).fuse(fusion.getValue()).limit(3));
            System.out.println("hybrid " + fusion.getKey() + ": " + ids(hybrid));
        }

        // 4. A graph over the notes (experimental; skipped on a build without it).
        try {
            try {
                loams.graph().create("notes_graph");
            } catch (AlreadyExistsException ignored) {
                // created by an earlier run
            }
            loams.graph().query("notes_graph", "INSERT (:Note {id: $id, title: $title})-[:TAGGED]->(:Tag {name: $tag})",
                    Map.of("id", 1L, "title", "Refund policy", "tag", "policy"), false);
            ExecuteResponse rows = loams.graph().query("notes_graph",
                    "MATCH (n:Note)-[:TAGGED]->(t:Tag) RETURN n.title, t.name", null, true);
            System.out.println("graph: " + rows.getRows().getRowsCount() + " row(s)");
            loams.graph().delete("notes_graph");
        } catch (UnimplementedException notInBuild) {
            System.out.println("graph: not in this build");
        }

        // 5. SQL with $n parameters over the same collection, and a stream of events.
        var sql = loams.sql().query("SELECT n, title FROM notes WHERE n > $1 ORDER BY n", List.of(1));
        for (Map<String, Object> row : SqlModule.records(sql)) {
            System.out.println("  sql row " + row.get("n") + ": " + row.get("title"));
        }
        loams.streams().create("events", 1);
        var produced = loams.streams().produce("events", 0, List.of(
                ProducerRecord.of("1", Map.of("kind", "created", "note", 1)),
                ProducerRecord.of("2", Map.of("kind", "patched", "note", 2)),
                ProducerRecord.of("4", Map.of("kind", "deleted", "note", 4))),
                "notes-demo", 1L);
        System.out.println("events at " + produced.getBaseOffset() + ".." + produced.getLastOffset()
                + (produced.getDuplicate() ? " (already produced)" : ""));
        try (var subscription = loams.streams().subscribe("events", 0, produced.getBaseOffset(), 1000)) {
            while (subscription.hasNext()) {
                var record = subscription.next();
                System.out.println("  event " + record.offset() + ": " + record.text());
                if (record.offset() >= produced.getLastOffset()) {
                    break;
                }
            }
        }

        // 6. A missing collection is a typed error; then clean up.
        try {
            loams.collections().get("missing", CallOptions.none());
        } catch (NotFoundException missing) {
            System.out.println("missing collection: " + missing.code());
        }
        notes.drop();
        System.out.println("done");
    }
}
