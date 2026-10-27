// The collection handle (SDKP §2.8): `client.collection("notes")` returns calls
// with the collection (and namespace) filled in. It holds no state of its own.

import Foundation

public struct CollectionHandle: Sendable {
    let client: Loams
    public let name: String
    public let namespace: String?

    public func upsert(_ docs: [Doc], options: CallOptions = .init()) async throws -> DynamicMessage {
        try await client.documents.upsert(name, docs, namespace: namespace, options: options)
    }

    public func get(_ ids: [DocumentID], consistency: Consistent? = nil, options: CallOptions = .init()) async throws -> DynamicMessage {
        try await client.documents.get(name, ids: ids, consistency: consistency, namespace: namespace, options: options)
    }

    public func delete(_ ids: [DocumentID], options: CallOptions = .init()) async throws -> DynamicMessage {
        try await client.documents.delete(name, ids: ids, namespace: namespace, options: options)
    }

    public func patch(_ id: DocumentID, set: [String: JSONValue]? = nil, unset: [String] = [],
                      options: CallOptions = .init()) async throws -> DynamicMessage {
        try await client.documents.patch(name, id: id, set: set, unset: unset, namespace: namespace, options: options)
    }

    public func writeBatch(_ docs: [Doc], chunkRows: Int = loamsDefaultChunkRows,
                           options: CallOptions = .init()) async throws -> BatchResult {
        try await client.documents.writeBatch(name, docs, chunkRows: chunkRows, namespace: namespace, options: options)
    }

    public func count(where filter: Query? = nil, options: CallOptions = .init()) async throws -> UInt64 {
        try await client.documents.count(name, where: filter, namespace: namespace, options: options)
    }

    public func scrollAll(where filter: Query? = nil, limit: Int? = nil,
                          options: CallOptions = .init()) -> AsyncThrowingStream<JSONValue, Error> {
        client.documents.scrollAll(name, where: filter, limit: limit, namespace: namespace, options: options)
    }

    /// A search over this collection: the builder starts on it.
    public func search(_ build: (Search) -> Search, options: CallOptions = .init()) async throws -> DynamicMessage {
        var search = build(Search.on(name))
        if let namespace { search = search.inNamespace(namespace) }
        return try await client.search.query(search, options: options)
    }

    public func vectorSearch(_ vector: [Double], field: String, limit: Int? = nil,
                             options: CallOptions = .init()) async throws -> DynamicMessage {
        try await client.vector.search(name, vector: vector, field: field, limit: limit, namespace: namespace, options: options)
    }

    public func drop(options: CallOptions = .init()) async throws -> DynamicMessage {
        try await client.collections.drop(name, namespace: namespace, options: options)
    }

    /// The collection's info (`collections.get`).
    public func info(options: CallOptions = .init()) async throws -> DynamicMessage {
        try await client.collections.get(name, namespace: namespace, options: options)
    }
}
