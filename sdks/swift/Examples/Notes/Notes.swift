// A notes app on Loams with nothing but the Swift SDK: a namespace, a collection
// with a schema, five notes, reads that see their own writes, text, vector and
// hybrid search with a filter, a count, a scroll, and a graph over the notes.
//
//     LOAMS_ENDPOINT=http://127.0.0.1:8080 LOAMS_NAMESPACE=notes-demo swift run Notes
//
// LOAMS_API_KEY (or LOAMS_TOKEN) is sent as the bearer when it is set.

import Foundation
import Loams

@main
struct Notes {
    static func main() async throws {
        var options = try Options.fromEnvironment()
        options.sessionConsistency = true
        let ns = options.namespace ?? "notes-demo"
        options.namespace = ns
        let loams = try Loams(options)

        let instance = try await loams.instance.getInstance()
        print("connected to \(instance.name) \(instance.serverVersion)")

        // 1. A namespace (a repeat is already_exists) and a collection.
        do { _ = try await loams.namespaces.create(ns) } catch let error as LoamsError where error.code == .alreadyExists {}
        let schema = Schema().text("title").text("body").keyword("tags").i64("n").vector("embedding", dim: 4)
        _ = try await loams.collections.create("notes", schema: schema)
        let notes = loams.collection("notes")

        // 2. Five notes, as one write; the session gives the reads below read-your-writes.
        func note(_ id: UInt64, _ title: String, _ body: String, _ tags: [String], _ embedding: [Double]) -> Doc {
            Doc(.uint(id), ["title": .string(title), "body": .string(body), "tags": tags.json, "n": .int(Int(id))],
                vectors: ["embedding": embedding])
        }
        let write = try await notes.upsert([
            note(1, "Refund policy", "Refunds are issued within 14 days.", ["policy", "billing"], [1, 0, 0, 0]),
            note(2, "Shipping", "Orders ship in two business days.", ["shipping"], [0, 1, 0, 0]),
            note(3, "Refund window", "The refund window closes after 30 days.", ["policy"], [0.9, 0.1, 0, 0]),
            note(4, "Support hours", "Support answers on weekdays.", ["support"], [0, 0, 1, 0]),
            note(5, "Returns", "Returns need a receipt and a refund form.", ["policy", "returns"], [0.7, 0, 0.3, 0]),
        ])
        print("wrote 5 notes, token \(write.fields["token"]?.stringValue ?? "")")
        let got = try await notes.get([1, 3])
        if case .array(let docs)? = got.fields["documents"] { print("read back \(docs.count) notes") }

        _ = try await notes.patch(2, set: ["title": .string("Shipping times")])
        _ = try await notes.delete([4])
        print("\(try await notes.count(where: Filter.eq("tags", "policy"))) notes tagged policy")
        for try await doc in notes.scrollAll(limit: 2) {
            if case .object(let fields) = doc, case .object(let source)? = fields["source"] {
                print("  note \(DocumentID(json: fields["id"])?.text ?? "?"): \(source["title"]?.stringValue ?? "")")
            }
        }

        // 3. Search: text with a filter, highlights and a projection; a vector; hybrid.
        let text = try await loams.search.query(
            Search.on("notes").text("refund", fields: ["body"]).filter(Filter.eq("tags", "policy"))
                .highlight("body", pre: "[", post: "]").select(include: ["title"]).sort(.score(), .pk()).limit(2)
        )
        print("text: \(ids(text))")
        let near = try await loams.vector.search("notes", vector: [1, 0, 0, 0], field: "embedding", limit: 3)
        print("vector: \(ids(near))")
        for (name, fusion) in [("rrf", Fusion.rrf(k: 60)), ("dbsf", .dbsf), ("weighted", .weighted([0.7, 0.3]))] {
            let hybrid = try await loams.search.query(
                Search.on("notes").text("refund", fields: ["body"]).vector("embedding", [1, 0, 0, 0]).fuse(fusion).limit(3)
            )
            print("hybrid \(name): \(ids(hybrid))")
        }

        // 4. A graph over the notes (experimental; skipped on a build without it).
        do {
            do { _ = try await loams.graph.create("notes_graph") } catch let error as LoamsError where error.code == .alreadyExists {}
            _ = try await loams.graph.query(
                "notes_graph", "INSERT (:Note {id: $id, title: $title})-[:TAGGED]->(:Tag {name: $tag})",
                parameters: ["id": GraphValue.int(1), "title": GraphValue.string("Refund policy"), "tag": GraphValue.string("policy")]
            )
            let rows = try await loams.graph.query("notes_graph", "MATCH (n:Note)-[:TAGGED]->(t:Tag) RETURN n.title, t.name", readOnly: true)
            if case .object(let set)? = rows.fields["rows"], case .array(let list)? = set["rows"] { print("graph: \(list.count) row(s)") }
            _ = try await loams.graph.delete("notes_graph")
        } catch where LoamsError.code(of: error) == .unimplemented {
            print("graph: not in this build")
        }

        // 5. SQL with $n parameters over the same collection, and a stream of events.
        let sql = try await loams.sql.query("SELECT n, title FROM notes WHERE n > $1 ORDER BY n", params: [.int(1)])
        for row in SqlModule.records(sql) {
            print("  sql row \(show(row["n"])): \(show(row["title"]))")
        }
        _ = try await loams.streams.create("events")
        let produced = try await loams.streams.produce("events", records: [
            ProducerRecord(key: "1", value: JSONValue.object(["kind": .string("created"), "note": .int(1)])),
            ProducerRecord(key: "2", value: JSONValue.object(["kind": .string("patched"), "note": .int(2)])),
            ProducerRecord(key: "4", value: JSONValue.object(["kind": .string("deleted"), "note": .int(4)])),
        ], producerId: "notes-demo", producerSeq: 1)
        let base = UInt64(produced.fields["baseOffset"]?.stringValue ?? "0") ?? 0
        let last = UInt64(produced.fields["lastOffset"]?.stringValue ?? "0") ?? 0
        print("events at \(base)..\(last)\(produced.fields["duplicate"] == .bool(true) ? " (already produced)" : "")")
        for try await record in loams.streams.subscribe("events", from: base) {
            print("  event \(record.offset): \(record.text)")
            if record.offset >= last { break }
        }

        // 6. A missing collection is a typed error; then clean up.
        do {
            _ = try await loams.collections.get("missing")
        } catch let error as LoamsError where error.code == .notFound {
            print("missing collection: \(error.code)")
        }
        _ = try await notes.drop()
        print("done")
    }

    static func show(_ value: JSONValue?) -> String {
        switch value {
        case .string(let s)?: return s
        case .int(let n)?: return String(n)
        case .number(let d)?: return d == d.rounded() ? String(Int(d)) : String(d)
        case .bool(let b)?: return String(b)
        default: return ""
        }
    }

    static func ids(_ response: DynamicMessage) -> String {
        guard case .array(let hits)? = response.fields["hits"] else { return "" }
        return hits.compactMap { hit -> String? in
            guard case .object(let fields) = hit else { return nil }
            return DocumentID(json: fields["pk"])?.text
        }.joined(separator: ", ")
    }
}
