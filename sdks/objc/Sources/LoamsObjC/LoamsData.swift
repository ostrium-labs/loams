import Foundation

/// `sql`: one read-only statement over a namespace's collections, with
/// `$1…$n` parameters. SQL reads see the session's collection writes.
#if canImport(ObjectiveC)
@objcMembers
#endif
public final class LoamsSqlModule: LoamsAppModule {
    /// The answer's `rows` are arrays in `columns` order; see `records(_:)`.
    public func query(_ sql: String, params: [Any] = [], maxRows: NSNumber? = nil, consistency: String? = nil,
                      options: LoamsCallOptions? = nil) throws -> [String: Any] {
        return try invoke("sql.query", try request(sql, params, maxRows, consistency), options)
    }

    /// The same statement, answered as Arrow IPC bytes.
    public func queryArrow(_ sql: String, params: [Any] = [], maxRows: NSNumber? = nil, consistency: String? = nil,
                           options: LoamsCallOptions? = nil) throws -> Data {
        let answer = try invoke("sql.query_arrow", try request(sql, params, maxRows, consistency), options)
        return Data(base64Encoded: answer["arrowIpc"] as? String ?? "") ?? Data()
    }

    /// A `QueryResponse`'s rows as dictionaries, column name to value.
    public static func records(_ response: [String: Any]) -> [[String: Any]] {
        let names = ((response["columns"] as? [[String: Any]]) ?? []).map { $0["name"] as? String ?? "" }
        return ((response["rows"] as? [[Any]]) ?? []).map { row in
            var record: [String: Any] = [:]
            for (i, name) in names.enumerated() { record[name] = i < row.count ? row[i] : NSNull() }
            return record
        }
    }

    private func request(_ sql: String, _ params: [Any], _ maxRows: NSNumber?, _ consistency: String?) throws -> [String: Any] {
        return loamsCompact([("sql", sql), ("params", params.isEmpty ? nil : params), ("maxRows", maxRows?.intValue),
                             ("consistency", try loamsConsistencyJSON(consistency))])
    }
}

/// One fetched record, decoded.
#if canImport(ObjectiveC)
@objcMembers
#endif
public final class LoamsStreamRecord: NSObject {
    public let offset: UInt64
    public let key: Data?
    public let value: Data?
    public let headers: [String: Data]
    public let timestampMs: Int64

    init(offset: UInt64, key: Data?, value: Data?, headers: [String: Data], timestampMs: Int64) {
        self.offset = offset
        self.key = key
        self.value = value
        self.headers = headers
        self.timestampMs = timestampMs
        super.init()
    }

    /// The value as UTF-8 text.
    public var text: String? { return value.flatMap { String(data: $0, encoding: .utf8) } }

    /// The value as JSON.
    public var json: Any? { return value.flatMap { try? JSONSerialization.jsonObject(with: $0, options: [.fragmentsAllowed]) } }
}

/// `streams`: a namespace's logs. Records are bytes on the wire; the module
/// takes `Data`, strings or JSON values (sent as UTF-8 JSON) and answers bytes.
#if canImport(ObjectiveC)
@objcMembers
#endif
public final class LoamsStreamsModule: LoamsAppModule {
    public func create(_ name: String, partitions: Int = 1, options: LoamsCallOptions? = nil) throws -> [String: Any] {
        return try invoke("streams.create", loamsCompact([("name", name), ("partitions", partitions == 0 ? nil : partitions)]), options)
    }

    public func get(_ stream: String, options: LoamsCallOptions? = nil) throws -> [String: Any] {
        return try invoke("streams.get", ["stream": stream], options)
    }

    public func list(options: LoamsCallOptions? = nil) throws -> [String: Any] {
        return try invoke("streams.list", [:], options)
    }

    public func listAll(options: LoamsCallOptions? = nil) throws -> [[String: Any]] {
        return try paged("streams.list", [:], items: "streams", options)
    }

    /// Appends `records` to one partition. A record is a value (`Data`, a string
    /// or a JSON value) or `["key": …, "value": …, "headers": [name: …]]`. With
    /// `producerId` and `producerSeq` a repeat is deduplicated (`duplicate`).
    public func produce(_ stream: String, records: [Any], partition: UInt32 = 0, producerId: String? = nil,
                        producerSeq: NSNumber? = nil, options: LoamsCallOptions? = nil) throws -> [String: Any] {
        return try invoke("streams.produce", loamsCompact([
            ("stream", stream), ("partition", partition == 0 ? nil : Int(partition)),
            ("records", try records.map(LoamsStreamsModule.record)),
            ("producerId", producerId), ("producerSeq", producerSeq?.stringValue),
        ]), options)
    }

    /// Records at and after `offset`; `records(_:)` decodes the answer.
    public func fetch(_ stream: String, partition: UInt32 = 0, offset: UInt64 = 0, maxRecords: NSNumber? = nil,
                      maxBytes: NSNumber? = nil, maxWaitMs: NSNumber? = nil, options: LoamsCallOptions? = nil) throws -> [String: Any] {
        return try invoke("streams.fetch", loamsCompact([
            ("stream", stream), ("partition", partition == 0 ? nil : Int(partition)),
            ("offset", offset == 0 ? nil : String(offset)), ("maxBytes", maxBytes?.intValue),
            ("maxRecords", maxRecords?.intValue), ("maxWaitMs", maxWaitMs?.intValue),
        ]), options)
    }

    /// CloudEvents (dictionaries) appended as one JSON batch.
    public func produceEvents(_ stream: String, events: [[String: Any]], partition: NSNumber? = nil,
                              options: LoamsCallOptions? = nil) throws -> [String: Any] {
        let batch = try JSONSerialization.data(withJSONObject: events).base64EncodedString()
        return try invoke("streams.produce_events", loamsCompact([
            ("stream", stream), ("partition", partition?.intValue), ("jsonBatch", batch),
        ]), options)
    }

    /// CloudEvents at and after `offset`, decoded from the answer's JSON batch.
    public func fetchEvents(_ stream: String, partition: UInt32 = 0, offset: UInt64 = 0,
                            options: LoamsCallOptions? = nil) throws -> [[String: Any]] {
        let answer = try invoke("streams.fetch_events", loamsCompact([
            ("stream", stream), ("partition", partition == 0 ? nil : Int(partition)),
            ("offset", offset == 0 ? nil : String(offset)),
        ]), options)
        guard let text = answer["jsonBatch"] as? String, let data = Data(base64Encoded: text), !data.isEmpty else { return [] }
        return (try JSONSerialization.jsonObject(with: data) as? [[String: Any]]) ?? []
    }

    /// R14: every record from `fromOffset` on, as a loop over `fetch`. A fetch
    /// that fails stops the subscription.
    public func subscribe(_ stream: String, partition: UInt32 = 0, fromOffset: UInt64 = 0, maxRecords: NSNumber? = nil,
                          maxWaitMs: NSNumber? = nil, options: LoamsCallOptions? = nil) -> LoamsSubscription {
        var stop: LoamsSubscription?
        let sub = LoamsSubscription(fromOffset: fromOffset, fetch: { [weak self] offset in
            guard let self = self,
                  let answer = try? self.fetch(stream, partition: partition, offset: offset, maxRecords: maxRecords,
                                               maxWaitMs: maxWaitMs, options: options)
            else {
                stop?.cancel()
                return []
            }
            return LoamsStreamsModule.records(answer).map { r -> [String: Any] in
                var out: [String: Any] = ["offset": NSNumber(value: r.offset), "headers": r.headers, "timestampMs": NSNumber(value: r.timestampMs)]
                out["key"] = r.key
                out["value"] = r.value
                return out
            }
        })
        stop = sub
        return sub
    }

    /// A `FetchResponse`'s records, decoded.
    public static func records(_ response: [String: Any]) -> [LoamsStreamRecord] {
        return ((response["records"] as? [[String: Any]]) ?? []).map { r in
            var headers: [String: Data] = [:]
            for h in (r["headers"] as? [[String: Any]]) ?? [] {
                if let k = h["key"] as? String { headers[k] = (h["value"] as? String).flatMap { Data(base64Encoded: $0) } ?? Data() }
            }
            return LoamsStreamRecord(
                offset: UInt64(r["offset"] as? String ?? "0") ?? 0,
                key: (r["key"] as? String).flatMap { Data(base64Encoded: $0) },
                value: (r["value"] as? String).flatMap { Data(base64Encoded: $0) },
                headers: headers,
                timestampMs: Int64(r["timestampMs"] as? String ?? "0") ?? 0)
        }
    }

    static func record(_ record: Any) throws -> [String: Any] {
        if let d = record as? [String: Any], d["value"] != nil || d["key"] != nil {
            var headers: [[String: Any]] = []
            for (k, v) in (d["headers"] as? [String: Any]) ?? [:] {
                headers.append(["key": k, "value": try bytes(v).base64EncodedString()])
            }
            return loamsCompact([("key", try d["key"].map { try bytes($0).base64EncodedString() }),
                                 ("value", try d["value"].map { try bytes($0).base64EncodedString() }),
                                 ("headers", headers.isEmpty ? nil : headers)])
        }
        return ["value": try bytes(record).base64EncodedString()]
    }

    static func bytes(_ value: Any) throws -> Data {
        if let d = value as? Data { return d }
        if let s = value as? String { return Data(s.utf8) }
        return try JSONSerialization.data(withJSONObject: value, options: [.fragmentsAllowed, .sortedKeys])
    }
}
