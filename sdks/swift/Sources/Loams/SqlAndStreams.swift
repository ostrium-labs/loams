// The `sql` and `streams` modules (SDKP §2.5, Part C): `loams.sql.v1` and
// `loams.stream.v1` on the main port (SDKP P0b and P0c).

import Foundation

/// `loams.sql`: one read-only statement over a namespace's collections, with `$n` parameters.
public struct SqlModule: Sendable {
    let invoker: CallInvoker

    /// Runs `sql` with `params` bound to `$1…$n` (a count mismatch is `invalid_argument`,
    /// reason `sql_param_count`) and answers its columns and rows.
    public func query(_ sql: String, params: [JSONValue] = [], maxRows: Int? = nil, consistency: Consistent? = nil,
                      namespace: String? = nil, options: CallOptions = .init()) async throws -> DynamicMessage {
        var fields: [String: JSONValue] = ["sql": .string(sql)]
        if !params.isEmpty { fields["params"] = .array(params) }
        if let maxRows { fields["maxRows"] = .int(maxRows) }
        if let consistency { fields["consistency"] = consistency.json }
        if let namespace { fields["namespace"] = .string(namespace) }
        let binding = try loamsRequireBinding(module: "sql", call: "Query")
        return try await invoker.unary(binding: binding,
                                       request: DynamicMessage(protoTypeName: "loams.sql.v1.QueryRequest", fields: fields),
                                       overrides: options)
    }

    /// Each row of an answer as a dictionary of column name to value.
    public static func records(_ response: DynamicMessage) -> [[String: JSONValue]] {
        guard case .array(let columns)? = response.fields["columns"] else { return [] }
        let names = columns.compactMap { column -> String? in
            if case .object(let c) = column { return c["name"]?.stringValue }
            return nil
        }
        guard case .array(let rows)? = response.fields["rows"] else { return [] }
        return rows.map { row in
            guard case .array(let cells) = row else { return [:] }
            var record: [String: JSONValue] = [:]
            for (i, name) in names.enumerated() where i < cells.count { record[name] = cells[i] }
            return record
        }
    }
}

/// `loams.streams`: a namespace's logs: create, list, produce, fetch and subscribe.
public struct StreamsModule: Sendable {
    let invoker: CallInvoker

    private func call(_ name: String, _ type: String, _ fields: [String: JSONValue], _ options: CallOptions)
        async throws -> DynamicMessage {
        let binding = try loamsRequireBinding(module: "streams", call: name)
        return try await invoker.unary(binding: binding,
                                       request: DynamicMessage(protoTypeName: "loams.stream.v1." + type, fields: fields),
                                       overrides: options)
    }

    /// Creates a stream. Idempotent by name and partition count, so retried.
    public func create(_ name: String, partitions: Int = 1, namespace: String? = nil,
                       options: CallOptions = .init()) async throws -> DynamicMessage {
        var fields: [String: JSONValue] = ["name": .string(name), "partitions": .int(partitions)]
        if let namespace { fields["namespace"] = .string(namespace) }
        return try await call("Create", "CreateStreamRequest", fields, options)
    }

    /// One stream, with each partition's offsets.
    public func get(_ stream: String, options: CallOptions = .init()) async throws -> DynamicMessage {
        try await call("Get", "DescribeStreamRequest", ["stream": .string(stream)], options)
    }

    /// One page of the namespace's streams.
    public func list(pageSize: Int = 0, pageToken: String = "", options: CallOptions = .init()) async throws -> DynamicMessage {
        var fields: [String: JSONValue] = [:]
        if pageSize > 0 { fields["pageSize"] = .int(pageSize) }
        if !pageToken.isEmpty { fields["pageToken"] = .string(pageToken) }
        return try await call("List", "ListStreamsRequest", fields, options)
    }

    /// Appends records to one partition: each is `Data`, a `String` (UTF-8) or any
    /// `JSONValue` (as JSON), or a ``ProducerRecord`` to give it a key and headers. With `producerId` and `producerSeq` a repeat answers the first
    /// append with `duplicate` set, so the SDK retries it; without them a produce is a
    /// plain append and is never retried.
    public func produce(_ stream: String, partition: Int = 0, records: [any Sendable], producerId: String? = nil,
                        producerSeq: UInt64? = nil, namespace: String? = nil,
                        options: CallOptions = .init()) async throws -> DynamicMessage {
        var fields: [String: JSONValue] = [
            "stream": .string(stream),
            "records": .array(try records.map(Self.recordJSON)),
        ]
        var settings = options
        // proto3 JSON omits a zero partition, as the recorded app_streams bodies do.
        if partition != 0 { fields["partition"] = .int(partition) }
        if let producerId, let producerSeq {
            fields["producerId"] = .string(producerId)
            fields["producerSeq"] = .string(String(producerSeq))
            settings.retrySafe = settings.retrySafe ?? true
        }
        if let namespace { fields["namespace"] = .string(namespace) }
        return try await call("Produce", "ProduceRequest", fields, settings)
    }

    /// One `loams.stream.v1.Record` as proto3 JSON: bytes as base64, an unset key omitted.
    static func recordJSON(_ record: any Sendable) throws -> JSONValue {
        guard let record = record as? ProducerRecord else {
            return .object(["value": .string(try encode(record).base64EncodedString())])
        }
        var fields: [String: JSONValue] = ["value": .string(try encode(record.value).base64EncodedString())]
        if let key = record.key { fields["key"] = .string(try encode(key).base64EncodedString()) }
        if !record.headers.isEmpty {
            fields["headers"] = .array(record.headers.map {
                .object(["key": .string($0.key), "value": .string($0.value.base64EncodedString())])
            })
        }
        return .object(fields)
    }

    /// A record's bytes: as given, a string's UTF-8, or a value's JSON.
    public static func encode(_ record: any Sendable) throws -> Data {
        switch record {
        case let data as Data: return data
        case let text as String: return Data(text.utf8)
        case let value as JSONValue: return try value.encoded()
        default:
            throw LoamsError.internalError("", "loams: a stream record is Data, a String or a JSONValue")
        }
    }

    /// Reads one partition from `offset`; `maxWaitMs` holds an empty fetch until a record
    /// arrives (a long poll, at most 30 000).
    public func fetch(_ stream: String, partition: Int = 0, offset: UInt64, maxRecords: Int? = nil, maxWaitMs: Int? = nil,
                      namespace: String? = nil, options: CallOptions = .init()) async throws -> DynamicMessage {
        var fields: [String: JSONValue] = [
            "stream": .string(stream),
        ]
        // proto3 JSON omits a zero partition and offset, as the recorded app_streams bodies do.
        if partition != 0 { fields["partition"] = .int(partition) }
        if offset != 0 { fields["offset"] = .string(String(offset)) }
        if let maxRecords { fields["maxRecords"] = .int(maxRecords) }
        var settings = options
        if let maxWaitMs {
            fields["maxWaitMs"] = .int(maxWaitMs)
            settings.timeout = settings.timeout ?? (loamsDefaultTimeout + Double(maxWaitMs) / 1000)
        }
        if let namespace { fields["namespace"] = .string(namespace) }
        return try await call("Fetch", "FetchRequest", fields, settings)
    }

    /// A resumable subscription to one partition (R14); each empty fetch long-polls for `maxWaitMs`.
    public func subscribe(_ stream: String, partition: Int = 0, from offset: UInt64 = 0, maxWaitMs: Int = 1000,
                          namespace: String? = nil, options: CallOptions = .init()) -> Subscription {
        let module = self
        return Subscription(from: offset) { next in
            let page = try await module.fetch(stream, partition: partition, offset: next, maxWaitMs: maxWaitMs,
                                              namespace: namespace, options: options)
            guard case .array(let records)? = page.fields["records"] else { return [] }
            return records.compactMap { record -> StreamRecord? in
                guard case .object(let r) = record else { return nil }
                let at: UInt64
                switch r["offset"] {
                case .string(let s)?: at = UInt64(s) ?? 0
                case .int(let n)?: at = UInt64(n)
                default: at = 0
                }
                let value = r["value"]?.stringValue.flatMap { Data(base64Encoded: $0) } ?? Data()
                let key = r["key"]?.stringValue.flatMap { Data(base64Encoded: $0) }
                return StreamRecord(partition: partition, offset: at, value: value, key: key)
            }
        }
    }
}

/// A record to produce with a key and headers. The key (partitioning, compaction) and the
/// value are each `Data`, a `String` (UTF-8) or a `JSONValue` (as JSON).
public struct ProducerRecord: Sendable {
    public let key: (any Sendable)?
    public let value: any Sendable
    public let headers: [StreamHeader]

    public init(key: (any Sendable)? = nil, value: any Sendable, headers: [StreamHeader] = []) {
        self.key = key
        self.value = value
        self.headers = headers
    }
}

/// One record header: a name and its bytes.
public struct StreamHeader: Sendable, Equatable {
    public let key: String
    public let value: Data

    public init(_ key: String, _ value: Data) {
        self.key = key
        self.value = value
    }

    public init(_ key: String, _ text: String) {
        self.init(key, Data(text.utf8))
    }
}
