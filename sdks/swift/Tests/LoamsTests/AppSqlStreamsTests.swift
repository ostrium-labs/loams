// SDKP Part C (L7): the `sql` and `streams` modules, against the stub transport.

import Foundation
import XCTest
@testable import Loams

final class AppSqlStreamsTests: XCTestCase {
    private func client(_ transport: StubTransport) throws -> Loams {
        try Loams(Options(endpoint: URL(string: "http://stub.invalid")!, maxRetries: 3, namespace: "app"), transport: transport)
    }

    func test_swift_sql_params_bind_and_rows_read_as_records() async throws {
        let stub = StubTransport(replies: [.success([
            "columns": .array([.object(["name": .string("tags")]), .object(["name": .string("c")])]),
            "rows": .array([.array([.string("a"), .int(2)])]),
        ])])
        let answer = try await client(stub).sql.query("SELECT tags, count(*) AS c FROM notes WHERE n > $1 GROUP BY tags",
                                                     params: [.int(1)])
        XCTAssertEqual(stub.calls[0].rpc, "loams.sql.v1.SqlService/Query")
        XCTAssertEqual(stub.calls[0].body["params"], .array([.int(1)]))
        XCTAssertEqual(stub.calls[0].body["namespace"], .string("app"))
        XCTAssertEqual(SqlModule.records(answer), [["tags": .string("a"), "c": .int(2)]])
    }

    func test_swift_streams_produce_is_retried_only_with_a_producer_key() async throws {
        let unkeyed = StubTransport(replies: [.failure(code: .unavailable, message: "restarting", info: nil)])
        do {
            _ = try await client(unkeyed).streams.produce("events", records: ["a"])
            XCTFail("an unkeyed produce succeeded")
        } catch {}
        XCTAssertEqual(unkeyed.calls.count, 1)

        let keyed = StubTransport(replies: [
            .failure(code: .unavailable, message: "restarting", info: nil),
            .success(["lastOffset": .string("2")]),
        ])
        _ = try await client(keyed).streams.produce("events", records: ["text", JSONValue.object(["n": .int(1)])],
                                                    producerId: "app-1", producerSeq: 7)
        XCTAssertEqual(keyed.calls.count, 2)
        XCTAssertEqual(keyed.calls[0].body["producerId"], .string("app-1"))
        guard case .array(let records)? = keyed.calls[0].body["records"] else { return XCTFail("no records") }
        let values = records.compactMap { record -> String? in
            guard case .object(let r) = record, let b64 = r["value"]?.stringValue, let data = Data(base64Encoded: b64) else { return nil }
            return String(decoding: data, as: UTF8.self)
        }
        XCTAssertEqual(values, ["text", "{\"n\":1}"])
        XCTAssertNil(keyed.calls[0].body["partition"], "proto3 JSON omits a zero partition")
    }

    /// Keyed records (SDKP-25) encode as the recorded `app_streams` produce body does.
    func test_swift_streams_produce_keyed_records() async throws {
        let stub = StubTransport(replies: [.success(["lastOffset": .string("1")])])
        let records: [any Sendable] = [
            ProducerRecord(key: "1", value: JSONValue.object(["note": .int(1), "op": .string("created")])),
            ProducerRecord(key: "5", value: JSONValue.object(["note": .int(5), "op": .string("deleted")]),
                           headers: [StreamHeader("source", "notes")]),
            "plain",
        ]
        _ = try await client(stub).streams.produce("note-events", records: records,
                                                    producerId: "app-producer-1", producerSeq: 1)
        XCTAssertEqual(stub.calls[0].body["records"], .array([
            .object(["key": .string("MQ=="), "value": .string("eyJub3RlIjoxLCJvcCI6ImNyZWF0ZWQifQ==")]),
            .object(["key": .string("NQ=="), "value": .string("eyJub3RlIjo1LCJvcCI6ImRlbGV0ZWQifQ=="),
                     "headers": .array([.object(["key": .string("source"), "value": .string("bm90ZXM=")])])]),
            .object(["value": .string("cGxhaW4=")]),
        ]))
        XCTAssertEqual(stub.calls[0].body["producerSeq"], .string("1"))
    }

    /// The recorded `app_streams` bodies are proto3 JSON: a zero partition or offset is omitted.
    func test_swift_streams_fetch_omits_zero_partition_and_offset() async throws {
        let stub = StubTransport(replies: [.success([:]), .success([:])])
        let loams = try client(stub)
        _ = try await loams.streams.fetch("note-events", offset: 0, maxRecords: 10)
        XCTAssertEqual(stub.calls[0].body, ["stream": .string("note-events"), "maxRecords": .int(10), "namespace": .string("app")])
        _ = try await loams.streams.fetch("note-events", partition: 1, offset: 2, maxRecords: 10)
        XCTAssertEqual(stub.calls[1].body["partition"], .int(1))
        XCTAssertEqual(stub.calls[1].body["offset"], .string("2"))
    }
}
