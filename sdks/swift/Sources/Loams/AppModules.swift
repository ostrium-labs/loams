// The app-building modules (SDKP §2.2–§2.5): namespaces, collections,
// documents, search, vector and graph. Hand-written under SDKP-4; every method
// resolves its binding by name and names no RPC path or retry class.
//
// Requests and responses are `DynamicMessage`s in proto3 JSON; the builders
// (`Schema`, `Filter`, `Search`) produce the request shapes. A request with no
// `namespace` gets the client's default (R11); `CallOptions.namespace`
// overrides it per call.

import Foundation

private func loamsAppCall(
    _ invoker: CallInvoker,
    _ module: String,
    _ name: String,
    _ type: String,
    _ fields: [String: JSONValue],
    _ options: CallOptions
) async throws -> DynamicMessage {
    let binding = try loamsRequireBinding(module: module, call: name)
    return try await invoker.unary(
        binding: binding,
        request: DynamicMessage(protoTypeName: type, fields: fields),
        overrides: options
    )
}

/// Follows `next_page_token` to the end, yielding the items of `field`.
private func loamsListAll(
    _ fetch: @escaping @Sendable (String) async throws -> DynamicMessage,
    items field: String
) -> AsyncThrowingStream<JSONValue, Error> {
    AsyncThrowingStream { continuation in
        let task = Task {
            do {
                var token = ""
                for _ in 0..<100_000 {
                    let page = try await fetch(token)
                    if case .array(let items)? = page.fields[field] {
                        for item in items { continuation.yield(item) }
                    }
                    let next = page.fields["nextPageToken"]?.stringValue ?? page.fields["next_page_token"]?.stringValue ?? ""
                    if next.isEmpty { break }
                    token = next
                }
                continuation.finish()
            } catch {
                continuation.finish(throwing: error)
            }
        }
        continuation.onTermination = { _ in task.cancel() }
    }
}

/// `loams.namespaces`: the container every collection and stream lives in.
public struct NamespacesModule: Sendable {
    let invoker: CallInvoker

    /// Creates a namespace. A taken name is `already_exists`. Retry-safe.
    public func create(_ name: String, options: CallOptions = .init()) async throws -> DynamicMessage {
        try await loamsAppCall(invoker, "namespaces", "Create", "loams.collection.v1.CreateNamespaceRequest",
                       ["namespace": .string(name)], options)
    }

    /// One page of namespaces.
    public func list(pageSize: Int = 0, pageToken: String = "", options: CallOptions = .init()) async throws -> DynamicMessage {
        var fields: [String: JSONValue] = [:]
        if pageSize > 0 { fields["pageSize"] = .int(pageSize) }
        if !pageToken.isEmpty { fields["pageToken"] = .string(pageToken) }
        return try await loamsAppCall(invoker, "namespaces", "List", "loams.collection.v1.ListNamespacesRequest", fields, options)
    }

    /// Every namespace, following the page tokens.
    public func listAll(pageSize: Int = 0, options: CallOptions = .init()) -> AsyncThrowingStream<JSONValue, Error> {
        let module = self
        return loamsListAll({ try await module.list(pageSize: pageSize, pageToken: $0, options: options) }, items: "namespaces")
    }
}

/// `loams.collections`: collections and their schemas.
public struct CollectionsModule: Sendable {
    let invoker: CallInvoker

    /// Creates a collection with a schema from ``Schema``. Idempotent by name.
    public func create(_ name: String, schema: Schema, partitions: Int? = nil, namespace: String? = nil,
                       options: CallOptions = .init()) async throws -> DynamicMessage {
        var fields: [String: JSONValue] = ["name": .string(name), "schema": schema.json]
        if let partitions { fields["partitions"] = .int(partitions) }
        if let namespace { fields["namespace"] = .string(namespace) }
        return try await loamsAppCall(invoker, "collections", "Create", "loams.collection.v1.CreateCollectionRequest", fields, options)
    }

    /// One page of the namespace's collections.
    public func list(pageSize: Int = 0, pageToken: String = "", namespace: String? = nil,
                     options: CallOptions = .init()) async throws -> DynamicMessage {
        var fields: [String: JSONValue] = [:]
        if pageSize > 0 { fields["pageSize"] = .int(pageSize) }
        if !pageToken.isEmpty { fields["pageToken"] = .string(pageToken) }
        if let namespace { fields["namespace"] = .string(namespace) }
        return try await loamsAppCall(invoker, "collections", "List", "loams.collection.v1.ListCollectionsRequest", fields, options)
    }

    /// Every collection in the namespace, following the page tokens.
    public func listAll(pageSize: Int = 0, namespace: String? = nil,
                        options: CallOptions = .init()) -> AsyncThrowingStream<JSONValue, Error> {
        let module = self
        return loamsListAll({
            try await module.list(pageSize: pageSize, pageToken: $0, namespace: namespace, options: options)
        }, items: "collections")
    }

    /// One collection, by name or alias.
    public func get(_ collection: String, namespace: String? = nil, options: CallOptions = .init()) async throws -> DynamicMessage {
        var fields: [String: JSONValue] = ["collection": .string(collection)]
        if let namespace { fields["namespace"] = .string(namespace) }
        return try await loamsAppCall(invoker, "collections", "Get", "loams.collection.v1.GetCollectionRequest", fields, options)
    }

    /// Drops a collection.
    public func drop(_ collection: String, namespace: String? = nil, options: CallOptions = .init()) async throws -> DynamicMessage {
        var fields: [String: JSONValue] = ["collection": .string(collection)]
        if let namespace { fields["namespace"] = .string(namespace) }
        return try await loamsAppCall(invoker, "collections", "Drop", "loams.collection.v1.DropCollectionRequest", fields, options)
    }

    /// Any collection call by its facade name (`AddFields`, `ListVersions`,
    /// `UpdateAliases`, `Scan`, `SetHot`, `Warm`), with the request's fields.
    public func call(_ name: String, _ fields: [String: JSONValue], options: CallOptions = .init()) async throws -> DynamicMessage {
        try await loamsAppCall(invoker, "collections", name, "", fields, options)
    }
}

/// One document to upsert: an id, its source, and its dense vectors.
public struct Doc: Sendable, Equatable {
    public let id: DocumentID
    public let source: [String: JSONValue]
    public let vectors: [String: [Double]]

    public init(_ id: DocumentID, _ source: [String: JSONValue], vectors: [String: [Double]] = [:]) {
        self.id = id
        self.source = source
        self.vectors = vectors
    }

    /// The write op that upserts this document.
    public var op: JSONValue {
        var upsert: [String: JSONValue] = ["id": id.json, "source": .object(source)]
        if !vectors.isEmpty {
            upsert["vectors"] = .object(vectors.mapValues { .array($0.map { .number($0) }) })
        }
        return .object(["upsert": .object(upsert)])
    }
}

/// `loams.documents`: write, read, scroll and count documents.
public struct DocumentsModule: Sendable {
    let invoker: CallInvoker

    private func write(_ collection: String, _ ops: [JSONValue], namespace: String?, options: CallOptions)
        async throws -> DynamicMessage {
        var fields: [String: JSONValue] = ["collection": .string(collection), "ops": .array(ops)]
        if let namespace { fields["namespace"] = .string(namespace) }
        return try await loamsAppCall(invoker, "documents", "Write", "loams.collection.v1.WriteDocumentsRequest", fields, options)
    }

    /// Raw write ops, as one write: keyed, the key reused on every retry (R3).
    public func write(_ collection: String, ops: [JSONValue], namespace: String? = nil,
                      options: CallOptions = .init()) async throws -> DynamicMessage {
        try await write(collection, ops, namespace: namespace, options: options)
    }

    /// Upserts documents, as one write.
    public func upsert(_ collection: String, _ docs: [Doc], namespace: String? = nil,
                       options: CallOptions = .init()) async throws -> DynamicMessage {
        try await write(collection, docs.map(\.op), namespace: namespace, options: options)
    }

    /// Deletes documents by id, as one write.
    public func delete(_ collection: String, ids: [DocumentID], namespace: String? = nil,
                       options: CallOptions = .init()) async throws -> DynamicMessage {
        try await write(collection, ids.map { .object(["delete": .object(["id": $0.json])]) },
                        namespace: namespace, options: options)
    }

    /// Changes one document: `set` merges into its source and `unset` removes keys.
    public func patch(_ collection: String, id: DocumentID, set: [String: JSONValue]? = nil, unset: [String] = [],
                      mode: String? = nil, namespace: String? = nil, options: CallOptions = .init()) async throws -> DynamicMessage {
        var patch: [String: JSONValue] = ["id": id.json]
        if let set { patch["source"] = .object(set) }
        if !unset.isEmpty { patch["deleteKeys"] = .array(unset.map { .string($0) }) }
        if let mode { patch["mode"] = .string(mode) }
        return try await write(collection, [.object(["patch": .object(patch)])], namespace: namespace, options: options)
    }

    /// Writes any number of documents in chunks, one chunk in flight at a time (R13).
    public func writeBatch(_ collection: String, _ docs: [Doc], chunkRows: Int = loamsDefaultChunkRows,
                           chunkBytes: Int = loamsDefaultChunkBytes, namespace: String? = nil,
                           options: CallOptions = .init()) async throws -> BatchResult {
        try await loamsWriteBatch(docs.map(\.op), chunkRows: chunkRows, chunkBytes: chunkBytes) { chunk, key in
            var keyed = options
            keyed.idempotencyKey = key
            return try await self.write(collection, chunk, namespace: namespace, options: keyed)
        }
    }

    /// Documents by id.
    public func get(_ collection: String, ids: [DocumentID], consistency: Consistent? = nil,
                    namespace: String? = nil, options: CallOptions = .init()) async throws -> DynamicMessage {
        var fields: [String: JSONValue] = ["collection": .string(collection), "ids": .array(ids.map(\.json))]
        if let consistency { fields["consistency"] = consistency.json }
        if let namespace { fields["namespace"] = .string(namespace) }
        return try await loamsAppCall(invoker, "documents", "Get", "loams.collection.v1.GetDocumentsRequest", fields, options)
    }

    /// One page of documents in id order.
    public func scroll(_ fields: [String: JSONValue], options: CallOptions = .init()) async throws -> DynamicMessage {
        try await loamsAppCall(invoker, "documents", "Scroll", "loams.collection.v1.ScrollDocumentsRequest", fields, options)
    }

    /// Every document matching `where` (all when nil), in id order, following `next`.
    public func scrollAll(_ collection: String, where filter: Query? = nil, limit: Int? = nil,
                          namespace: String? = nil, options: CallOptions = .init()) -> AsyncThrowingStream<JSONValue, Error> {
        var first: [String: JSONValue] = ["collection": .string(collection)]
        if let filter { first["filter"] = Filter.ir(filter) }
        if let limit { first["limit"] = .int(limit) }
        if let namespace { first["namespace"] = .string(namespace) }
        let start = first
        let module = self
        return AsyncThrowingStream { continuation in
            let task = Task {
                do {
                    var request = start
                    for _ in 0..<100_000 {
                        let page = try await module.scroll(request, options: options)
                        if case .array(let docs)? = page.fields["documents"] {
                            for doc in docs { continuation.yield(doc) }
                        }
                        guard let next = page.fields["next"], next != .null else { break }
                        request["pageToken"] = next
                    }
                    continuation.finish()
                } catch {
                    continuation.finish(throwing: error)
                }
            }
            continuation.onTermination = { _ in task.cancel() }
        }
    }

    /// How many documents match `where` (all when nil).
    public func count(_ collection: String, where filter: Query? = nil, namespace: String? = nil,
                      options: CallOptions = .init()) async throws -> UInt64 {
        var fields: [String: JSONValue] = ["collection": .string(collection)]
        if let filter { fields["filter"] = Filter.ir(filter) }
        if let namespace { fields["namespace"] = .string(namespace) }
        let answer = try await loamsAppCall(invoker, "documents", "Count", "loams.collection.v1.CountDocumentsRequest", fields, options)
        switch answer.fields["count"] {
        case .string(let s)?: return UInt64(s) ?? 0
        case .int(let n)?: return UInt64(n)
        case .number(let d)?: return UInt64(d)
        default: return 0
        }
    }

    /// `DeleteByFilter` or `PatchByFilter`, with the request's fields. Never retried.
    public func call(_ name: String, _ fields: [String: JSONValue], options: CallOptions = .init()) async throws -> DynamicMessage {
        try await loamsAppCall(invoker, "documents", name, "", fields, options)
    }
}

/// `loams.search`: text, vector and hybrid search over one collection.
public struct SearchModule: Sendable {
    let invoker: CallInvoker

    /// One search, from the builder.
    public func query(_ search: Search, options: CallOptions = .init()) async throws -> DynamicMessage {
        try await query(search.json, options: options)
    }

    /// One search: the full request IR.
    public func query(_ fields: [String: JSONValue], options: CallOptions = .init()) async throws -> DynamicMessage {
        try await loamsAppCall(invoker, "search", "Query", "loams.collection.v1.SearchRequest", fields, options)
    }

    /// A hybrid search: text on `textFields` plus a vector on `vectorField`, fused (RRF k=60 by default).
    public func hybrid(_ collection: String, text: String, textFields: [String], vectorField: String,
                       vector: [Double], fusion: Fusion = .rrf(), limit: Int? = nil,
                       options: CallOptions = .init()) async throws -> DynamicMessage {
        var search = Search.on(collection).text(text, fields: textFields).vector(vectorField, vector).fuse(fusion)
        if let limit { search = search.limit(limit) }
        return try await query(search, options: options)
    }

    /// Every hit, page after page, following `searchAfter` from the last hit's
    /// sort values until a page comes back short. The search must sort with a
    /// primary-key tiebreaker (``Sort/pk(descending:)``); without one this throws
    /// `invalid_argument` before any call.
    public func queryAll(_ search: Search, options: CallOptions = .init()) throws -> AsyncThrowingStream<JSONValue, Error> {
        guard search.sortKeys.contains(where: \.isPk) else {
            throw LoamsError.loams(
                code: .invalidArgument, reason: .invalidArgument, unknownReason: "search_after_needs_tiebreak",
                metadata: ["field": "sort"], hint: "", rpc: LoamsAppService.query + "/Search",
                message: "queryAll pages with searchAfter, which needs a sort with a primary-key tiebreaker: "
                    + "add .pk() as the last sort key"
            )
        }
        let limit = search.limitValue ?? Search.defaultK
        let first = search.limit(limit)
        let module = self
        return AsyncThrowingStream { continuation in
            let task = Task {
                do {
                    var page = first
                    for _ in 0..<100_000 {
                        let answer = try await module.query(page, options: options)
                        guard case .array(let hits)? = answer.fields["hits"] else { break }
                        for hit in hits { continuation.yield(hit) }
                        guard hits.count >= limit, case .object(let last)? = hits.last,
                              case .array(let values)? = last["sortValues"] else { break }
                        page = page.after(values)
                    }
                    continuation.finish()
                } catch {
                    continuation.finish(throwing: error)
                }
            }
            continuation.onTermination = { _ in task.cancel() }
        }
    }
}

/// `loams.vector`: the search RPC with a vector's shape (Q613).
public struct VectorModule: Sendable {
    let invoker: CallInvoker

    /// The nearest neighbours of `vector` in `field`.
    public func search(_ collection: String, vector: [Double], field: String, limit: Int? = nil,
                       filter: Query? = nil, namespace: String? = nil, options: CallOptions = .init()) async throws -> DynamicMessage {
        var search = Search.on(collection).vector(field, vector)
        if let limit { search = search.limit(limit) }
        if let filter { search = search.filter(filter) }
        if let namespace { search = search.inNamespace(namespace) }
        return try await loamsAppCall(invoker, "vector", "Search", "loams.collection.v1.SearchRequest", search.json, options)
    }
}

/// `loams.graph`: stored property graphs, queried with GQL. **Experimental**:
/// the package is `unstable` (GR1e) and may change.
public struct GraphModule: Sendable {
    let invoker: CallInvoker

    /// Creates an owned graph. Keyed (R3).
    public func create(_ name: String, namespace: String? = nil, options: CallOptions = .init()) async throws -> DynamicMessage {
        var fields: [String: JSONValue] = ["name": .string(name)]
        if let namespace { fields["namespace"] = .string(namespace) }
        return try await loamsAppCall(invoker, "graph", "Create", "loams.graph.v1.CreateGraphRequest", fields, options)
    }

    public func get(_ name: String, options: CallOptions = .init()) async throws -> DynamicMessage {
        try await loamsAppCall(invoker, "graph", "Get", "loams.graph.v1.GetGraphRequest", ["name": .string(name)], options)
    }

    public func list(options: CallOptions = .init()) async throws -> DynamicMessage {
        try await loamsAppCall(invoker, "graph", "List", "loams.graph.v1.ListGraphsRequest", [:], options)
    }

    /// Deletes a graph; answers its operation. Never retried.
    public func delete(_ name: String, options: CallOptions = .init()) async throws -> DynamicMessage {
        try await loamsAppCall(invoker, "graph", "Delete", "loams.graph.v1.DeleteGraphRequest", ["name": .string(name)], options)
    }

    public func schema(_ name: String, options: CallOptions = .init()) async throws -> DynamicMessage {
        try await loamsAppCall(invoker, "graph", "Schema", "loams.graph.v1.GetSchemaRequest", ["name": .string(name)], options)
    }

    /// Runs one GQL statement. Read-only statements are retried freely and not
    /// keyed; writes are keyed (R3) and retried with their key.
    public func query(_ graph: String, _ statement: String, parameters: [String: JSONValue] = [:],
                      readOnly: Bool = false, namespace: String? = nil, options: CallOptions = .init()) async throws -> DynamicMessage {
        var fields: [String: JSONValue] = [
            "graph": .string(graph),
            "statement": .string(statement),
            "language": .string("QUERY_LANGUAGE_GQL"),
        ]
        if readOnly { fields["readOnly"] = .bool(true) }
        if !parameters.isEmpty { fields["parameters"] = .object(parameters) }
        if let namespace { fields["namespace"] = .string(namespace) }
        var settings = options
        if readOnly {
            settings.retrySafe = settings.retrySafe ?? true
            settings.mintIdempotencyKey = settings.mintIdempotencyKey ?? false
        }
        return try await loamsAppCall(invoker, "graph", "Query", "loams.graph.v1.ExecuteRequest", fields, settings)
    }

    /// Any graph call by its facade name (`Update`, `QueryBatch`, `Explain`), with the request's fields.
    public func call(_ name: String, _ fields: [String: JSONValue], options: CallOptions = .init()) async throws -> DynamicMessage {
        try await loamsAppCall(invoker, "graph", name, "", fields, options)
    }
}

/// GQL parameter values (`loams.graph.v1.Value`).
public enum GraphValue {
    public static func int(_ n: Int64) -> JSONValue { .object(["int64": .string(String(n))]) }
    public static func string(_ s: String) -> JSONValue { .object(["string": .string(s)]) }
    public static func bool(_ b: Bool) -> JSONValue { .object(["boolean": .bool(b)]) }
    public static func double(_ d: Double) -> JSONValue { .object(["float64": .number(d)]) }
    public static func vector(_ v: [Double]) -> JSONValue { .object(["vector": .object(["values": .array(v.map { .number($0) })])]) }
}
