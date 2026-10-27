import Foundation
#if canImport(FoundationNetworking)
import FoundationNetworking
#endif
import XCTest
import LoamsObjC

/// The SDKP app scenario through the public facade, against a running server:
/// `LOAMS_TEST_ENDPOINT=http://… swift test`. Skipped without one.
final class AppLiveTests: XCTestCase {
    var c: LoamsClient!
    var ns = ""

    let notes: [LoamsDocument] = [
        LoamsDocument(id: 1, source: ["title": "Refunds", "body": "refund policy for orders", "tags": ["policy"], "n": 1], vectors: ["embedding": [1, 0, 0, 0]]),
        LoamsDocument(id: 2, source: ["title": "Shipping", "body": "shipping times and carriers", "tags": ["ops"], "n": 2], vectors: ["embedding": [0, 1, 0, 0]]),
        LoamsDocument(id: 3, source: ["title": "Refund window", "body": "the refund window is thirty days", "tags": ["policy"], "n": 3], vectors: ["embedding": [0.9, 0.1, 0, 0]]),
        LoamsDocument(id: 4, source: ["title": "Returns", "body": "returns need a receipt", "tags": ["policy", "ops"], "n": 4], vectors: ["embedding": [0.5, 0.5, 0, 0]]),
        LoamsDocument(id: 5, source: ["title": "Holidays", "body": "holiday hours", "tags": ["ops"], "n": 5], vectors: ["embedding": [0, 0, 1, 0]]),
    ]

    func schema() -> LoamsSchema {
        return LoamsSchema().keyword("title").text("body").keyword("tags").i64("n").vector("embedding", dim: 4)
    }

    override func setUpWithError() throws {
        guard let endpoint = ProcessInfo.processInfo.environment["LOAMS_TEST_ENDPOINT"], !endpoint.isEmpty else {
            throw XCTSkip("LOAMS_TEST_ENDPOINT is not set")
        }
        ns = "parity-objc-\(Int(Date().timeIntervalSince1970 * 1000))-\(Int.random(in: 0..<1000))"
        let key = ProcessInfo.processInfo.environment["LOAMS_TEST_API_KEY"]
        let config = LoamsClientConfiguration(endpoint: endpoint, tokenSource: key.map { LoamsStaticTokenSource(token: $0) })
        config.namespace = ns
        config.sessionConsistency = true
        c = LoamsClient(configuration: config)
    }

    func test_objc_live_app_collections() throws {
        XCTAssertFalse(try c.instance.getInstance().isEmpty)
        _ = try c.namespaces.create(ns)
        XCTAssertThrowsError(try c.namespaces.create(ns)) { XCTAssertEqual(($0 as! LoamsError).codeString, "already_exists") }
        XCTAssertTrue(try c.namespaces.listAll().contains { $0["namespace"] as? String == ns })

        XCTAssertEqual(try c.collections.create("notes", schema: schema())["name"] as? String, "notes")
        XCTAssertEqual(try c.collections.get("notes")["name"] as? String, "notes")
        _ = try c.collections.create("scratch", schema: LoamsSchema().keyword("k"))
        XCTAssertEqual(try c.collections.listAll(pageSize: 1).compactMap { $0["name"] as? String }.sorted(), ["notes", "scratch"])

        let keyed = LoamsCallOptions()
        keyed.idempotencyKey = LoamsIdempotency.mintKey()
        let first = try c.documents.upsert("notes", documents: notes, options: keyed)
        let again = try c.documents.upsert("notes", documents: notes, options: keyed)
        XCTAssertEqual(first["token"] as? String, again["token"] as? String, "the same key answers the same write (R3)")

        let got = try c.documents.get("notes", ids: [1, 2], consistency: first["token"] as? String)
        XCTAssertEqual((got["documents"] as! [[String: Any]]).map { ($0["source"] as! [String: Any])["title"] as! String }, ["Refunds", "Shipping"])

        _ = try c.documents.patch("notes", id: 4, set: ["title": "Returns and exchanges"])
        _ = try c.documents.delete("notes", ids: [5])
        // The native filter JSON until P0a types `where`.
        XCTAssertEqual(try c.documents.count("notes", filter: ["term": ["field": "tags", "value": "policy"]])["count"] as? String, "3")
        XCTAssertEqual(try c.documents.scrollAll("notes", limit: 2).count, 4)

        XCTAssertThrowsError(try c.collections.get("missing")) { XCTAssertEqual(($0 as! LoamsError).codeString, "not_found") }
        XCTAssertEqual(try c.collections.drop("scratch")["dropped"] as? Bool, true)

        let handle = c.collection("notes")
        let result = try handle.writeBatch((10..<15).map { LoamsDocument(id: $0, source: ["n": $0]) }, chunkRows: 2)
        XCTAssertEqual([result.chunks, result.rows], [3, 5])
        XCTAssertEqual(try handle.count(consistency: result.token)["count"] as? String, "9")
    }

    func test_objc_live_app_search() throws {
        _ = try c.namespaces.create(ns)
        _ = try c.collections.create("notes", schema: schema())
        _ = try c.documents.upsert("notes", documents: notes)

        let page = try c.search.query(LoamsSearch.on("notes").text("refund", fields: ["body"])
            .filter(LoamsFilter.eq("tags", value: "policy")).highlight("body").select(include: ["title"])
            .limit(2).sort(["score", "pk"]))
        let hits = page["hits"] as! [[String: Any]]
        XCTAssertEqual(hits.count, 2)
        XCTAssertEqual(Array((hits[0]["source"] as! [String: Any]).keys), ["title"])

        XCTAssertEqual(try c.search.queryAll(LoamsSearch.on("notes").text("refund", fields: ["body"]).sort(["score", "pk"]).limit(1)).count, 2)
        let near = try c.vector.search("notes", vector: [1, 0, 0, 0], field: "embedding", limit: 2)
        XCTAssertEqual(((near["hits"] as! [[String: Any]])[0]["pk"] as! [String: Any])["uint"] as? String, "1")

        for fusion in [LoamsFusion.rrf(60), LoamsFusion.dbsf(), LoamsFusion.weighted([0.7, 0.3])] {
            let res = try c.search.hybrid("notes", text: "refund", vector: [1, 0, 0, 0], field: "embedding", fields: ["body"],
                                          fusion: fusion, filter: LoamsFilter.eq("tags", value: "policy"), limit: 3)
            XCTAssertFalse((res["hits"] as? [Any] ?? []).isEmpty)
        }
        XCTAssertThrowsError(try c.search.hybrid("notes", text: "refund", vector: [1, 0, 0, 0], field: "embedding",
                                                 fields: ["body"], fusion: LoamsFusion.weighted([0.7])))
        XCTAssertEqual((try c.collection("notes").search(LoamsSearch.on("x").text("refund", fields: ["body"]).limit(5))["hits"] as! [Any]).count, 2)
    }

    func test_objc_live_app_graph() throws {
        _ = try c.namespaces.create(ns)
        do {
            _ = try c.graph.create("notes_graph")
        } catch let error as LoamsError where error.reason == "feature_not_in_variant" {
            throw XCTSkip("this build serves no graph")
        }
        _ = try c.graph.query("notes_graph", statement: "INSERT (:Note {id: $id, title: $title})-[:TAGGED]->(:Tag {name: $tag})",
                              parameters: ["id": 1, "title": "Refunds", "tag": "policy"])
        let read = try c.graph.query("notes_graph", statement: "MATCH (n:Note)-[:TAGGED]->(t:Tag) RETURN n.title AS title, t.name AS tag", readOnly: true)
        let records = LoamsGraphModule.records(read)
        XCTAssertEqual(records.count, 1)
        XCTAssertEqual(records.first?["title"] as? String, "Refunds")
        XCTAssertEqual(records.first?["tag"] as? String, "policy")
        XCTAssertTrue(try c.graph.listAll().contains { $0["name"] as? String == "notes_graph" })
        _ = try c.graph.delete("notes_graph")
    }
}
