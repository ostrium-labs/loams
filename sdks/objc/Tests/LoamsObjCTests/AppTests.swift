import Foundation
#if canImport(FoundationNetworking)
import FoundationNetworking
#endif
import XCTest
import LoamsObjC

/// A scripted server: every request is recorded, every answer queued.
final class Script: LoamsHTTPSender, @unchecked Sendable {
    var requests: [URLRequest] = []
    var answers: [(Data, Int)?] = []

    func ok(_ json: [String: Any] = [:]) {
        answers.append((try! JSONSerialization.data(withJSONObject: json), 200))
    }

    func fail(_ status: Int, _ code: String, _ message: String) {
        answers.append((try! JSONSerialization.data(withJSONObject: ["code": code, "message": message]), status))
    }

    /// A queued nil answers as a timeout.
    func timeout() { answers.append(nil) }

    func send(_ request: URLRequest) throws -> LoamsHTTPResult {
        requests.append(request)
        guard !answers.isEmpty else { throw LoamsTransport.clientDeadline() }
        guard let answer = answers.removeFirst() else { throw LoamsTransport.clientDeadline() }
        return LoamsHTTPResult(data: answer.0, status: answer.1)
    }

    func sent(_ i: Int) -> [String: Any] {
        return (try? JSONSerialization.jsonObject(with: requests[i].httpBody ?? Data())) as? [String: Any] ?? [:]
    }

    func header(_ i: Int, _ name: String) -> String? { return requests[i].value(forHTTPHeaderField: name) }

    func method(_ i: Int) -> String { return requests[i].url!.lastPathComponent }
}

func client(_ s: Script, namespace: String? = nil, session: Bool = false, timeout: TimeInterval = 30) -> LoamsClient {
    let config = LoamsClientConfiguration(endpoint: "http://127.0.0.1:4400")
    config.namespace = namespace
    config.sessionConsistency = session
    config.timeout = timeout
    config.sender = s
    return LoamsClient(configuration: config)
}

/// Proto3 JSON compared as values: numbers by value, key order ignored.
func normalized(_ value: Any) -> NSObject {
    let data = try! JSONSerialization.data(withJSONObject: ["v": value], options: [.sortedKeys])
    return try! JSONSerialization.jsonObject(with: data) as! NSObject
}

func fixturesDir() -> URL {
    if let env = ProcessInfo.processInfo.environment["LOAMS_FIXTURES_DIR"], !env.isEmpty {
        return URL(fileURLWithPath: env)
    }
    return URL(fileURLWithPath: FileManager.default.currentDirectoryPath).deletingLastPathComponent().appendingPathComponent("fixtures")
}

final class AppTests: XCTestCase {
    func test_objc_consistency_token_merge() throws {
        XCTAssertEqual(try LoamsConsistencyToken.parse("v1:s7/p3@918274").stringValue, "v1:s7/p3@918274")
        XCTAssertEqual(try LoamsConsistencyToken.parse("v1:s2/p0@1,s1/p5@3").stringValue, "v1:s2/p0@1,s1/p5@3", "parse keeps the order")
        let merged = try LoamsConsistencyToken.parse("v1:s7/p3@10,s1/p0@5").merge(try LoamsConsistencyToken.parse("v1:s7/p3@4,s7/p1@2,s1/p0@9"))
        XCTAssertEqual(merged.stringValue, "v1:s1/p0@9,s7/p1@2,s7/p3@10")
        XCTAssertEqual(merged.offset(stream: 7, partition: 3), NSNumber(value: UInt64(10)))
        XCTAssertNil(merged.offset(stream: 1, partition: 1))

        let corpus = fixturesDir().appendingPathComponent("tokens.json")
        guard let data = try? Data(contentsOf: corpus) else {
            print("no token corpus at \(corpus.path); the inline cases ran")
            return
        }
        let json = try JSONSerialization.jsonObject(with: data) as! [String: Any]
        for c in json["parse"] as! [[String: Any]] {
            let token = try LoamsConsistencyToken.parse(c["text"] as! String)
            XCTAssertEqual(token.stringValue, c["text"] as? String, c["name"] as! String)
            let want = (c["items"] as! [[Any]]).map { $0.map { "\($0)" } }
            XCTAssertEqual(token.itemStrings, want, c["name"] as! String)
        }
        for c in json["merge"] as! [[String: Any]] {
            let a = try LoamsConsistencyToken.parse(c["a"] as! String)
            let b = try LoamsConsistencyToken.parse(c["b"] as! String)
            XCTAssertEqual(a.merge(b).stringValue, c["expect"] as? String, c["name"] as! String)
        }
        for bad in json["malformed"] as! [String] {
            XCTAssertThrowsError(try LoamsConsistencyToken.parse(bad), bad) { error in
                XCTAssertEqual((error as! LoamsError).codeString, "invalid_argument")
            }
        }
    }

    func test_objc_session_read_your_writes() throws {
        let s = Script()
        let c = client(s, namespace: "app", session: true)
        s.ok(["token": "v1:s1/p0@5"])
        s.ok(["readToken": "v1:s1/p0@5,s1/p1@3"])
        s.ok()
        s.ok()
        _ = try c.documents.upsert("notes", documents: [LoamsDocument(id: 1, source: ["title": "a"])])
        _ = try c.documents.get("notes", ids: [1])
        XCTAssertEqual((s.sent(1)["consistency"] as? [String: Any])?["atLeast"] as? String, "v1:s1/p0@5")
        _ = try c.documents.get("notes", ids: [1])
        XCTAssertEqual((s.sent(2)["consistency"] as? [String: Any])?["atLeast"] as? String, "v1:s1/p0@5,s1/p1@3")
        _ = try c.documents.get("notes", ids: [1], consistency: "eventual")
        XCTAssertEqual((s.sent(3)["consistency"] as? [String: Any])?["freshness"] as? String, "FRESHNESS_EVENTUAL")

        // SDKP-17: a graph read never takes the session's token.
        s.ok()
        _ = try c.graph.query("g", statement: "MATCH (n) RETURN n", readOnly: true)
        XCTAssertNil(s.sent(4)["consistency"])

        let p = Script()
        let plain = client(p, namespace: "app")
        p.ok(["token": "v1:s1/p0@5"])
        p.ok()
        _ = try plain.documents.upsert("notes", documents: [LoamsDocument(id: 1)])
        _ = try plain.documents.get("notes", ids: [1])
        XCTAssertNil(p.sent(1)["consistency"], "off by default")
    }

    func test_objc_default_namespace() throws {
        let s = Script()
        let c = client(s, namespace: "app")
        s.ok(["name": "notes"])
        s.ok(["name": "notes"])
        _ = try c.collections.get("notes")
        XCTAssertEqual(s.sent(0)["namespace"] as? String, "app")
        _ = try c.collections.get("notes", options: LoamsCallOptions(namespace: "other"))
        XCTAssertEqual(s.sent(1)["namespace"] as? String, "other")

        let b = Script()
        XCTAssertThrowsError(try client(b).collections.get("notes")) { error in
            XCTAssertEqual((error as! LoamsError).codeString, "invalid_argument")
            XCTAssertEqual((error as! LoamsError).reason, "namespace_required")
        }
        XCTAssertTrue(b.requests.isEmpty, "refused before any network call")
    }

    func test_objc_timeout_deadline() throws {
        let s = Script()
        let c = client(s, namespace: "app", timeout: 5)
        s.ok()
        s.ok()
        _ = try c.collections.get("notes")
        let ms = Int(s.header(0, "connect-timeout-ms") ?? "") ?? -1
        XCTAssertTrue(ms > 0 && ms <= 5000, "the client default goes out as connect-timeout-ms")
        XCTAssertLessThanOrEqual(s.requests[0].timeoutInterval, 5)
        let quick = LoamsCallOptions()
        quick.timeout = 0.25
        _ = try c.collections.get("notes", options: quick)
        XCTAssertLessThanOrEqual(Int(s.header(1, "connect-timeout-ms") ?? "") ?? 9999, 250)

        let slow = Script()
        slow.timeout()
        slow.ok()
        XCTAssertThrowsError(try client(slow, namespace: "app").collections.get("notes")) { error in
            let e = error as! LoamsError
            XCTAssertEqual(e.codeString, "deadline_exceeded")
            XCTAssertEqual(e.reason, "client_deadline_exceeded")
        }
        XCTAssertEqual(slow.requests.count, 1, "a spent budget is never retried")

        let none = Script()
        none.ok()
        _ = try client(none, namespace: "app", timeout: 0).collections.get("notes")
        XCTAssertNil(none.header(0, "connect-timeout-ms"))
    }

    func test_objc_write_batch_chunks() throws {
        let docs = (1...5).map { LoamsDocument(id: $0, source: ["n": $0]) }
        let s = Script()
        s.ok(["token": "v1:s1/p0@2"])
        s.ok(["token": "v1:s1/p1@2"])
        s.ok(["token": "v1:s1/p0@5"])
        let result = try client(s, namespace: "app").documents.writeBatch("notes", documents: docs, chunkRows: 2)
        XCTAssertEqual(result.token, "v1:s1/p0@5,s1/p1@2")
        XCTAssertEqual([result.chunks, result.rows], [3, 5])
        XCTAssertEqual((0..<3).map { (s.sent($0)["ops"] as! [Any]).count }, [2, 2, 1])
        let keys = (0..<3).map { s.header($0, "idempotency-key") ?? "" }
        XCTAssertEqual(Set(keys).count, 3, "every chunk has its own key")
        XCTAssertEqual(keys, (0..<3).map { s.sent($0)["idempotencyKey"] as? String ?? "" })

        let f = Script()
        f.ok(["token": "v1:s1/p0@2"])
        f.fail(503, "unavailable", "down")
        f.ok(["token": "v1:s1/p0@4"])
        f.fail(400, "invalid_argument", "bad doc")
        XCTAssertThrowsError(try client(f, namespace: "app").documents.writeBatch("notes", documents: docs, chunkRows: 2)) { error in
            let e = error as! LoamsBatchError
            XCTAssertEqual([e.chunksApplied, e.rowsApplied], [2, 4])
            XCTAssertEqual(e.token, "v1:s1/p0@4")
            XCTAssertEqual(e.reason, "batch_partially_applied")
            XCTAssertEqual(e.codeString, "aborted")
            XCTAssertEqual(e.cause?.codeString, "invalid_argument")
            XCTAssertEqual(e.metadata["chunks_applied"], "2")
        }
        XCTAssertEqual(f.header(1, "idempotency-key"), f.header(2, "idempotency-key"), "a retried chunk reuses its key")

        let b = Script()
        for _ in 0..<5 { b.ok() }
        _ = try client(b, namespace: "app").documents.writeBatch("notes", documents: docs, chunkBytes: 60)
        XCTAssertGreaterThan(b.requests.count, 1, "the byte budget closes a chunk")
    }

    func test_objc_streams_subscribe_resumes() {
        let log: [[String: Any]] = (0..<5).map { ["offset": $0, "value": "r\($0)"] }
        nonisolated(unsafe) var fetched: [UInt64] = []
        nonisolated(unsafe) var sleeps: [Int] = []
        let sub = LoamsSubscription(fromOffset: 1, fetch: { offset in
            fetched.append(offset)
            if fetched.count == 2 || fetched.count == 3 { return [] }
            return Array(log.filter { UInt64($0["offset"] as! Int) >= offset }.prefix(2))
        }, sleep: { sleeps.append(Int(($0 * 1000).rounded())) })
        var got: [String] = []
        while let record = sub.next() {
            got.append(record["value"] as! String)
            if got.count == 4 { sub.cancel() }
        }
        XCTAssertEqual(got, ["r1", "r2", "r3", "r4"])
        XCTAssertEqual(sub.offset, 5, "the next offset is the last + 1")
        XCTAssertEqual(fetched, [1, 3, 3, 3])
        XCTAssertEqual(sleeps, [100, 200], "an empty fetch backs off, doubling")

        nonisolated(unsafe) var capped: [Int] = []
        nonisolated(unsafe) var empty: LoamsSubscription?
        empty = LoamsSubscription(fromOffset: 0, fetch: { _ in [] }, sleep: {
            capped.append(Int(($0 * 1000).rounded()))
            if capped.count == 6 { empty?.cancel() }
        })
        XCTAssertNil(empty!.next())
        XCTAssertEqual(capped, [100, 200, 400, 800, 1000, 1000], "the backoff stops at 1 s")
    }

    func test_objc_builders_match_corpus() throws {
        XCTAssertEqual(normalized(LoamsSchema().text("title").keyword("tags").i64("n").vector("embedding", dim: 4).toJSON()), normalized([
            "fields": [
                ["name": "title", "source_path": "title", "kind": ["text": ["analyzer": "standard", "positions": true]], "indexed": true, "fast": false],
                ["name": "tags", "source_path": "tags", "kind": "keyword", "indexed": true, "fast": true],
                ["name": "n", "source_path": "n", "kind": "i64", "indexed": true, "fast": true],
            ],
            "vectors": [["name": "embedding", "dim": 4, "distance": "cosine"]],
            "sparse_vectors": [Any](),
            "dynamic": "ignore",
            "max_fields": 1000,
        ]), "schema_notes")
        XCTAssertEqual(normalized(LoamsFilter.ids([1, "k-str", ["uuid": "0190f5c4-6c1e-7b3a-9d2e-4f5a6b7c8d9e"]]).json),
                       normalized(["ids": [["uint": "1"], "k-str", ["uuid": "0190f5c4-6c1e-7b3a-9d2e-4f5a6b7c8d9e"]]]), "filter_ids")
        XCTAssertEqual(normalized(LoamsFusion.rrf()), normalized(["rrf": [String: Any]()]))
        XCTAssertThrowsError(try LoamsSearch.on("n").consistency("not-a-token"))
        XCTAssertThrowsError(try LoamsSearch.on("n").text("x").vector("v", values: [1]).fuse(LoamsFusion.weighted([0.7])).build())
        let wire = try LoamsSearch.wire(LoamsSearch.on("n").text("x").vector("v", values: [1], k: 3).limit(25).json)
        XCTAssertEqual((wire["retrievers"] as! [[String: [String: Any]]]).map { $0.values.first!["k"] as? Int }, [nil, 3],
                       "k goes out only when given")

        let dir = fixturesDir().appendingPathComponent("builders")
        guard let files = try? FileManager.default.contentsOfDirectory(at: dir, includingPropertiesForKeys: nil) else {
            print("no builder corpus in \(dir.path); the inline cases ran")
            return
        }
        var ran = 0
        for file in files where file.pathExtension == "json" {
            let c = try JSONSerialization.jsonObject(with: Data(contentsOf: file)) as! [String: Any]
            let got = try BuilderCorpus.build(c)
            XCTAssertEqual(normalized(got), normalized(c["expect"]!), c["name"] as! String)
            ran += 1
        }
        XCTAssertGreaterThan(ran, 0)
    }

    func test_objc_collection_handle_delegates() throws {
        let s = Script()
        let c = client(s, namespace: "app")
        let notes = c.collection("notes", namespace: "other")
        s.ok()
        s.ok()
        s.ok(["count": "3"])
        s.ok()
        s.ok()
        s.ok()
        s.ok(["dropped": true])
        _ = try notes.upsert([LoamsDocument(id: "a", source: ["x": 1])])
        _ = try notes.get(["a"])
        XCTAssertEqual(try notes.count()["count"] as? String, "3")
        _ = try notes.search(LoamsSearch.on("ignored").text("x", fields: ["body"]))
        _ = try notes.vectorSearch([1, 0], field: "embedding")
        _ = try notes.info()
        XCTAssertEqual(try notes.drop()["dropped"] as? Bool, true)
        XCTAssertEqual((0..<7).map { s.method($0) },
                       ["WriteDocuments", "GetDocuments", "CountDocuments", "Search", "Search", "GetCollection", "DropCollection"])
        for i in 0..<7 {
            XCTAssertEqual(s.sent(i)["namespace"] as? String, "other")
            XCTAssertEqual(s.sent(i)["collection"] as? String, "notes")
        }
    }

    func test_objc_search_query_all_requires_tiebreak() throws {
        let s = Script()
        let c = client(s, namespace: "app")
        XCTAssertThrowsError(try c.search.queryAll(LoamsSearch.on("notes").text("x").sort(["score"])))
        XCTAssertTrue(s.requests.isEmpty)
        s.ok(["hits": [["pk": ["uint": "1"], "sortValues": [0.9, 1]], ["pk": ["uint": "2"], "sortValues": [0.8, 2]]]])
        s.ok(["hits": [["pk": ["uint": "3"], "sortValues": [0.7, 3]]]])
        let hits = try c.search.queryAll(LoamsSearch.on("notes").text("x").sort(["score", "pk"]).limit(2))
        XCTAssertEqual(hits.count, 3)
        XCTAssertEqual((s.sent(1)["searchAfter"] as! [Any])[1] as? Int, 2, "the last hit's sortValues, as answered")
    }

    func test_objc_sql_and_streams_send_the_recorded_bodies() throws {
        let s = Script()
        let c = client(s, namespace: "app")
        for _ in 0..<7 { s.ok(["columns": [["name": "tags"], ["name": "c"]], "rows": [["policy", 2]]]) }
        let answer = try c.sql.query("SELECT tags, count(*) AS c FROM notes WHERE n > $1 GROUP BY tags ORDER BY tags", params: [1])
        XCTAssertEqual(normalized(s.sent(0)), normalized(["namespace": "app", "sql": "SELECT tags, count(*) AS c FROM notes WHERE n > $1 GROUP BY tags ORDER BY tags", "params": [1]]))
        XCTAssertEqual(normalized(LoamsSqlModule.records(answer)), normalized([["tags": "policy", "c": 2]]))
        _ = try c.sql.query("SELECT title, n FROM notes ORDER BY n", maxRows: 2)
        XCTAssertEqual(normalized(s.sent(1)), normalized(["namespace": "app", "sql": "SELECT title, n FROM notes ORDER BY n", "maxRows": 2]))
        _ = try c.streams.create("note-events")
        XCTAssertEqual(normalized(s.sent(2)), normalized(["namespace": "app", "name": "note-events", "partitions": 1]))
        _ = try c.streams.produce("note-events", records: [["key": "1", "value": ["note": 1, "op": "created"]],
                                                           ["key": "5", "value": ["note": 5, "op": "deleted"]]],
                                  producerId: "app-producer-1", producerSeq: 1)
        XCTAssertEqual(normalized(s.sent(3)), normalized(["namespace": "app", "stream": "note-events", "records": [
            ["key": "MQ==", "value": "eyJub3RlIjoxLCJvcCI6ImNyZWF0ZWQifQ=="],
            ["key": "NQ==", "value": "eyJub3RlIjo1LCJvcCI6ImRlbGV0ZWQifQ=="],
        ], "producerId": "app-producer-1", "producerSeq": "1"]))
        _ = try c.streams.fetch("note-events", maxRecords: 10)
        XCTAssertEqual(normalized(s.sent(4)), normalized(["namespace": "app", "stream": "note-events", "maxRecords": 10]),
                       "a zero partition and offset are omitted")
        _ = try c.streams.fetch("note-events", offset: 2, maxRecords: 10)
        XCTAssertEqual(normalized(s.sent(5)), normalized(["namespace": "app", "stream": "note-events", "offset": "2", "maxRecords": 10]))
        _ = try c.streams.get("note-events")
        XCTAssertEqual(normalized(s.sent(6)), normalized(["namespace": "app", "stream": "note-events"]))
    }

    func test_objc_streams_module_subscribe() throws {
        let s = Script()
        let c = client(s, namespace: "app")
        s.ok(["records": [["offset": "1", "value": Data("a".utf8).base64EncodedString()],
                          ["offset": "2", "value": Data("b".utf8).base64EncodedString()]]])
        s.ok(["records": [["offset": "3", "value": Data("c".utf8).base64EncodedString()]]])
        let sub = c.streams.subscribe("events", fromOffset: 1)
        var got: [String] = []
        while let record = sub.next() {
            got.append(String(data: record["value"] as! Data, encoding: .utf8)!)
            if got.count == 3 { sub.cancel() }
        }
        XCTAssertEqual(got, ["a", "b", "c"])
        XCTAssertEqual(sub.offset, 4)
        XCTAssertEqual(s.sent(0)["offset"] as? String, "1")
        XCTAssertEqual(s.sent(1)["offset"] as? String, "3", "the next fetch starts after the last record")
    }

    func test_objc_facade_matches_index() throws {
        for (name, call) in loamsAppCalls {
            XCTAssertTrue(["loams.collection.v1.", "loams.graph.v1.", "loams.sql.v1.", "loams.stream.v1."].contains { call.rpc.hasPrefix($0) }, name)
        }
        let path = fixturesDir().appendingPathComponent("facade-index.json")
        guard let data = try? Data(contentsOf: path) else {
            print("no facade index at \(path.path)")
            return
        }
        let modules = Set(loamsAppCalls.keys.map { String($0.split(separator: ".")[0]) })
        let index = try JSONSerialization.jsonObject(with: data) as! [String: Any]
        for c in index["calls"] as! [[String: Any]] {
            guard let module = c["module"] as? String, modules.contains(module), c["stream"] as? Bool != true else { continue }
            let call = c["call"] as! String
            let snake = call.reduce(into: "") { out, ch in
                if ch.isUppercase && !out.isEmpty { out += "_" }
                out += ch.lowercased()
            }
            let key = "\(module).\(snake)"
            XCTAssertEqual(loamsAppCalls[key]?.rpc, c["rpc"] as? String, key)
            if let retry = c["retry"] as? String { XCTAssertEqual(loamsAppCalls[key]?.retry, retry, key) }
        }
    }
}

/// Drives the builders from the builder corpus's neutral notation
/// (`sdks/fixtures/builders/README.md`).
enum BuilderCorpus {
    static func build(_ c: [String: Any]) throws -> [String: Any] {
        switch c["builder"] as! String {
        case "schema":
            let schema = LoamsSchema()
            for call in c["spec"] as! [[String: Any]] { try applySchema(schema, call) }
            return schema.toJSON()
        case "filter":
            return try filter(c["spec"] as! [String: Any]).json
        default:
            let spec = c["spec"] as! [String: Any]
            var search = LoamsSearch.on(spec["on"] as! String)
            for call in spec["calls"] as! [[String: Any]] { search = try applySearch(search, call) }
            return try search.build()
        }
    }

    static func split(_ call: [String: Any], options: Bool = true) -> ([Any], [String: Any]) {
        var args = (call["args"] as? [Any]) ?? []
        var named: [String: Any] = [:]
        if options, let last = args.last as? [String: Any], last["op"] == nil {
            named = last
            args.removeLast()
        }
        return (args, named)
    }

    static func applySchema(_ s: LoamsSchema, _ call: [String: Any]) throws {
        let (args, _) = split(call)
        let name = args[0] as! String
        switch call["op"] as! String {
        case "text": s.text(name)
        case "keyword": s.keyword(name)
        case "i64": s.i64(name)
        case "f64": s.f64(name)
        case "bool": s.boolean(name)
        case "timestamp": s.timestamp(name)
        case "uuid": s.uuid(name)
        case "vector": s.vector(name, dim: args[1] as! Int, distance: args.count > 2 ? args[2] as! String : "cosine")
        case "sparse_vector": s.sparseVector(name)
        case "dynamic": s.dynamicMapping(name)
        default: throw LoamsValues.invalidArgument("unknown schema op")
        }
    }

    static func filter(_ call: [String: Any]) throws -> LoamsFilter {
        let op = call["op"] as! String
        let (args, o) = split(call, options: op != "ids")
        let filters = { try args.map { try filter($0 as! [String: Any]) } }
        switch op {
        case "all": return LoamsFilter.all()
        case "none": return LoamsFilter.none()
        case "eq": return LoamsFilter.eq(args[0] as! String, value: args[1])
        case "in": return LoamsFilter.isIn(args[0] as! String, values: args[1] as! [Any])
        case "range": return LoamsFilter.range(args[0] as! String, gt: o["gt"], gte: o["gte"], lt: o["lt"], lte: o["lte"])
        case "exists": return LoamsFilter.exists(args[0] as! String)
        case "is_null": return LoamsFilter.isNull(args[0] as! String)
        case "is_empty": return LoamsFilter.isEmpty(args[0] as! String)
        case "prefix": return LoamsFilter.prefix(args[0] as! String, value: args[1] as! String)
        case "wildcard": return LoamsFilter.wildcard(args[0] as! String, pattern: args[1] as! String)
        case "match": return LoamsFilter.match(args[0] as! String, text: args[1] as! String)
        case "phrase": return LoamsFilter.phrase(args[0] as! String, text: args[1] as! String)
        case "multi_match": return LoamsFilter.multiMatch(args[0] as! [String], text: args[1] as! String)
        case "query_string": return LoamsFilter.queryString(args[0] as! String)
        case "ids": return LoamsFilter.ids(args)
        case "and": return LoamsFilter.and(try filters())
        case "or": return LoamsFilter.or(try filters())
        case "not": return LoamsFilter.not(try filters())
        case "filter": return LoamsFilter.filter(try filters())
        default: throw LoamsValues.invalidArgument("unknown filter op \(op)")
        }
    }

    static func nested(_ call: [String: Any]) throws -> [String: Any] {
        let (args, _) = split(call)
        switch call["op"] as! String {
        case "rrf": return LoamsFusion.rrf(args.first.map { NSNumber(value: $0 as! Int) })
        case "dbsf": return LoamsFusion.dbsf()
        case "weighted": return LoamsFusion.weighted(args.map { ($0 as! NSNumber).doubleValue })
        case "score": return LoamsSort.score(args.first as? String)
        case "pk": return LoamsSort.pk(args.first as? String)
        case "field": return LoamsSort.field(args[0] as! String)
        default: return try filter(call).json
        }
    }

    static func number(_ value: Any?) -> NSNumber? {
        switch value {
        case let i as Int: return NSNumber(value: i)
        case let d as Double: return NSNumber(value: d)
        case let n as NSNumber: return n
        default: return nil
        }
    }

    static func applySearch(_ s: LoamsSearch, _ call: [String: Any]) throws -> LoamsSearch {
        let (args, o) = split(call)
        switch call["op"] as! String {
        case "text":
            if let q = args[0] as? [String: Any] { return s.textQuery(try filter(q), k: number(o["k"])) }
            return s.text(args[0] as! String, fields: o["fields"] as? [String], k: number(o["k"]))
        case "vector":
            return s.vector(args[0] as! String, values: (args[1] as! [Any]).map { number($0)!.doubleValue },
                            k: number(o["k"]), exact: number(o["exact"]), nprobes: number(o["nprobes"]),
                            refineFactor: number(o["refine_factor"]), ef: number(o["ef"]), distance: o["distance"] as? String)
        case "fuse": return s.fuse(try nested(args[0] as! [String: Any]))
        case "filter": return s.filter(try filter(args[0] as! [String: Any]))
        case "sort": return s.sort(try args.map { try nested($0 as! [String: Any]) })
        case "limit": return s.limit(args[0] as! Int)
        case "offset": return s.offset(args[0] as! Int)
        case "after": return s.after(args[0] as! [Any])
        case "min_score": return s.minScore(number(args[0])!.doubleValue)
        case "select": return s.select(include: o["include"] as? [String], exclude: o["exclude"] as? [String],
                                       noSource: o["no_source"] as? Bool ?? false)
        case "highlight": return s.highlight(args[0] as! String)
        case "group_by": return s.groupBy(args[0] as! String)
        case "expand": return s.expand(args[0] as! String, fromField: o["from_field"] as? String, hops: number(o["hops"]),
                                       direction: o["direction"] as? String)
        case "consistency": return try s.consistency(args[0] as! String)
        default: throw LoamsValues.invalidArgument("unknown search op")
        }
    }
}
