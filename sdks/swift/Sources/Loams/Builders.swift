// The Schema, Filter and Search builders (SDKP §2.4, §2.6): the same verbs in
// every SDK, each a pure function onto the request's proto3 JSON.
//
//     let request = Search.on("notes")
//         .text("refund", fields: ["body"])
//         .vector("embedding", [0.1, 0.2, 0.3, 0.4])
//         .fuse(.rrf(k: 60))
//         .filter(Filter.eq("tags", "policy"))
//         .limit(10)
//
// Every builder is a value type and `Sendable`.

import Foundation

// MARK: - Values

/// A Swift value as proto3 JSON (`google.protobuf.Value` and friends).
public protocol JSONConvertible: Sendable {
    var json: JSONValue { get }
}

extension JSONValue: JSONConvertible { public var json: JSONValue { self } }
extension String: JSONConvertible { public var json: JSONValue { .string(self) } }
extension Bool: JSONConvertible { public var json: JSONValue { .bool(self) } }
extension Int: JSONConvertible { public var json: JSONValue { .int(self) } }
extension Int64: JSONConvertible { public var json: JSONValue { .int(Int(self)) } }
extension Double: JSONConvertible { public var json: JSONValue { .number(self) } }
extension Float: JSONConvertible { public var json: JSONValue { .number(Double(self)) } }
extension Array: JSONConvertible where Element: JSONConvertible {
    public var json: JSONValue { .array(map(\.json)) }
}
extension Dictionary: JSONConvertible where Key == String, Value: JSONConvertible {
    public var json: JSONValue { .object(mapValues(\.json)) }
}

/// A document id: an unsigned 64-bit integer, a string or a UUID.
public enum DocumentID: Sendable, Equatable, Hashable, ExpressibleByIntegerLiteral, ExpressibleByStringLiteral {
    case uint(UInt64)
    case string(String)
    case uuid(String)

    public init(integerLiteral value: UInt64) { self = .uint(value) }
    public init(stringLiteral value: String) { self = .string(value) }

    /// The proto3 JSON of `loams.collection.v1.DocumentId` (a `uint64` is a string).
    public var json: JSONValue {
        switch self {
        case .uint(let n): return .object(["uint": .string(String(n))])
        case .string(let s): return .object(["string": .string(s)])
        case .uuid(let u): return .object(["uuid": .string(u)])
        }
    }

    /// The form `Query.ids` carries: `{"uint": "1"}` (exact past 2^53, and the builder
    /// corpus's spelling), a bare string, or `{"uuid": …}`.
    var bare: JSONValue {
        switch self {
        case .uint(let n): return .object(["uint": .string(String(n))])
        case .string(let s): return .string(s)
        case .uuid(let u): return .object(["uuid": .string(u)])
        }
    }

    /// The id from a response's `DocumentId` JSON.
    public init?(json: JSONValue?) {
        guard case .object(let fields)? = json else { return nil }
        if let n = fields["uint"] {
            switch n {
            case .string(let s): guard let v = UInt64(s) else { return nil }; self = .uint(v)
            case .int(let i): self = .uint(UInt64(i))
            case .number(let d): self = .uint(UInt64(d))
            default: return nil
            }
        } else if let s = fields["string"]?.stringValue {
            self = .string(s)
        } else if let u = fields["uuid"]?.stringValue {
            self = .uuid(u)
        } else {
            return nil
        }
    }

    /// The id as text.
    public var text: String {
        switch self {
        case .uint(let n): return String(n)
        case .string(let s), .uuid(let s): return s
        }
    }
}

// MARK: - Schema

/// Builds a collection schema: the `Struct` `CreateCollectionRequest.schema`
/// takes, in the shape the engine's schema parser reads.
public struct Schema: Sendable, Equatable {
    private var fields: [JSONValue] = []
    private var vectors: [JSONValue] = []
    private var sparse: [JSONValue] = []
    private var dynamicMode = "ignore"
    private var maxFieldCount = 1000
    private var annotations: [String: JSONValue] = [:]

    public init() {}

    private func field(_ name: String, kind: JSONValue, indexed: Bool, fast: Bool, sourcePath: String?) -> Schema {
        var copy = self
        copy.fields.append(.object([
            "name": .string(name),
            "source_path": .string(sourcePath ?? name),
            "kind": kind,
            "indexed": .bool(indexed),
            "fast": .bool(fast),
        ]))
        return copy
    }

    public func text(_ name: String, analyzer: String = "standard", positions: Bool = true,
                     indexed: Bool = true, fast: Bool = false, sourcePath: String? = nil) -> Schema {
        field(name, kind: .object(["text": .object(["analyzer": .string(analyzer), "positions": .bool(positions)])]),
              indexed: indexed, fast: fast, sourcePath: sourcePath)
    }

    public func keyword(_ name: String, indexed: Bool = true, fast: Bool = true, sourcePath: String? = nil) -> Schema {
        field(name, kind: .string("keyword"), indexed: indexed, fast: fast, sourcePath: sourcePath)
    }

    public func i64(_ name: String, indexed: Bool = true, fast: Bool = true, sourcePath: String? = nil) -> Schema {
        field(name, kind: .string("i64"), indexed: indexed, fast: fast, sourcePath: sourcePath)
    }

    public func f64(_ name: String, indexed: Bool = true, fast: Bool = true, sourcePath: String? = nil) -> Schema {
        field(name, kind: .string("f64"), indexed: indexed, fast: fast, sourcePath: sourcePath)
    }

    public func bool(_ name: String, indexed: Bool = true, fast: Bool = true, sourcePath: String? = nil) -> Schema {
        field(name, kind: .string("bool"), indexed: indexed, fast: fast, sourcePath: sourcePath)
    }

    /// A timestamp field (the engine's `date` kind).
    public func timestamp(_ name: String, indexed: Bool = true, fast: Bool = true, sourcePath: String? = nil) -> Schema {
        field(name, kind: .string("date"), indexed: indexed, fast: fast, sourcePath: sourcePath)
    }

    public func uuid(_ name: String, indexed: Bool = true, fast: Bool = true, sourcePath: String? = nil) -> Schema {
        field(name, kind: .string("uuid"), indexed: indexed, fast: fast, sourcePath: sourcePath)
    }

    public func jsonField(_ name: String, indexed: Bool = true, fast: Bool = false, sourcePath: String? = nil) -> Schema {
        field(name, kind: .string("json"), indexed: indexed, fast: fast, sourcePath: sourcePath)
    }

    public func vector(_ name: String, dim: Int, distance: String = "cosine") -> Schema {
        var copy = self
        copy.vectors.append(.object(["name": .string(name), "dim": .int(dim), "distance": .string(distance)]))
        return copy
    }

    public func sparseVector(_ name: String, modifier: String? = nil) -> Schema {
        var copy = self
        var spec: [String: JSONValue] = ["name": .string(name)]
        if let modifier { spec["modifier"] = .string(modifier) }
        copy.sparse.append(.object(spec))
        return copy
    }

    /// What happens to a source key no field names: `ignore`, `strict` or `map`.
    public func dynamic(_ mode: String) -> Schema {
        var copy = self
        copy.dynamicMode = mode
        return copy
    }

    public func maxFields(_ n: Int) -> Schema {
        var copy = self
        copy.maxFieldCount = n
        return copy
    }

    public func annotation(_ key: String, _ value: String) -> Schema {
        var copy = self
        copy.annotations[key] = .string(value)
        return copy
    }

    /// The schema as the request's `Struct`.
    public var json: JSONValue {
        var out: [String: JSONValue] = [
            "fields": .array(fields),
            "vectors": .array(vectors),
            "sparse_vectors": .array(sparse),
            "dynamic": .string(dynamicMode),
            "max_fields": .int(maxFieldCount),
        ]
        if !annotations.isEmpty { out["annotations"] = .object(annotations) }
        return .object(out)
    }
}

// MARK: - Filter

/// A `loams.collection.v1.Query`, as proto3 JSON. Build one with ``Filter``;
/// `&&`, `||` and `!` are optional sugar for `and`, `or` and `not`.
public struct Query: Sendable, Equatable {
    public let json: JSONValue

    public init(json: JSONValue) {
        self.json = json
    }

    public static func && (lhs: Query, rhs: Query) -> Query { Filter.and(lhs, rhs) }
    public static func || (lhs: Query, rhs: Query) -> Query { Filter.or(lhs, rhs) }
    public static prefix func ! (query: Query) -> Query { Filter.not(query) }
}

/// The filter builder: the same verbs in every SDK.
public enum Filter {
    private static func arm(_ name: String, _ body: [String: JSONValue]) -> Query {
        Query(json: .object([name: .object(body)]))
    }

    private static func bool(_ name: String, _ queries: [Query], extra: [String: JSONValue] = [:]) -> Query {
        var body = extra
        body[name] = .array(queries.map(\.json))
        return arm("bool", body)
    }

    public static func all() -> Query { Query(json: .object(["matchAll": .object([:])])) }
    public static func none() -> Query { Query(json: .object(["matchNone": .object([:])])) }

    /// The field equals the value exactly (`term`).
    public static func eq(_ field: String, _ value: some JSONConvertible) -> Query {
        arm("term", ["field": .string(field), "value": value.json])
    }

    /// The field equals one of the values (`terms`).
    public static func `in`(_ field: String, _ values: [any JSONConvertible]) -> Query {
        arm("terms", ["field": .string(field), "values": .array(values.map(\.json))])
    }

    /// The field is in a range; each bound left nil is open.
    public static func range(_ field: String, gt: (any JSONConvertible)? = nil, gte: (any JSONConvertible)? = nil,
                             lt: (any JSONConvertible)? = nil, lte: (any JSONConvertible)? = nil) -> Query {
        var body: [String: JSONValue] = ["field": .string(field)]
        if let gt { body["gt"] = gt.json }
        if let gte { body["gte"] = gte.json }
        if let lt { body["lt"] = lt.json }
        if let lte { body["lte"] = lte.json }
        return arm("range", body)
    }

    public static func exists(_ field: String) -> Query { arm("exists", ["field": .string(field)]) }
    public static func isNull(_ field: String) -> Query { arm("isNull", ["field": .string(field)]) }
    public static func isEmpty(_ field: String) -> Query { arm("isEmpty", ["field": .string(field)]) }

    public static func prefix(_ field: String, _ value: String) -> Query {
        arm("prefix", ["field": .string(field), "value": .string(value)])
    }

    public static func wildcard(_ field: String, _ pattern: String) -> Query {
        arm("wildcard", ["field": .string(field), "pattern": .string(pattern)])
    }

    /// Within `edits` edits of the value; nil means `auto`.
    public static func fuzzy(_ field: String, _ value: String, edits: Int? = nil) -> Query {
        let fuzziness: JSONValue = edits.map { .object(["edits": .int($0)]) } ?? .object(["auto": .object([:])])
        return arm("fuzzy", ["field": .string(field), "value": .string(value), "fuzziness": fuzziness])
    }

    /// A full-text match on one field (scored).
    public static func match(_ field: String, _ text: String, all: Bool = false) -> Query {
        var body: [String: JSONValue] = ["field": .string(field), "text": .string(text)]
        if all { body["operator"] = .string("and") }
        return arm("match", body)
    }

    public static func phrase(_ field: String, _ text: String, slop: Int = 0) -> Query {
        var body: [String: JSONValue] = ["field": .string(field), "text": .string(text)]
        if slop > 0 { body["slop"] = .int(slop) }
        return arm("matchPhrase", body)
    }

    public static func multiMatch(_ text: String, fields: [String]) -> Query {
        arm("multiMatch", ["text": .string(text), "fields": .array(fields.map { .object(["field": .string($0)]) })])
    }

    public static func queryString(_ query: String, defaultFields: [String] = []) -> Query {
        var body: [String: JSONValue] = ["query": .string(query)]
        if !defaultFields.isEmpty { body["defaultFields"] = .array(defaultFields.map { .string($0) }) }
        return arm("queryString", body)
    }

    public static func ids(_ ids: [DocumentID]) -> Query {
        Query(json: .object(["ids": .array(ids.map(\.bare))]))
    }

    public static func and(_ queries: Query...) -> Query { bool("must", queries) }
    public static func or(_ queries: Query...) -> Query { bool("should", queries) }
    public static func or(minimumShouldMatch: String, _ queries: Query...) -> Query {
        bool("should", queries, extra: ["minimumShouldMatch": .string(minimumShouldMatch)])
    }
    public static func not(_ queries: Query...) -> Query { bool("mustNot", queries) }
    /// All of the queries, without scoring (`bool.filter`; the plan's `filter` verb).
    public static func nonScoring(_ queries: Query...) -> Query { bool("filter", queries) }

    public static func boost(_ query: Query, _ boost: Double) -> Query {
        Query(json: .object(["boost": .object(["query": query.json, "boost": .number(boost)])]))
    }

    public static func constantScore(_ query: Query, _ score: Double) -> Query {
        Query(json: .object(["constantScore": .object(["query": query.json, "score": .number(score)])]))
    }

    // MARK: the native IR

    /// A filter in the engine's native JSON IR, as `ScrollDocuments`,
    /// `CountDocuments`, `DeleteByFilter` and `PatchByFilter` take it in their
    /// `filter` field today: snake_case keys, unit arms as bare strings, ids as
    /// bare keys. The mapping `connect_query_filters.rs` applies to a search
    /// filter. Retired when those requests gain a typed `where` (SDKP P0a).
    public static func ir(_ query: Query) -> JSONValue { irOf(query.json) }

    private static func snake(_ key: String) -> String {
        var out = ""
        for c in key {
            if c.isUppercase { out += "_" + c.lowercased() } else { out.append(c) }
        }
        return out
    }

    private static func irOf(_ value: JSONValue) -> JSONValue {
        guard case .object(let fields) = value, fields.count == 1, let (key, body) = fields.first else { return value }
        let name = snake(key)
        switch name {
        case "match_all", "match_none":
            return .string(name)
        case "ids":
            guard case .array(let ids) = body else { return value }
            return .object(["ids": .array(ids.map { id in
                // Exact as a JSON number up to 2^53; past that the IR keeps the string.
                if case .object(let o) = id, let n = o["uint"] {
                    return n.stringValue.flatMap { UInt64($0) }.flatMap { $0 <= 1 << 53 ? .int(Int($0)) : nil } ?? n
                }
                return id
            })])
        case "bool":
            guard case .object(let arms) = body else { return value }
            var out: [String: JSONValue] = [:]
            for (arm, queries) in arms {
                if case .array(let list) = queries {
                    out[snake(arm)] = .array(list.map(irOf))
                } else {
                    out[snake(arm)] = queries
                }
            }
            return .object(["bool": .object(out)])
        case "boost", "constant_score":
            guard case .object(var inner) = body else { return value }
            if let q = inner["query"] { inner["query"] = irOf(q) }
            return .object([name: .object(inner)])
        default:
            guard case .object(let inner) = body else { return .object([name: body]) }
            var out: [String: JSONValue] = [:]
            for (k, v) in inner {
                if k == "fuzziness", case .object(let f) = v {
                    out[k] = f["edits"] ?? .string("auto")
                } else {
                    out[snake(k)] = v
                }
            }
            return .object([name: .object(out)])
        }
    }
}

// MARK: - Search

/// How several retrievers' results combine.
public enum Fusion: Sendable, Equatable {
    case rrf(k: Int? = 60)
    case dbsf
    case weighted([Double])

    public var json: JSONValue {
        switch self {
        case .rrf(let k): return .object(["rrf": .object(k.map { ["k": .int($0)] } ?? [:])])
        case .dbsf: return .object(["dbsf": .object([:])])
        case .weighted(let weights): return .object(["weightedSum": .object(["weights": .array(weights.map { .number($0) })])])
        }
    }
}

/// Sort keys.
public enum Sort: Sendable, Equatable {
    case score(ascending: Bool = false)
    case pk(descending: Bool = false)
    case field(String, descending: Bool = false, missingFirst: Bool? = nil)

    public var json: JSONValue {
        switch self {
        case .score(let ascending):
            // The default order (best first) is sent unset, as the builder corpus spells it.
            return .object(["score": .object(ascending ? ["order": .string("SORT_ORDER_ASC")] : [:])])
        case .pk(let descending):
            // Ascending, the default, is sent unset.
            return .object(["pk": .object(descending ? ["order": .string("SORT_ORDER_DESC")] : [:])])
        case .field(let name, let descending, let missingFirst):
            var body: [String: JSONValue] = [
                "field": .string(name),
                "order": .string(descending ? "SORT_ORDER_DESC" : "SORT_ORDER_ASC"),
            ]
            if let missingFirst { body["missing"] = .string(missingFirst ? "MISSING_ORDER_FIRST" : "MISSING_ORDER_LAST") }
            return .object(["field": .object(body)])
        }
    }

    var isPk: Bool { if case .pk = self { return true } else { return false } }
}

/// Consistency levels for a collection read.
public enum Consistent: Sendable, Equatable {
    case strong
    case eventual
    case atLeast(String)

    public var json: JSONValue {
        switch self {
        case .strong: return .object(["freshness": .string("FRESHNESS_STRONG")])
        case .eventual: return .object(["freshness": .string("FRESHNESS_EVENTUAL")])
        case .atLeast(let token): return .object(["atLeast": .string(token)])
        }
    }
}

/// Builds a `SearchRequest`. Each verb returns a new value.
public struct Search: Sendable, Equatable {
    /// The page size `queryAll` uses when the search names no limit.
    public static let defaultK = 10

    public private(set) var collection: String
    public private(set) var namespace: String?
    private var retrievers: [JSONValue] = []
    private var fields: [String: JSONValue] = [:]
    private var highlights: [JSONValue] = []
    var sortKeys: [Sort] = []
    var limitValue: Int?

    private init(collection: String) {
        self.collection = collection
    }

    /// A search over one collection.
    public static func on(_ collection: String) -> Search { Search(collection: collection) }

    private func with(_ change: (inout Search) -> Void) -> Search {
        var copy = self
        change(&copy)
        return copy
    }

    public func inNamespace(_ ns: String) -> Search { with { $0.namespace = ns } }

    /// A text retriever: a `match` on one field, a `multiMatch` on several, or a
    /// query-string query when no field is named.
    public func text(_ text: String, fields: [String] = []) -> Search {
        let query: Query
        switch fields.count {
        case 0: query = Filter.queryString(text)
        case 1: query = Filter.match(fields[0], text)
        default: query = Filter.multiMatch(text, fields: fields)
        }
        return self.text(query)
    }

    /// A text retriever over any scoring query; `k` nil takes the default.
    public func text(_ query: Query, k: Int? = nil) -> Search {
        var body: [String: JSONValue] = ["query": query.json]
        if let k { body["k"] = .int(k) }
        return with { $0.retrievers.append(.object(["text": .object(body)])) }
    }

    /// A dense vector retriever (kNN).
    public func vector(_ field: String, _ values: [Double], k: Int? = nil, params: [String: JSONValue]? = nil,
                       filter: Query? = nil) -> Search {
        var body: [String: JSONValue] = ["field": .string(field), "query": .array(values.map { .number($0) })]
        if let k { body["k"] = .int(k) }
        if let params { body["params"] = .object(params) }
        if let filter { body["filter"] = filter.json }
        return with { $0.retrievers.append(.object(["vector": .object(body)])) }
    }

    /// A sparse vector retriever over term weights.
    public func sparse(_ field: String, _ terms: [String: Double], k: Int? = nil) -> Search {
        var body: [String: JSONValue] = ["field": .string(field), "query": .object(terms.mapValues { .number($0) })]
        if let k { body["k"] = .int(k) }
        return with { $0.retrievers.append(.object(["sparse": .object(body)])) }
    }

    public func fuse(_ fusion: Fusion) -> Search { with { $0.fields["fusion"] = fusion.json } }
    public func filter(_ query: Query) -> Search { with { $0.fields["filter"] = query.json } }
    public func sort(_ keys: Sort...) -> Search { with { $0.sortKeys = keys } }
    public func limit(_ n: Int) -> Search { with { $0.limitValue = n } }
    public func offset(_ n: Int) -> Search { with { $0.fields["offset"] = .int(n) } }
    public func after(_ sortValues: [JSONValue]) -> Search { with { $0.fields["searchAfter"] = .array(sortValues) } }
    public func minScore(_ score: Double) -> Search { with { $0.fields["scoreThreshold"] = .number(score) } }

    /// The projection: which source paths, vectors and typed fields each hit carries.
    public func select(include: [String]? = nil, exclude: [String]? = nil, vectors: [String]? = nil,
                       fields typed: [String]? = nil) -> Search {
        var select: [String: JSONValue] = [:]
        if include != nil || exclude != nil {
            // Only the lists the caller gave, as the builder corpus spells it.
            var source: [String: JSONValue] = [:]
            if let include { source["include"] = .array(include.map { .string($0) }) }
            if let exclude { source["exclude"] = .array(exclude.map { .string($0) }) }
            select["source"] = .object(source)
        }
        if let vectors { select["vectors"] = .array(vectors.map { .string($0) }) }
        if let typed { select["fields"] = .array(typed.map { .string($0) }) }
        return with { $0.fields["select"] = .object(select) }
    }

    public func highlight(_ field: String, pre: String? = nil, post: String? = nil, fragmentSize: Int? = nil,
                          fragments: Int? = nil) -> Search {
        var body: [String: JSONValue] = ["field": .string(field)]
        if let pre { body["preTag"] = .string(pre) }
        if let post { body["postTag"] = .string(post) }
        if let fragmentSize { body["fragmentSize"] = .int(fragmentSize) }
        if let fragments { body["numberOfFragments"] = .int(fragments) }
        return with { $0.highlights.append(.object(body)) }
    }

    public func groupBy(_ field: String, size: Int? = nil, limit: Int? = nil) -> Search {
        var body: [String: JSONValue] = ["field": .string(field)]
        if let size { body["groupSize"] = .int(size) }
        if let limit { body["limit"] = .int(limit) }
        return with { $0.fields["groupBy"] = .object(body) }
    }

    /// Expands each hit through a graph (`expandSpec`).
    public func expand(graph: String, fromField: String, hops: Int = 1, direction: String? = nil,
                       edgeTypes: [String] = [], limitPerSeed: Int? = nil, limit: Int? = nil) -> Search {
        var body: [String: JSONValue] = ["graph": .string(graph), "fromField": .string(fromField), "hops": .int(hops)]
        if let direction { body["direction"] = .string(direction) }
        if !edgeTypes.isEmpty { body["edgeTypes"] = .array(edgeTypes.map { .string($0) }) }
        if let limitPerSeed { body["limitPerSeed"] = .int(limitPerSeed) }
        if let limit { body["limit"] = .int(limit) }
        return with { $0.fields["expandSpec"] = .object(body) }
    }

    public func consistency(_ consistency: Consistent) -> Search { with { $0.fields["consistency"] = consistency.json } }

    /// The request's fields. A retriever's `k` is sent only when the caller set it; left
    /// unset, the engine fills in `max(10, offset + limit)`.
    public var json: [String: JSONValue] {
        var out = fields
        out["collection"] = .string(collection)
        if let namespace { out["namespace"] = .string(namespace) }
        out["retrievers"] = .array(retrievers)
        if !sortKeys.isEmpty { out["sort"] = .array(sortKeys.map(\.json)) }
        if let limitValue { out["limit"] = .int(limitValue) }
        if !highlights.isEmpty { out["highlight"] = .object(["fields": .array(highlights)]) }
        return out
    }

    /// The request as a message.
    public var request: DynamicMessage {
        DynamicMessage(protoTypeName: "loams.collection.v1.SearchRequest", fields: json)
    }
}
