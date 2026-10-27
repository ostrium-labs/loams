// SDKP (docs/plans/2026-10-19-sdk-parity.md §5, L7): the runtime additions R4,
// R11–R14, the Schema, Filter and Search builders, the collection handle and
// the app modules, against the stub transport.

import Foundation
import XCTest
@testable import Loams

private let malformed = [
    "v2:s1/p0@1", "v1:s1/p0", "v1:s01/p0@1", "v1:s1/p0@1,", "", "v1", "s1/p0@1", "v1:,s1/p0@1",
    "v1:s1/p00@1", "v1:s1/p0@01", "v1:s1/p4294967296@1", "v1:s18446744073709551616/p0@1", "v1:s+1/p0@1",
    "v1:s1/p0@-1", "v1:s1p0@1", "v1:S1/P0@1", "v1: s1/p0@1", "v1:s1/p0@1@2", "v1:s/p0@1",
]

private func client(_ transport: StubTransport, namespace: String? = "app", session: Bool = false,
                    timeout: Double = loamsDefaultTimeout) throws -> Loams {
    try Loams(
        Options(endpoint: URL(string: "http://stub.invalid")!, maxRetries: 3, sessionConsistency: session,
                namespace: namespace, timeout: timeout),
        transport: transport
    )
}

/// A transport that never answers until it is cancelled.
private final class HangingTransport: HTTPTransport, @unchecked Sendable {
    let config = TransportConfig(endpoint: URL(string: "http://stub.invalid")!)
    private let lock = NSLock()
    private var seen: [WireRequest] = []
    var requests: [WireRequest] { lock.withLock { seen } }

    func unary(_ request: WireRequest, bearer: String?) async throws -> WireResponse {
        lock.withLock { seen.append(request) }
        try await Task.sleep(nanoseconds: 60_000_000_000)
        return .success(body: [:], trailers: [:])
    }

    func serverStream(_ request: WireRequest, bearer: String?) async throws -> WireStream {
        AsyncThrowingStream { $0.finish() }
    }
}

final class AppRuntimeTests: XCTestCase {
    func test_swift_consistency_token_merge() throws {
        let one = try ConsistencyToken.parse("v1:s7/p3@918274")
        XCTAssertEqual(one.positions, [.init(stream: 7, partition: 3, offset: 918_274)])
        XCTAssertEqual(one.description, "v1:s7/p3@918274")
        let max = "v1:s18446744073709551615/p4294967295@18446744073709551615"
        XCTAssertEqual(try ConsistencyToken.parse(max).description, max)
        XCTAssertEqual(try ConsistencyToken.parse("v1:"), .empty)

        let merged = try ConsistencyToken.parse("v1:s7/p3@10,s1/p0@5")
            .merge(try ConsistencyToken.parse("v1:s7/p3@4,s7/p1@2,s1/p0@9"))
        XCTAssertEqual(merged.description, "v1:s1/p0@9,s7/p1@2,s7/p3@10")
        XCTAssertEqual(merged.offset(stream: 7, partition: 3), 10)
        XCTAssertNil(merged.offset(stream: 1, partition: 1))
        XCTAssertEqual(
            ConsistencyToken([
                .init(stream: 2, partition: 0, offset: 1), .init(stream: 1, partition: 5, offset: 3),
                .init(stream: 2, partition: 0, offset: 8), .init(stream: 1, partition: 5, offset: 2),
            ]).description,
            "v1:s1/p5@3,s2/p0@8"
        )
        for bad in malformed {
            XCTAssertNil(ConsistencyToken.tryParse(bad), bad)
            XCTAssertThrowsError(try ConsistencyToken.parse(bad)) { error in
                XCTAssertEqual(LoamsError.code(of: error), .invalidArgument)
            }
        }
    }

    func test_swift_session_read_your_writes() async throws {
        let stub = StubTransport(replies: [
            .success(["token": .string("v1:s4/p0@7")]),
            .success(["token": .string("v1:s4/p1@2,s4/p0@3")]),
            .success(["documents": .array([]), "readToken": .string("v1:s4/p0@9")]),
            .success([:]),
        ])
        let loams = try client(stub, session: true)
        _ = try await loams.documents.upsert("notes", [Doc(1, ["title": .string("a")])])
        _ = try await loams.documents.delete("notes", ids: [2])
        let afterWrites = await loams.session?.current()
        XCTAssertEqual(afterWrites, "v1:s4/p0@7,s4/p1@2")

        _ = try await loams.documents.get("notes", ids: [1])
        XCTAssertEqual(stub.calls[2].body["consistency"], .object(["atLeast": .string("v1:s4/p0@7,s4/p1@2")]))
        let afterRead = await loams.session?.current()
        XCTAssertEqual(afterRead, "v1:s4/p0@9,s4/p1@2")

        _ = try await loams.documents.get("notes", ids: [1], consistency: .strong)
        XCTAssertEqual(stub.calls[3].body["consistency"], .object(["freshness": .string("FRESHNESS_STRONG")]))

        let plain = StubTransport(replies: [.success([:])])
        let off = try client(plain)
        _ = try await off.documents.get("notes", ids: [1])
        XCTAssertNil(plain.calls[0].body["consistency"])
    }

    func test_swift_default_namespace() async throws {
        let stub = StubTransport(replies: [.success(["name": .string("notes")])])
        let loams = try client(stub)
        _ = try await loams.collections.get("notes")
        _ = try await loams.collections.get("notes", namespace: "other")
        _ = try await loams.collections.get("notes", options: CallOptions(namespace: "per-call"))
        XCTAssertEqual(stub.calls.map { $0.body["namespace"]?.stringValue }, ["app", "other", "per-call"])

        let none = StubTransport(replies: [.success([:])])
        let bare = try client(none, namespace: nil)
        do {
            _ = try await bare.collections.get("notes")
            XCTFail("no namespace went out")
        } catch {
            XCTAssertEqual(LoamsError.code(of: error), .invalidArgument)
            XCTAssertEqual((error as? LoamsError)?.metadata["field"], "namespace")
        }
        XCTAssertEqual(none.calls.count, 0)
    }

    func test_swift_timeout_deadline() async throws {
        let hanging = HangingTransport()
        let loams = try Loams(
            Options(endpoint: URL(string: "http://stub.invalid")!, namespace: "app", timeout: 0.2),
            transport: hanging
        )
        let started = Date()
        do {
            _ = try await loams.collections.get("notes")
            XCTFail("the call outlived its deadline")
        } catch let error as LoamsError {
            XCTAssertEqual(error.code, .deadlineExceeded)
            XCTAssertEqual(error.unknownReason, loamsClientDeadlineExceeded)
        }
        XCTAssertLessThan(Date().timeIntervalSince(started), 5)
        XCTAssertEqual(hanging.requests.count, 1, "a spent budget is not retried")
        let sent = Int(hanging.requests[0].headers["connect-timeout-ms"] ?? "") ?? -1
        XCTAssertTrue((1...200).contains(sent), "connect-timeout-ms was \(sent)")

        do {
            _ = try await loams.collections.get("notes", options: CallOptions(timeout: 0.05))
            XCTFail("the per-call deadline was ignored")
        } catch let error as LoamsError {
            XCTAssertEqual(error.metadata["timeout_ms"], "50")
        }
        XCTAssertEqual(loamsDefaultTimeout, 30)
    }

    func test_swift_write_batch_chunks() async throws {
        let docs = (0..<25).map { Doc(.uint(UInt64($0)), ["n": .int($0)]) }
        let stub = StubTransport(replies: [
            .success(["token": .string("v1:s1/p0@10")]),
            .failure(code: .unavailable, message: "restarting", info: nil),
            .success(["token": .string("v1:s1/p1@4")]),
            .success(["token": .string("v1:s1/p0@25")]),
        ])
        let loams = try client(stub)
        let result = try await loams.documents.writeBatch("notes", docs, chunkRows: 10)
        XCTAssertEqual(result.chunks, 3)
        XCTAssertEqual(result.ops, 25)
        XCTAssertEqual(result.token, "v1:s1/p0@25,s1/p1@4")
        let sizes = stub.calls.map { call -> Int in
            if case .array(let ops)? = call.body["ops"] { return ops.count }
            return 0
        }
        XCTAssertEqual(sizes, [10, 10, 10, 5])
        let keys = stub.idempotencyKeys
        XCTAssertEqual(keys.count, 4)
        XCTAssertEqual(keys[1], keys[2], "the retried chunk reuses its key")
        XCTAssertEqual(Set(keys).count, 3)
        XCTAssertEqual(loamsChunks(docs.map(\.op), rows: 10_000, bytes: 1).count, 25, "bytes cut a chunk")

        let failing = StubTransport(replies: [
            .success(["token": .string("v1:s1/p0@10")]),
            .failure(code: .invalidArgument, message: "bad doc", info: nil),
        ])
        let other = try client(failing)
        do {
            _ = try await other.documents.writeBatch("notes", docs, chunkRows: 10)
            XCTFail("a failed chunk did not stop the batch")
        } catch let error as BatchError {
            XCTAssertEqual(error.appliedChunks, 1)
            XCTAssertEqual(error.appliedOps, 10)
            XCTAssertEqual(error.token, "v1:s1/p0@10")
            XCTAssertEqual(error.reason, loamsBatchPartiallyApplied)
        }
        XCTAssertEqual(failing.calls.count, 2)
    }

    func test_swift_streams_subscribe_resumes() async throws {
        actor Script {
            var pages: [[StreamRecord]]
            var asked: [UInt64] = []
            var waits: [Int] = []
            init(_ pages: [[StreamRecord]]) { self.pages = pages }
            func fetch(_ offset: UInt64) -> [StreamRecord] {
                asked.append(offset)
                return pages.isEmpty ? [] : pages.removeFirst()
            }
            func wait(_ ms: Int) { waits.append(ms) }
        }
        @Sendable func record(_ offset: UInt64) -> StreamRecord {
            StreamRecord(partition: 0, offset: offset, value: Data("{\"n\":\(offset)}".utf8))
        }
        let script = Script([[record(0), record(1)], [], [], [record(2), record(3)]])
        let subscription = Subscription(from: 0, sleep: { await script.wait($0) }, fetch: { await script.fetch($0) })
        var seen: [UInt64] = []
        for try await item in subscription {
            seen.append(item.offset)
            if seen.count == 3 { break }
        }
        XCTAssertEqual(seen, [0, 1, 2])
        XCTAssertEqual(subscription.offset, 3)
        let asked = await script.asked
        XCTAssertEqual(asked, [0, 2, 2, 2])
        let waits = await script.waits
        XCTAssertEqual(waits, [100, 200])

        let resumed = Subscription(from: subscription.offset, sleep: { _ in }, fetch: { offset in
            offset == 3 ? [record(3)] : []
        })
        for try await item in resumed {
            XCTAssertEqual(item.offset, 3)
            break
        }
        XCTAssertEqual(resumed.offset, 4)
    }
}

final class AppBuilderTests: XCTestCase {
    private let embedding = [0.1, 0.2, 0.3, 0.4]
    private let textRefund: JSONValue = .object(["text": .object([
        "query": .object(["match": .object(["field": .string("body"), "text": .string("refund")])]),
    ])])

    /// Each case's builder output, keyed by its file name in `sdks/fixtures/builders`.
    private func corpusCases() -> [String: JSONValue] { builderCorpusCases() }

    /// Numbers compare by value, as the corpus README asks: `1` and `1.0` are one value.
    private func normalized(_ value: JSONValue) -> JSONValue {
        switch value {
        case .int(let i): return .number(Double(i))
        case .array(let a): return .array(a.map(normalized))
        case .object(let o): return .object(o.mapValues(normalized))
        default: return value
        }
    }

    private func decode(_ text: String) throws -> JSONValue {
        try JSONDecoder().decode(JSONValue.self, from: Data(text.utf8))
    }

    func test_swift_builders_match_corpus() throws {
        let cases = corpusCases()
        let textRefund = #"{"text":{"query":{"match":{"field":"body","text":"refund"}}}}"#
        let vectorUnit = #"{"vector":{"field":"embedding","query":[1,0,0,0],"k":5}}"#
        let pageBody = #""collection":"notes","retrievers":[\#(textRefund)],"filter":{"term":{"field":"tags","value":"policy"}},"highlight":{"fields":[{"field":"body"}]},"select":{"source":{"include":["title"]}},"limit":2,"sort":[{"score":{}},{"pk":{}}]"#
        let text = { (name: String) in
            #"{"name":"\#(name)","source_path":"\#(name)","kind":{"text":{"analyzer":"standard","positions":true}},"indexed":true,"fast":false}"#
        }
        // The corpus's `expect`s, inline so the test holds on a tree without the corpus.
        let expected: [String: String] = [
            "schema_notes": #"{"fields":[\#(text("title")),\#(text("body")),{"name":"tags","source_path":"tags","kind":"keyword","indexed":true,"fast":true},{"name":"n","source_path":"n","kind":"i64","indexed":true,"fast":true}],"vectors":[{"name":"embedding","dim":4,"distance":"cosine"}],"sparse_vectors":[],"dynamic":"ignore","max_fields":1000}"#,
            "filter_term": #"{"term":{"field":"tags","value":"policy"}}"#,
            "filter_bool_nested": #"{"bool":{"must":[{"term":{"field":"tags","value":"policy"}},{"bool":{"should":[{"range":{"field":"n","gte":2}},{"bool":{"mustNot":[{"exists":{"field":"archived"}}]}}]}}]}}"#,
            "filter_range_open": #"{"range":{"field":"n","gt":1}}"#,
            "filter_ids": #"{"ids":[{"uint":"1"},"k-str",{"uuid":"0190f5c4-6c1e-7b3a-9d2e-4f5a6b7c8d9e"}]}"#,
            "search_text_filter_highlight_select": "{\(pageBody)}",
            "search_after_page": "{\(pageBody),\"searchAfter\":[0.3438858389854431,3]}",
            "search_vector_ann_params": #"{"collection":"notes","retrievers":[{"vector":{"field":"embedding","query":[1,0,0,0],"k":3,"params":{"nprobes":8,"refineFactor":2}}}],"limit":3}"#,
            "search_hybrid_rrf": #"{"collection":"notes","retrievers":[\#(textRefund),\#(vectorUnit)],"fusion":{"rrf":{"k":60}},"limit":3}"#,
            "search_hybrid_dbsf": #"{"collection":"notes","retrievers":[\#(textRefund),\#(vectorUnit)],"fusion":{"dbsf":{}},"limit":3}"#,
            "search_hybrid_weighted": #"{"collection":"notes","retrievers":[\#(textRefund),\#(vectorUnit)],"fusion":{"weightedSum":{"weights":[0.7,0.3]}},"limit":3}"#,
            "search_expand": #"{"collection":"notes","retrievers":[\#(textRefund)],"expandSpec":{"graph":"notes_graph","fromField":"id","hops":1}}"#,
        ]
        XCTAssertEqual(Set(cases.keys), Set(expected.keys))
        for (name, json) in expected {
            XCTAssertEqual(normalized(cases[name] ?? .null), normalized(try decode(json)), name)
        }

        // The shared corpus (SDKP P0d), when this tree has it: every file must be a case here.
        let corpus = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
            .deletingLastPathComponent().appendingPathComponent("fixtures/builders")
        guard let files = try? FileManager.default.contentsOfDirectory(atPath: corpus.path) else { return }
        for file in files.sorted() where file.hasSuffix(".json") {
            let name = String(file.dropLast(5))
            let data = try Data(contentsOf: corpus.appendingPathComponent(file))
            guard case .object(let entry) = try JSONDecoder().decode(JSONValue.self, from: data),
                  let expect = entry["expect"] else { return XCTFail("\(file): no expect") }
            guard let built = cases[name] else { XCTFail("\(name): a corpus case this test does not build"); continue }
            XCTAssertEqual(normalized(built), normalized(expect), name)
        }
    }

    func test_swift_filter_ir_matches_the_engine_mapping() {
        let ir = Filter.ir(Filter.and(Filter.all(), Filter.not(Filter.isNull("x")), Filter.ids([1, "a"])))
        XCTAssertEqual(ir, .object(["bool": .object(["must": .array([
            .string("match_all"),
            .object(["bool": .object(["must_not": .array([.object(["is_null": .object(["field": .string("x")])])])])]),
            .object(["ids": .array([.int(1), .string("a")])]),
        ])])]))
    }

    func test_swift_collection_handle_delegates() async throws {
        let stub = StubTransport(replies: [.success([:])])
        let loams = try client(stub)
        let notes = loams.collection("notes", namespace: "other")
        _ = try await notes.upsert([Doc(1, ["title": .string("a")])])
        _ = try await notes.get([1])
        _ = try await notes.delete([1])
        _ = try await notes.patch(1, set: ["title": .string("b")], unset: ["body"])
        _ = try await notes.count(where: Filter.eq("tags", "x"))
        _ = try await notes.search { $0.text("refund", fields: ["body"]) }
        _ = try await notes.vectorSearch(embedding, field: "embedding")
        _ = try await notes.info()
        _ = try await notes.drop()
        let c = "loams.collection.v1."
        XCTAssertEqual(stub.calls.map(\.rpc), [
            c + "DocumentService/WriteDocuments", c + "DocumentService/GetDocuments",
            c + "DocumentService/WriteDocuments", c + "DocumentService/WriteDocuments",
            c + "DocumentService/CountDocuments", c + "QueryService/Search", c + "QueryService/Search",
            c + "CollectionService/GetCollection", c + "CollectionService/DropCollection",
        ])
        for call in stub.calls {
            XCTAssertEqual(call.body["namespace"], .string("other"))
            XCTAssertEqual(call.body["collection"], .string("notes"))
        }
        XCTAssertEqual(stub.calls[4].body["filter"],
                       .object(["term": .object(["field": .string("tags"), "value": .string("x")])]))

        _ = try await loams.collection("notes").info()
        XCTAssertEqual(stub.calls.last?.body["namespace"], .string("app"))
    }

    func test_swift_search_query_all_requires_tiebreak() async throws {
        let stub = StubTransport(replies: [.success([:])])
        let loams = try client(stub)
        XCTAssertThrowsError(try loams.search.queryAll(Search.on("notes").text("refund").sort(.score()))) { error in
            XCTAssertEqual((error as? LoamsError)?.metadata["field"], "sort")
        }
        XCTAssertEqual(stub.calls.count, 0)

        let paged = StubTransport(replies: [
            .success(["hits": .array([
                .object(["pk": .object(["uint": .string("1")]), "sortValues": .array([.int(2), .int(1)])]),
                .object(["pk": .object(["uint": .string("2")]), "sortValues": .array([.int(1), .int(2)])]),
            ])]),
            .success(["hits": .array([
                .object(["pk": .object(["uint": .string("3")]), "sortValues": .array([.number(0.5), .int(3)])]),
            ])]),
        ])
        let other = try client(paged)
        var ids: [String] = []
        for try await hit in try other.search.queryAll(Search.on("notes").text("refund").sort(.score(), .pk()).limit(2)) {
            if case .object(let fields) = hit, let id = DocumentID(json: fields["pk"]) { ids.append(id.text) }
        }
        XCTAssertEqual(ids, ["1", "2", "3"])
        XCTAssertEqual(paged.calls.count, 2)
        XCTAssertEqual(paged.calls[1].body["searchAfter"], .array([.int(1), .int(2)]))
    }

    func test_swift_app_bindings() throws {
        let safe = [
            ("namespaces", "Create"), ("namespaces", "List"), ("collections", "Create"), ("collections", "List"),
            ("collections", "Get"), ("collections", "Drop"), ("collections", "AddFields"),
            ("collections", "ListVersions"), ("collections", "UpdateAliases"), ("collections", "SetHot"),
            ("collections", "Warm"), ("documents", "Get"), ("documents", "Scroll"), ("documents", "Count"),
            ("search", "Query"), ("vector", "Search"), ("graph", "Get"), ("graph", "List"), ("graph", "Schema"),
            ("graph", "Explain"),
        ]
        for (module, name) in safe {
            XCTAssertEqual(try loamsRequireBinding(module: module, call: name).retry, .safe, "\(module).\(name)")
        }
        let manual = [
            ("collections", "Scan"), ("documents", "Write"), ("documents", "DeleteByFilter"),
            ("documents", "PatchByFilter"), ("graph", "Create"), ("graph", "Update"), ("graph", "Delete"),
            ("graph", "Query"), ("graph", "QueryBatch"), ("graph", "QueryStream"),
        ]
        for (module, name) in manual {
            XCTAssertEqual(try loamsRequireBinding(module: module, call: name).retry, .manual, "\(module).\(name)")
        }
        XCTAssertEqual(try loamsRequireBinding(module: "search", call: "Query").rpc,
                       try loamsRequireBinding(module: "vector", call: "Search").rpc)
        XCTAssertEqual(try loamsRequireBinding(module: "documents", call: "Write").rpc,
                       "loams.collection.v1.DocumentService/WriteDocuments")
        XCTAssertTrue(try loamsRequireBinding(module: "documents", call: "Write").takesIdempotencyKey)
        XCTAssertEqual(try loamsRequireBinding(module: "graph", call: "QueryStream").streaming, .server)
        XCTAssertTrue(try XCTUnwrap(loamsModule("graph")).unstable)
    }

    func test_swift_graph_read_only_query_is_unkeyed_and_retried() async throws {
        let stub = StubTransport(replies: [
            .failure(code: .unavailable, message: "restarting", info: nil),
            .success(["rows": .object(["columns": .array([.string("n")])])]),
        ])
        let loams = try client(stub)
        _ = try await loams.graph.query("g", "MATCH (n) RETURN n", readOnly: true)
        XCTAssertEqual(stub.calls.count, 2)
        XCTAssertNil(stub.calls[0].body[ProtoField.idempotencyKey])
        XCTAssertEqual(stub.calls[0].body["namespace"], .string("app"))

        let write = StubTransport(replies: [.success([:])])
        let writer = try client(write)
        _ = try await writer.graph.query("g", "INSERT (:Note {id: $id})", parameters: ["id": GraphValue.int(1)])
        XCTAssertEqual(write.idempotencyKeys.count, 1)
    }
}
