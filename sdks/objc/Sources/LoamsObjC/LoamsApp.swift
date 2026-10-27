import Foundation

/// One app call: its RPC, its retry class (D610: `safe`, `keyed` or `never`),
/// whether its request names a namespace, and whether the session applies to it.
public struct LoamsAppCall: Sendable {
    public let rpc: String
    public let retry: String
    public let namespaced: Bool
    public let read: Bool

    /// The proto package: the session's scope.
    /// The session's scope: the proto package, except that SQL reads (and adds
    /// to) the collection token, since it reads collections.
    public var scope: String {
        let service = rpc.split(separator: "/")[0]
        let package = String(service[..<service.lastIndex(of: ".")!])
        return package == "loams.sql.v1" ? "loams.collection.v1" : package
    }
}

/// The RPCs behind every app call (SDKP §2), `module.call` to its `LoamsAppCall`.
/// Checked against `sdks/fixtures/facade-index.json` by `objc_facade_matches_index`.
public let loamsAppCalls: [String: LoamsAppCall] = {
    func c(_ rpc: String, _ retry: String, namespaced: Bool = true, read: Bool = false) -> LoamsAppCall {
        return LoamsAppCall(rpc: rpc, retry: retry, namespaced: namespaced, read: read)
    }
    let col = "loams.collection.v1."
    let gr = "loams.graph.v1."
    return [
        "namespaces.create": c(col + "NamespaceService/CreateNamespace", "safe", namespaced: false),
        "namespaces.list": c(col + "NamespaceService/ListNamespaces", "safe", namespaced: false),
        "collections.create": c(col + "CollectionService/CreateCollection", "safe"),
        "collections.list": c(col + "CollectionService/ListCollections", "safe"),
        "collections.get": c(col + "CollectionService/GetCollection", "safe"),
        "collections.drop": c(col + "CollectionService/DropCollection", "safe"),
        "collections.add_fields": c(col + "CollectionService/AddFields", "safe"),
        "collections.list_versions": c(col + "CollectionService/ListVersions", "safe"),
        "collections.update_aliases": c(col + "CollectionService/UpdateAliases", "safe"),
        "collections.scan": c(col + "CollectionService/Scan", "never"),
        "collections.set_hot": c(col + "CollectionService/SetHot", "safe"),
        "collections.warm": c(col + "CollectionService/WarmCollection", "safe"),
        "documents.write": c(col + "DocumentService/WriteDocuments", "keyed"),
        "documents.get": c(col + "DocumentService/GetDocuments", "safe", read: true),
        "documents.scroll": c(col + "DocumentService/ScrollDocuments", "safe", read: true),
        "documents.count": c(col + "DocumentService/CountDocuments", "safe", read: true),
        "documents.delete_by_filter": c(col + "DocumentService/DeleteByFilter", "never", read: true),
        "documents.patch_by_filter": c(col + "DocumentService/PatchByFilter", "never", read: true),
        "search.query": c(col + "QueryService/Search", "safe", read: true),
        "vector.search": c(col + "QueryService/Search", "safe", read: true),
        "graph.create": c(gr + "GraphAdminService/CreateGraph", "keyed"),
        "graph.get": c(gr + "GraphAdminService/GetGraph", "safe"),
        "graph.list": c(gr + "GraphAdminService/ListGraphs", "safe"),
        "graph.update": c(gr + "GraphAdminService/UpdateGraph", "keyed", namespaced: false),
        "graph.delete": c(gr + "GraphAdminService/DeleteGraph", "keyed"),
        "graph.schema": c(gr + "GraphAdminService/GetSchema", "safe"),
        "graph.query": c(gr + "GraphService/Execute", "keyed", read: true),
        "graph.query_batch": c(gr + "GraphService/ExecuteBatch", "keyed"),
        "graph.explain": c(gr + "GraphService/Explain", "safe", read: true),
        "sql.query": c("loams.sql.v1.SqlService/Query", "safe", read: true),
        "sql.query_arrow": c("loams.sql.v1.SqlService/QueryArrow", "safe", read: true),
        "streams.create": c("loams.stream.v1.StreamService/CreateStream", "safe"),
        "streams.get": c("loams.stream.v1.StreamService/DescribeStream", "safe"),
        "streams.list": c("loams.stream.v1.StreamService/ListStreams", "safe"),
        "streams.produce": c("loams.stream.v1.StreamService/Produce", "never"),
        "streams.fetch": c("loams.stream.v1.StreamService/Fetch", "safe"),
        "streams.produce_events": c("loams.stream.v1.StreamService/ProduceCloudEvents", "never"),
        "streams.fetch_events": c("loams.stream.v1.StreamService/FetchCloudEvents", "safe"),
    ]
}()

/// The packages whose reads take the session's token (SDKP-17).
public let loamsSessionScopes: Set<String> = ["loams.collection.v1"]

/// What every app call takes besides its arguments.
#if canImport(ObjectiveC)
@objcMembers
#endif
public final class LoamsCallOptions: NSObject {
    /// This call's budget in seconds; negative is the client's, 0 is none (R12).
    public var timeout: TimeInterval = -1
    /// The idempotency key of a keyed call (R3); nil mints one.
    public var idempotencyKey: String?
    /// The namespace; nil is the client's default (R11).
    public var namespace: String?
    /// Extra request headers.
    public var headers: [String: String] = [:]

    public override init() { super.init() }

    public init(namespace: String?) {
        self.namespace = namespace
        super.init()
    }
}

/// What the modules share with their client.
public final class LoamsAppContext: @unchecked Sendable {
    let transport: LoamsTransport
    let defaultNamespace: String?
    let session: LoamsSessionStore?

    init(transport: LoamsTransport, defaultNamespace: String?, session: LoamsSessionStore?) {
        self.transport = transport
        self.defaultNamespace = defaultNamespace
        self.session = session
    }
}

/// Conversions between Swift values and the proto3 JSON the modules send and answer.
#if canImport(ObjectiveC)
@objcMembers
#endif
public final class LoamsValues: NSObject {
    /// A primary key as a `DocumentId`'s JSON: a non-negative integer is `uint`, a
    /// string is `string`, a dictionary naming its arm (`["uuid": …]`) passes through.
    public static func documentId(_ id: Any) throws -> [String: Any] {
        switch id {
        case let i as Int:
            guard i >= 0 else { throw invalidArgument("a document id is a non-negative integer, a string or [\"uuid\": …]") }
            return ["uint": String(i)]
        case let u as UInt64: return ["uint": String(u)]
        case let s as String: return ["string": s]
        case let d as [String: Any]: return d
        case let n as NSNumber: return ["uint": n.stringValue]
        default: throw invalidArgument("a document id is a non-negative integer, a string or [\"uuid\": …]")
        }
    }

    /// A `DocumentId`'s JSON back: an `NSNumber` (unsigned), a string, or the dictionary.
    public static func fromDocumentId(_ id: [String: Any]) -> Any {
        if let uint = id["uint"] as? String, let value = UInt64(uint) { return NSNumber(value: value) }
        if let string = id["string"] as? String { return string }
        return id
    }

    /// A value as a `loams.graph.v1.Value`'s JSON, for GQL parameters.
    public static func graphValue(_ value: Any?) throws -> [String: Any] {
        switch value {
        case nil: return ["null": "NULL_VALUE"]
        case is NSNull: return ["null": "NULL_VALUE"]
        case let b as Bool: return ["boolean": b]
        case let i as Int: return ["int64": String(i)]
        case let d as Double: return ["float64": d]
        case let s as String: return ["string": s]
        case let l as [Any]: return ["list": ["values": try l.map { try graphValue($0) }]]
        case let m as [String: Any]:
            var entries: [String: Any] = [:]
            for (k, v) in m { entries[k] = try graphValue(v) }
            return ["map": ["entries": entries]]
        default: throw invalidArgument("no graph value for \(type(of: value!))")
        }
    }

    /// A `loams.graph.v1.Value`'s JSON as a value. Nodes, relationships, paths and
    /// temporal values stay as their JSON.
    public static func fromGraphValue(_ value: [String: Any]) -> Any {
        guard let (kind, inner) = value.first else { return NSNull() }
        switch kind {
        case "null": return NSNull()
        case "boolean", "float64", "string": return inner
        case "int64": return (inner as? String).flatMap { Int64($0) }.map { NSNumber(value: $0) } ?? inner
        case "uint64": return (inner as? String).flatMap { UInt64($0) }.map { NSNumber(value: $0) } ?? inner
        case "list": return (((inner as? [String: Any])?["values"] as? [[String: Any]]) ?? []).map { fromGraphValue($0) }
        case "map":
            var out: [String: Any] = [:]
            for (k, v) in ((inner as? [String: Any])?["entries"] as? [String: [String: Any]]) ?? [:] {
                out[k] = fromGraphValue(v)
            }
            return out
        default: return value
        }
    }

    /// A client-side `invalid_argument` refusal.
    public static func invalidArgument(_ message: String, reason: String? = nil) -> LoamsError {
        return LoamsError(message: message, codeString: "invalid_argument", reason: reason, unknownReason: nil, httpStatus: 400)
    }
}

/// The call path every app module shares: proto3 JSON in, proto3 JSON out
/// (`lowerCamelCase` keys, 64-bit integers as strings), through the client's
/// transport, so retries, keys, token refresh, timeouts and errors are R1–R12.
#if canImport(ObjectiveC)
@objcMembers
#endif
public class LoamsAppModule: NSObject {
    let context: LoamsAppContext

    init(context: LoamsAppContext) {
        self.context = context
        super.init()
    }

    /// Sends `request` to the call `name` (a key of `loamsAppCalls`).
    ///
    /// R11: a request that names no namespace gets the client's default, and with
    /// neither it is refused here, before any network call. R4: with the session
    /// on, a read that leaves `consistency` unset reads at least the session's token,
    /// and every token an answer carries is merged in.
    func invoke(_ name: String, _ request: [String: Any], _ options: LoamsCallOptions?, retry: String? = nil) throws -> [String: Any] {
        let call = loamsAppCalls[name]!
        var body = request
        if body["namespace"] == nil, let ns = options?.namespace {
            body["namespace"] = ns
        }
        if call.namespaced {
            body = try withNamespace(body, rpc: call.rpc)
        }
        // SDKP-17: only collection (and SQL) reads take the session's token.
        if call.read, loamsSessionScopes.contains(call.scope), let session = context.session {
            body = session.apply(scope: call.scope, request: body)
        }
        let klass = retry ?? call.retry
        var key: String?
        if klass == "keyed" {
            key = options?.idempotencyKey ?? (body["idempotencyKey"] as? String) ?? LoamsIdempotency.mintKey()
            body["idempotencyKey"] = key
        }
        let data = try JSONSerialization.data(withJSONObject: body.isEmpty ? [String: Any]() : body)
        let answerData = try context.transport.callWithRetry(
            path: "/" + call.rpc,
            headers: ["Content-Type": "application/json", "Connect-Protocol-Version": "1"]
                .merging(options?.headers ?? [:]) { _, extra in extra },
            body: data,
            isMutation: klass != "safe",
            idempotencyKey: key,
            timeout: options?.timeout ?? -1)
        let answer = answerData.isEmpty ? [:] : ((try? JSONSerialization.jsonObject(with: answerData)) as? [String: Any] ?? [:])
        context.session?.observe(scope: call.scope, answer: answer)
        return answer
    }

    func withNamespace(_ request: [String: Any], rpc: String) throws -> [String: Any] {
        if let ns = request["namespace"] as? String, !ns.isEmpty { return request }
        guard let fallback = context.defaultNamespace, !fallback.isEmpty else {
            throw LoamsValues.invalidArgument("\(rpc) needs a namespace: pass one or give the client one", reason: "namespace_required")
        }
        var out = request
        out["namespace"] = fallback
        return out
    }

    /// Every item of a paged list, following `nextPageToken`.
    func paged(_ name: String, _ request: [String: Any], items: String, _ options: LoamsCallOptions?) throws -> [[String: Any]] {
        var out: [[String: Any]] = []
        var token = ""
        while true {
            var page = request
            if !token.isEmpty { page["pageToken"] = token }
            let answer = try invoke(name, page, options)
            out += (answer[items] as? [[String: Any]]) ?? []
            let next = answer["nextPageToken"] as? String ?? ""
            if next.isEmpty || next == token { return out }
            token = next
        }
    }
}

/// Any app call by its facade-index name over its request's proto3 JSON: what
/// `LoamsClient.call(_:request:options:)` goes through.
#if canImport(ObjectiveC)
@objcMembers
#endif
public final class LoamsRawModule: LoamsAppModule {
    public func call(_ name: String, request: [String: Any], options: LoamsCallOptions? = nil) throws -> [String: Any] {
        guard loamsAppCalls[name] != nil else { throw LoamsValues.invalidArgument("no app call \(name)") }
        return try invoke(name, request, options)
    }
}

/// `namespaces`: the container every collection and stream lives in.
#if canImport(ObjectiveC)
@objcMembers
#endif
public final class LoamsNamespaces: LoamsAppModule {
    public func create(_ namespace: String, options: LoamsCallOptions? = nil) throws -> [String: Any] {
        return try invoke("namespaces.create", ["namespace": namespace], options)
    }

    public func list(pageSize: Int = 0, pageToken: String? = nil, options: LoamsCallOptions? = nil) throws -> [String: Any] {
        return try invoke("namespaces.list", loamsCompact([("pageSize", pageSize > 0 ? pageSize : nil), ("pageToken", pageToken)]), options)
    }

    /// Every namespace, across pages.
    public func listAll(pageSize: Int = 0, options: LoamsCallOptions? = nil) throws -> [[String: Any]] {
        return try paged("namespaces.list", loamsCompact([("pageSize", pageSize > 0 ? pageSize : nil)]), items: "namespaces", options)
    }
}

/// `collections`: schema, aliases, versions and the hot tier.
#if canImport(ObjectiveC)
@objcMembers
#endif
public final class LoamsCollections: LoamsAppModule {
    public func create(_ name: String, schema: LoamsSchema, options: LoamsCallOptions? = nil) throws -> [String: Any] {
        return try invoke("collections.create", ["name": name, "schema": schema.toJSON()], options)
    }

    public func get(_ name: String, options: LoamsCallOptions? = nil) throws -> [String: Any] {
        return try invoke("collections.get", ["collection": name], options)
    }

    public func list(pageSize: Int = 0, pageToken: String? = nil, options: LoamsCallOptions? = nil) throws -> [String: Any] {
        return try invoke("collections.list", loamsCompact([("pageSize", pageSize > 0 ? pageSize : nil), ("pageToken", pageToken)]), options)
    }

    public func listAll(pageSize: Int = 0, options: LoamsCallOptions? = nil) throws -> [[String: Any]] {
        return try paged("collections.list", loamsCompact([("pageSize", pageSize > 0 ? pageSize : nil)]), items: "collections", options)
    }

    public func drop(_ name: String, options: LoamsCallOptions? = nil) throws -> [String: Any] {
        return try invoke("collections.drop", ["collection": name], options)
    }

    /// Additive schema change: the fields and vectors of `fields`.
    public func addFields(_ name: String, fields: LoamsSchema, options: LoamsCallOptions? = nil) throws -> [String: Any] {
        let schema = fields.toJSON()
        return try invoke("collections.add_fields", ["collection": name, "fields": schema["fields"]!, "vectors": schema["vectors"]!], options)
    }

    public func listVersions(_ name: String, options: LoamsCallOptions? = nil) throws -> [String: Any] {
        return try invoke("collections.list_versions", ["collection": name], options)
    }

    /// `[["create": ["alias": …, "collection": …]], ["delete": ["alias": …]]]`.
    public func updateAliases(_ actions: [[String: Any]], options: LoamsCallOptions? = nil) throws -> [String: Any] {
        return try invoke("collections.update_aliases", ["actions": actions], options)
    }

    public func scan(_ name: String, options: LoamsCallOptions? = nil) throws -> [String: Any] {
        return try invoke("collections.scan", ["collection": name], options)
    }

    public func setHot(_ name: String, vectors: Bool, text: Bool, fragments: Bool, options: LoamsCallOptions? = nil) throws -> [String: Any] {
        return try invoke("collections.set_hot", ["collection": name, "hot": ["vectors": vectors, "text": text, "fragments": fragments]], options)
    }

    public func warm(_ name: String, options: LoamsCallOptions? = nil) throws -> [String: Any] {
        return try invoke("collections.warm", ["collection": name], options)
    }
}

/// One document to write: `id` (an integer, a string or `["uuid": …]`), its
/// `source` and its named `vectors`.
#if canImport(ObjectiveC)
@objcMembers
#endif
public final class LoamsDocument: NSObject {
    public let id: Any
    public let source: [String: Any]
    public let vectors: [String: [Double]]

    public init(id: Any, source: [String: Any] = [:], vectors: [String: [Double]] = [:]) {
        self.id = id
        self.source = source
        self.vectors = vectors
        super.init()
    }

    public func toJSON() throws -> [String: Any] {
        var out: [String: Any] = ["id": try LoamsValues.documentId(id), "source": source]
        if !vectors.isEmpty { out["vectors"] = vectors }
        return out
    }
}

/// What a `writeBatch` applied: the merged consistency token, the chunks and the rows.
#if canImport(ObjectiveC)
@objcMembers
#endif
public final class LoamsBatchResult: NSObject {
    public let token: String?
    public let chunks: Int
    public let rows: Int

    init(token: String?, chunks: Int, rows: Int) {
        self.token = token
        self.chunks = chunks
        self.rows = rows
        super.init()
    }
}

/// A `writeBatch` stopped at a failed chunk (R13): the chunks before it were
/// applied, and `token` is their merged consistency token.
#if canImport(ObjectiveC)
@objcMembers
#endif
public final class LoamsBatchError: LoamsError, @unchecked Sendable {
    public let chunksApplied: Int
    public let rowsApplied: Int
    public let token: String?
    public let metadata: [String: String]
    /// The failed chunk's own error.
    public let cause: LoamsError?

    init(cause: LoamsError, chunksApplied: Int, rowsApplied: Int, token: String?) {
        self.chunksApplied = chunksApplied
        self.rowsApplied = rowsApplied
        self.token = token
        var metadata = ["chunks_applied": String(chunksApplied), "rows_applied": String(rowsApplied)]
        if let reason = cause.reason { metadata["cause_reason"] = reason }
        self.metadata = metadata
        self.cause = cause
        super.init(message: "writeBatch stopped at chunk \(chunksApplied + 1): \(chunksApplied) chunks (\(rowsApplied) operations) were applied before it; \(cause.localizedDescription)",
                   codeString: "aborted",  // SDKP-23: the failed chunk's error is the cause
                   reason: "batch_partially_applied",
                   unknownReason: nil,
                   httpStatus: cause.httpStatus,
                   rawDetails: cause.rawDetails)
    }

    public required init?(coder: NSCoder) {
        self.chunksApplied = 0
        self.rowsApplied = 0
        self.token = nil
        self.metadata = [:]
        self.cause = nil
        super.init(coder: coder)
    }
}

/// `documents`: write, get, scroll, count and the filter writes.
#if canImport(ObjectiveC)
@objcMembers
#endif
public final class LoamsDocuments: LoamsAppModule {
    public static let defaultChunkRows = 10_000
    public static let defaultChunkBytes = 4 * 1024 * 1024

    /// Raw write ops (`WriteOp` JSON); the answer's `token` is the write's consistency token.
    public func write(_ collection: String, ops: [[String: Any]], options: LoamsCallOptions? = nil) throws -> [String: Any] {
        return try invoke("documents.write", ["collection": collection, "ops": ops], options)
    }

    public func upsert(_ collection: String, documents: [LoamsDocument], options: LoamsCallOptions? = nil) throws -> [String: Any] {
        return try write(collection, ops: documents.map { ["upsert": try $0.toJSON()] }, options: options)
    }

    public func delete(_ collection: String, ids: [Any], options: LoamsCallOptions? = nil) throws -> [String: Any] {
        return try write(collection, ops: ids.map { ["delete": ["id": try LoamsValues.documentId($0)]] }, options: options)
    }

    /// Sets the keys of `set` and removes the keys named in `unset`.
    public func patch(_ collection: String, id: Any, set: [String: Any]? = nil, unset: [String]? = nil,
                      options: LoamsCallOptions? = nil) throws -> [String: Any] {
        let patch = loamsCompact([("id", try LoamsValues.documentId(id)), ("source", set),
                                  ("deleteKeys", (unset?.isEmpty ?? true) ? nil : unset)])
        return try write(collection, ops: [["patch": patch]], options: options)
    }

    /// R13: `documents` in chunks, one in flight at a time, each with its own
    /// idempotency key (reused on that chunk's retries). Throws `LoamsBatchError`.
    public func writeBatch(_ collection: String, documents: [LoamsDocument], chunkRows: Int = LoamsDocuments.defaultChunkRows,
                           chunkBytes: Int = LoamsDocuments.defaultChunkBytes, options: LoamsCallOptions? = nil) throws -> LoamsBatchResult {
        guard chunkRows > 0, chunkBytes > 0 else { throw LoamsValues.invalidArgument("chunkRows and chunkBytes must be positive") }
        var token: LoamsConsistencyToken?
        var applied = 0
        var rows = 0
        var chunk: [[String: Any]] = []
        var size = 0
        func flush() throws {
            guard !chunk.isEmpty else { return }
            let call = LoamsCallOptions(namespace: options?.namespace)
            call.timeout = options?.timeout ?? -1
            call.headers = options?.headers ?? [:]
            call.idempotencyKey = LoamsIdempotency.mintKey()
            let answer: [String: Any]
            do {
                answer = try write(collection, ops: chunk, options: call)
            } catch let error as LoamsError {
                throw LoamsBatchError(cause: error, chunksApplied: applied, rowsApplied: rows, token: token?.stringValue)
            }
            if let text = answer["token"] as? String, let parsed = try? LoamsConsistencyToken.parse(text) {
                token = token.map { $0.merge(parsed) } ?? parsed.normalized()
            }
            applied += 1
            rows += chunk.count
            chunk = []
            size = 0
        }
        for doc in documents {
            let op: [String: Any] = ["upsert": try doc.toJSON()]
            let bytes = try JSONSerialization.data(withJSONObject: op).count
            if !chunk.isEmpty && (chunk.count >= chunkRows || size + bytes > chunkBytes) {
                try flush()
            }
            chunk.append(op)
            size += bytes
        }
        try flush()
        return LoamsBatchResult(token: token?.stringValue, chunks: applied, rows: rows)
    }

    /// `consistency` is `strong`, `eventual` or a token; `select` a projection.
    public func get(_ collection: String, ids: [Any], select: [String: Any]? = nil, consistency: String? = nil,
                    options: LoamsCallOptions? = nil) throws -> [String: Any] {
        return try invoke("documents.get", loamsCompact([
            ("collection", collection), ("ids", try ids.map { try LoamsValues.documentId($0) }),
            ("select", select), ("consistency", try loamsConsistencyJSON(consistency)),
        ]), options)
    }

    /// One page. `where` is a typed filter; `filter` the server's native filter JSON.
    public func scroll(_ collection: String, where: LoamsFilter? = nil, filter: Any? = nil, after: [String: Any]? = nil,
                       limit: Int = 0, consistency: String? = nil, options: LoamsCallOptions? = nil) throws -> [String: Any] {
        return try invoke("documents.scroll", loamsCompact([
            ("collection", collection), ("where", `where`?.json), ("filter", filter), ("after", after),
            ("limit", limit > 0 ? limit : nil), ("consistency", try loamsConsistencyJSON(consistency)),
        ]), options)
    }

    /// Every document, page after page until the answer has no `next`.
    public func scrollAll(_ collection: String, where: LoamsFilter? = nil, filter: Any? = nil, limit: Int = 0,
                          options: LoamsCallOptions? = nil) throws -> [[String: Any]] {
        var out: [[String: Any]] = []
        var after: [String: Any]?
        while true {
            let page = try scroll(collection, where: `where`, filter: filter, after: after, limit: limit, options: options)
            out += (page["documents"] as? [[String: Any]]) ?? []
            guard let next = page["next"] as? [String: Any], !next.isEmpty else { return out }
            after = next
        }
    }

    public func count(_ collection: String, where: LoamsFilter? = nil, filter: Any? = nil, consistency: String? = nil,
                      options: LoamsCallOptions? = nil) throws -> [String: Any] {
        return try invoke("documents.count", loamsCompact([
            ("collection", collection), ("where", `where`?.json), ("filter", filter),
            ("consistency", try loamsConsistencyJSON(consistency)),
        ]), options)
    }

    public func deleteByFilter(_ collection: String, where: LoamsFilter? = nil, filter: Any? = nil,
                               options: LoamsCallOptions? = nil) throws -> [String: Any] {
        return try invoke("documents.delete_by_filter", loamsCompact([
            ("collection", collection), ("where", `where`?.json), ("filter", filter),
        ]), options)
    }

    public func patchByFilter(_ collection: String, patch: [String: Any], where: LoamsFilter? = nil, filter: Any? = nil,
                              options: LoamsCallOptions? = nil) throws -> [String: Any] {
        return try invoke("documents.patch_by_filter", loamsCompact([
            ("collection", collection), ("patch", patch), ("where", `where`?.json), ("filter", filter),
        ]), options)
    }
}

/// `search`: text, vector, sparse or hybrid, with filters, fusion, highlights,
/// `select` and pagination.
#if canImport(ObjectiveC)
@objcMembers
#endif
public final class LoamsSearchModule: LoamsAppModule {
    /// The full request, checked (`LoamsSearch.build`) and sent as built.
    public func query(_ request: LoamsSearch, options: LoamsCallOptions? = nil) throws -> [String: Any] {
        return try queryJSON(request.json, options: options)
    }

    /// The same over the `SearchRequest`'s JSON.
    public func queryJSON(_ request: [String: Any], options: LoamsCallOptions? = nil) throws -> [String: Any] {
        return try invoke("search.query", try LoamsSearch.wire(request), options)
    }

    /// A text search: a match on one field, a multi-match on several, or a query string.
    public func text(_ collection: String, text: String, fields: [String]? = nil, filter: LoamsFilter? = nil,
                     limit: Int = 0, options: LoamsCallOptions? = nil) throws -> [String: Any] {
        var s = LoamsSearch.on(collection).text(text, fields: fields)
        if let filter = filter { s = s.filter(filter) }
        if limit > 0 { s = s.limit(limit) }
        return try query(s, options: options)
    }

    /// Text plus a vector on `field`, fused (`LoamsFusion.rrf()` by default).
    public func hybrid(_ collection: String, text: String, vector: [Double], field: String, fields: [String]? = nil,
                       fusion: [String: Any]? = nil, filter: LoamsFilter? = nil, limit: Int = 0,
                       options: LoamsCallOptions? = nil) throws -> [String: Any] {
        var s = LoamsSearch.on(collection).text(text, fields: fields).vector(field, values: vector).fuse(fusion ?? LoamsFusion.rrf())
        if let filter = filter { s = s.filter(filter) }
        if limit > 0 { s = s.limit(limit) }
        return try query(s, options: options)
    }

    /// Every hit, page after page through `searchAfter`, until a page has fewer than
    /// `limit` hits. The sort must end with `LoamsSort.pk()`, or it throws before any
    /// network call.
    public func queryAll(_ request: LoamsSearch, options: LoamsCallOptions? = nil) throws -> [[String: Any]] {
        guard request.hasTiebreak else {
            throw LoamsValues.invalidArgument("queryAll needs a sort that ends with the primary key, such as sort([\"score\", \"pk\"])",
                                              reason: "search_after_needs_tiebreak")
        }
        var page = request.json
        if page["limit"] == nil { page["limit"] = LoamsSearch.defaultK }
        let limit = page["limit"] as? Int ?? LoamsSearch.defaultK
        var out: [[String: Any]] = []
        while true {
            let answer = try queryJSON(page, options: options)
            let hits = (answer["hits"] as? [[String: Any]]) ?? []
            out += hits
            guard let last = hits.last, hits.count >= limit else { return out }
            page["searchAfter"] = last["sortValues"] ?? [Any]()
        }
    }

}

/// `vector`: the same RPC as `search.query`, with the vector shape as its required arguments.
#if canImport(ObjectiveC)
@objcMembers
#endif
public final class LoamsVectorModule: LoamsAppModule {
    public func search(_ collection: String, vector: [Double], field: String, limit: Int = 0,
                       filter: LoamsFilter? = nil, options: LoamsCallOptions? = nil) throws -> [String: Any] {
        var s = LoamsSearch.on(collection).vector(field, values: vector)
        if let filter = filter { s = s.filter(filter) }
        if limit > 0 { s = s.limit(limit) }
        return try invoke("vector.search", try LoamsSearch.wire(s.json), options)
    }
}

/// `graph`: stored property graphs, queried with GQL. **Experimental**: the
/// package is `unstable`.
#if canImport(ObjectiveC)
@objcMembers
#endif
public final class LoamsGraphModule: LoamsAppModule {
    public var unstable: Bool { return true }

    public func create(_ name: String, options: LoamsCallOptions? = nil) throws -> [String: Any] {
        return try invoke("graph.create", ["name": name], options)
    }

    public func get(_ name: String, options: LoamsCallOptions? = nil) throws -> [String: Any] {
        return try invoke("graph.get", ["name": name], options)
    }

    public func listAll(options: LoamsCallOptions? = nil) throws -> [[String: Any]] {
        return try paged("graph.list", [:], items: "graphs", options)
    }

    /// `graph` is the `Graph` JSON (`name`, and `namespace` or the client's).
    public func update(_ graph: [String: Any], updateMask: [String], options: LoamsCallOptions? = nil) throws -> [String: Any] {
        return try invoke("graph.update", ["graph": try withNamespace(graph, rpc: loamsAppCalls["graph.update"]!.rpc),
                                           "updateMask": updateMask.joined(separator: ",")], options)
    }

    /// Answers an `Operation`.
    public func delete(_ name: String, options: LoamsCallOptions? = nil) throws -> [String: Any] {
        return try invoke("graph.delete", ["name": name], options)
    }

    public func schema(_ name: String, options: LoamsCallOptions? = nil) throws -> [String: Any] {
        return try invoke("graph.schema", ["name": name], options)
    }

    /// One GQL statement with `parameters`. A read-only statement is retried as a
    /// read; a write carries an idempotency key.
    public func query(_ graph: String, statement: String, parameters: [String: Any] = [:], readOnly: Bool = false,
                      options: LoamsCallOptions? = nil) throws -> [String: Any] {
        var params: [String: Any] = [:]
        for (k, v) in parameters { params[k] = try LoamsValues.graphValue(v) }
        let request: [String: Any] = ["graph": graph, "statement": statement, "language": "QUERY_LANGUAGE_GQL",
                                      "parameters": params, "readOnly": readOnly]
        return try invoke("graph.query", request, options, retry: readOnly ? "safe" : nil)
    }

    /// `statements`: `[["statement": …, "parameters": [...]]]`, in one batch.
    public func queryBatch(_ graph: String, statements: [[String: Any]], options: LoamsCallOptions? = nil) throws -> [String: Any] {
        let list: [[String: Any]] = try statements.map { s in
            var params: [String: Any] = [:]
            for (k, v) in (s["parameters"] as? [String: Any]) ?? [:] { params[k] = try LoamsValues.graphValue(v) }
            return ["statement": s["statement"] ?? "", "parameters": params]
        }
        return try invoke("graph.query_batch", ["graph": graph, "statements": list, "atomic": true,
                                                "language": "QUERY_LANGUAGE_GQL"], options)
    }

    public func explain(_ graph: String, statement: String, options: LoamsCallOptions? = nil) throws -> [String: Any] {
        return try invoke("graph.explain", ["graph": graph, "statement": statement, "language": "QUERY_LANGUAGE_GQL"], options)
    }

    /// An `ExecuteResponse`'s rows, column name to value.
    public static func records(_ response: [String: Any]) -> [[String: Any]] {
        let rows = response["rows"] as? [String: Any] ?? [:]
        let columns = rows["columns"] as? [String] ?? []
        return ((rows["rows"] as? [[String: Any]]) ?? []).map { row in
            let values = (row["values"] as? [[String: Any]]) ?? []
            var record: [String: Any] = [:]
            for (i, column) in columns.enumerated() {
                record[column] = i < values.count ? LoamsValues.fromGraphValue(values[i]) : NSNull()
            }
            return record
        }
    }
}

/// `client.collection("notes")` (SDKP §2.8): every document and search call with
/// the collection (and namespace) filled in. It holds no state of its own.
#if canImport(ObjectiveC)
@objcMembers
#endif
public final class LoamsCollectionHandle: NSObject {
    private unowned let client: LoamsClient
    public let name: String
    public let namespace: String?

    init(client: LoamsClient, name: String, namespace: String?) {
        self.client = client
        self.name = name
        self.namespace = namespace
        super.init()
    }

    private func with(_ options: LoamsCallOptions?) -> LoamsCallOptions? {
        guard let ns = namespace else { return options }
        let out = LoamsCallOptions(namespace: options?.namespace ?? ns)
        out.timeout = options?.timeout ?? -1
        out.idempotencyKey = options?.idempotencyKey
        out.headers = options?.headers ?? [:]
        return out
    }

    public func upsert(_ documents: [LoamsDocument], options: LoamsCallOptions? = nil) throws -> [String: Any] {
        return try client.documents.upsert(name, documents: documents, options: with(options))
    }

    public func get(_ ids: [Any], consistency: String? = nil, options: LoamsCallOptions? = nil) throws -> [String: Any] {
        return try client.documents.get(name, ids: ids, consistency: consistency, options: with(options))
    }

    public func delete(_ ids: [Any], options: LoamsCallOptions? = nil) throws -> [String: Any] {
        return try client.documents.delete(name, ids: ids, options: with(options))
    }

    public func patch(_ id: Any, set: [String: Any]? = nil, unset: [String]? = nil, options: LoamsCallOptions? = nil) throws -> [String: Any] {
        return try client.documents.patch(name, id: id, set: set, unset: unset, options: with(options))
    }

    public func writeBatch(_ documents: [LoamsDocument], chunkRows: Int = LoamsDocuments.defaultChunkRows,
                           options: LoamsCallOptions? = nil) throws -> LoamsBatchResult {
        return try client.documents.writeBatch(name, documents: documents, chunkRows: chunkRows, options: with(options))
    }

    public func count(where: LoamsFilter? = nil, filter: Any? = nil, consistency: String? = nil,
                      options: LoamsCallOptions? = nil) throws -> [String: Any] {
        return try client.documents.count(name, where: `where`, filter: filter, consistency: consistency, options: with(options))
    }

    public func scrollAll(limit: Int = 0, options: LoamsCallOptions? = nil) throws -> [[String: Any]] {
        return try client.documents.scrollAll(name, limit: limit, options: with(options))
    }

    /// `search`'s collection is set to this one.
    public func search(_ search: LoamsSearch, options: LoamsCallOptions? = nil) throws -> [String: Any] {
        return try client.search.query(search.collection(name), options: with(options))
    }

    public func vectorSearch(_ vector: [Double], field: String, limit: Int = 0, options: LoamsCallOptions? = nil) throws -> [String: Any] {
        return try client.vector.search(name, vector: vector, field: field, limit: limit, options: with(options))
    }

    public func drop(options: LoamsCallOptions? = nil) throws -> [String: Any] {
        return try client.collections.drop(name, options: with(options))
    }

    public func info(options: LoamsCallOptions? = nil) throws -> [String: Any] {
        return try client.collections.get(name, options: with(options))
    }
}

/// Stops a `LoamsSubscription`'s callback loop.
#if canImport(ObjectiveC)
@objcMembers
#endif
public final class LoamsCancelToken: NSObject, @unchecked Sendable {
    private let lock = NSLock()
    private var flag = false

    public var isCancelled: Bool {
        lock.lock()
        defer { lock.unlock() }
        return flag
    }

    public func cancel() {
        lock.lock()
        flag = true
        lock.unlock()
    }
}

/// R14: `streams.subscribe`, a loop over unary `Fetch` (SDKP-2).
///
/// `next()` is the pull reader: the next record, fetching when the buffer is
/// empty, or nil once cancelled. The next offset is the last record's offset + 1.
/// An empty fetch backs off 100 ms, doubling up to 1 s. `offset` is where the next
/// fetch starts, so a caller can resume. `start(onRecord:)` runs the loop on a
/// background queue and calls the block per record until the token is cancelled.
#if canImport(ObjectiveC)
@objcMembers
#endif
public final class LoamsSubscription: NSObject, @unchecked Sendable {
    public static let initialBackoff: TimeInterval = 0.1
    public static let maxBackoff: TimeInterval = 1.0

    private let fetch: (UInt64) -> [[String: Any]]
    private let sleep: (TimeInterval) -> Void
    private var buffer: [[String: Any]] = []
    public private(set) var offset: UInt64
    public let cancelToken = LoamsCancelToken()

    /// `fetch` answers the records at and after an offset, each with an `offset`.
    public init(fromOffset: UInt64, fetch: @escaping (UInt64) -> [[String: Any]],
                sleep: ((TimeInterval) -> Void)? = nil) {
        self.offset = fromOffset
        self.fetch = fetch
        self.sleep = sleep ?? { Thread.sleep(forTimeInterval: $0) }
        super.init()
    }

    public func cancel() { cancelToken.cancel() }

    public func next() -> [String: Any]? {
        var backoff = LoamsSubscription.initialBackoff
        while !cancelToken.isCancelled {
            if !buffer.isEmpty {
                let record = buffer.removeFirst()
                offset = LoamsSubscription.offsetOf(record) + 1
                return record
            }
            let records = fetch(offset)
            if records.isEmpty {
                sleep(backoff)
                backoff = min(backoff * 2, LoamsSubscription.maxBackoff)
                continue
            }
            backoff = LoamsSubscription.initialBackoff
            buffer = records
        }
        return nil
    }

    /// Runs the loop on a background queue; `onRecord` is called per record.
    public func start(onRecord: @escaping @Sendable ([String: Any]) -> Void) -> LoamsCancelToken {
        DispatchQueue.global().async {
            while let record = self.next() {
                onRecord(record)
            }
        }
        return cancelToken
    }

    private static func offsetOf(_ record: [String: Any]) -> UInt64 {
        switch record["offset"] {
        case let i as Int: return UInt64(i)
        case let u as UInt64: return u
        case let s as String: return UInt64(s) ?? 0
        case let n as NSNumber: return n.uint64Value
        default: return 0
        }
    }
}
