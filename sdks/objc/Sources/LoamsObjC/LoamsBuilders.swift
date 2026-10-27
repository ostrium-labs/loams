import Foundation

/// Drops the nil values, so an unset optional field is absent.
func loamsCompact(_ pairs: [(String, Any?)]) -> [String: Any] {
    var out: [String: Any] = [:]
    for (key, value) in pairs {
        if let value = value { out[key] = value }
    }
    return out
}

/// The `CreateCollectionRequest.schema` Struct (SDKP §2.6), in the shape the
/// server's schema parser reads. Each verb answers the same builder.
///
///     LoamsSchema().keyword("title").text("body").i64("n").vector("embedding", dim: 4)
#if canImport(ObjectiveC)
@objcMembers
#endif
public final class LoamsSchema: NSObject, @unchecked Sendable {
    private var fields: [[String: Any]] = []
    private var vectors: [[String: Any]] = []
    private var sparse: [[String: Any]] = []
    private var dynamicMode = "ignore"
    private var maxFieldCount = 1000

    /// A full-text field; `positions` is what phrase queries need.
    @discardableResult
    public func text(_ name: String) -> LoamsSchema {
        return text(name, analyzer: "standard", positions: true)
    }

    @discardableResult
    public func text(_ name: String, analyzer: String, positions: Bool) -> LoamsSchema {
        return field(name, kind: ["text": ["analyzer": analyzer, "positions": positions]], indexed: true, fast: false)
    }

    @discardableResult
    public func keyword(_ name: String) -> LoamsSchema { return field(name, kind: "keyword", indexed: true, fast: true) }

    @discardableResult
    public func i64(_ name: String) -> LoamsSchema { return field(name, kind: "i64", indexed: true, fast: true) }

    @discardableResult
    public func f64(_ name: String) -> LoamsSchema { return field(name, kind: "f64", indexed: true, fast: true) }

    @discardableResult
    public func boolean(_ name: String) -> LoamsSchema { return field(name, kind: "bool", indexed: true, fast: true) }

    /// A date-time field (the schema's `date` kind).
    @discardableResult
    public func timestamp(_ name: String) -> LoamsSchema { return field(name, kind: "date", indexed: true, fast: true) }

    @discardableResult
    public func uuid(_ name: String) -> LoamsSchema { return field(name, kind: "uuid", indexed: true, fast: true) }

    /// Any field kind, with its overrides.
    @discardableResult
    public func field(_ name: String, kind: Any, indexed: Bool, fast: Bool, sourcePath: String? = nil) -> LoamsSchema {
        fields.append(["name": name, "source_path": sourcePath ?? name, "kind": kind, "indexed": indexed, "fast": fast])
        return self
    }

    @discardableResult
    public func vector(_ name: String, dim: Int, distance: String = "cosine") -> LoamsSchema {
        vectors.append(["name": name, "dim": dim, "distance": distance])
        return self
    }

    @discardableResult
    public func sparseVector(_ name: String, modifier: String? = nil) -> LoamsSchema {
        sparse.append(loamsCompact([("name", name), ("modifier", modifier)]))
        return self
    }

    /// `ignore` (the builder's default), `strict` (the server's) or `map`.
    @discardableResult
    public func dynamicMapping(_ mode: String) -> LoamsSchema {
        dynamicMode = mode
        return self
    }

    @discardableResult
    public func maxFields(_ count: Int) -> LoamsSchema {
        maxFieldCount = count
        return self
    }

    public func toJSON() -> [String: Any] {
        return ["fields": fields, "vectors": vectors, "sparse_vectors": sparse,
                "dynamic": dynamicMode, "max_fields": maxFieldCount]
    }
}

/// One `loams.collection.v1.Query` (SDKP §2.4): a search's filter, a text
/// retriever's query, the typed `where` of the document reads.
///
///     LoamsFilter.and([LoamsFilter.eq("tags", value: "policy"), LoamsFilter.exists("title")])
#if canImport(ObjectiveC)
@objcMembers
#endif
public final class LoamsFilter: NSObject, @unchecked Sendable {
    public let json: [String: Any]

    public init(json: [String: Any]) {
        self.json = json
        super.init()
    }

    public static func all() -> LoamsFilter { return LoamsFilter(json: ["matchAll": [String: Any]()]) }
    public static func none() -> LoamsFilter { return LoamsFilter(json: ["matchNone": [String: Any]()]) }

    /// `field` equals `value` (a term query).
    public static func eq(_ field: String, value: Any) -> LoamsFilter {
        return LoamsFilter(json: ["term": ["field": field, "value": value]])
    }

    /// `field` is one of `values` (a terms query).
    public static func isIn(_ field: String, values: [Any]) -> LoamsFilter {
        return LoamsFilter(json: ["terms": ["field": field, "values": values]])
    }

    /// Unset bounds are nil.
    public static func range(_ field: String, gt: Any? = nil, gte: Any? = nil, lt: Any? = nil, lte: Any? = nil) -> LoamsFilter {
        return LoamsFilter(json: ["range": loamsCompact([("field", field), ("gt", gt), ("gte", gte), ("lt", lt), ("lte", lte)])])
    }

    public static func exists(_ field: String) -> LoamsFilter { return LoamsFilter(json: ["exists": ["field": field]]) }
    public static func isNull(_ field: String) -> LoamsFilter { return LoamsFilter(json: ["isNull": ["field": field]]) }
    public static func isEmpty(_ field: String) -> LoamsFilter { return LoamsFilter(json: ["isEmpty": ["field": field]]) }

    public static func prefix(_ field: String, value: String) -> LoamsFilter {
        return LoamsFilter(json: ["prefix": ["field": field, "value": value]])
    }

    public static func wildcard(_ field: String, pattern: String) -> LoamsFilter {
        return LoamsFilter(json: ["wildcard": ["field": field, "pattern": pattern]])
    }

    /// `edits` is a number of edits or `auto`; nil leaves the server's default.
    public static func fuzzy(_ field: String, value: String, edits: String? = nil) -> LoamsFilter {
        return LoamsFilter(json: ["fuzzy": loamsCompact([("field", field), ("value", value), ("fuzziness", fuzziness(edits))])])
    }

    /// Full-text match; `op` is `or` (default) or `and`.
    public static func match(_ field: String, text: String, op: String? = nil, minimumShouldMatch: String? = nil) -> LoamsFilter {
        return LoamsFilter(json: ["match": loamsCompact([("field", field), ("text", text), ("operator", op),
                                                         ("minimumShouldMatch", minimumShouldMatch)])])
    }

    public static func phrase(_ field: String, text: String, slop: NSNumber? = nil) -> LoamsFilter {
        return LoamsFilter(json: ["matchPhrase": loamsCompact([("field", field), ("text", text), ("slop", slop)])])
    }

    /// One text over several fields: names or `name^boost`.
    public static func multiMatch(_ fields: [String], text: String) -> LoamsFilter {
        return LoamsFilter(json: ["multiMatch": ["fields": fields.map(boosted), "text": text]])
    }

    public static func queryString(_ query: String, fields: [String]? = nil) -> LoamsFilter {
        let defaults: [String]? = (fields?.isEmpty ?? true) ? nil : fields
        return LoamsFilter(json: ["queryString": loamsCompact([("query", query), ("defaultFields", defaults)])])
    }

    /// Documents by primary key: integers, strings or `["uuid": …]`.
    public static func ids(_ ids: [Any]) -> LoamsFilter {
        return LoamsFilter(json: ["ids": ids.map(idValue)])
    }

    /// A primary key as `Query.ids` spells it: a number is `["uint": "…"]`, a
    /// string stays a string, a `["uuid": …]` dictionary passes through.
    public static func idValue(_ id: Any) -> Any {
        switch id {
        case let i as Int: return ["uint": String(i)]
        case let u as UInt64: return ["uint": String(u)]
        case let n as NSNumber: return ["uint": n.stringValue]
        default: return id
        }
    }

    /// Every clause matches (`bool.must`).
    public static func and(_ clauses: [LoamsFilter]) -> LoamsFilter {
        return LoamsFilter(json: ["bool": ["must": clauses.map { $0.json }]])
    }

    /// Any clause matches (`bool.should`).
    public static func or(_ clauses: [LoamsFilter]) -> LoamsFilter {
        return LoamsFilter(json: ["bool": ["should": clauses.map { $0.json }]])
    }

    /// At least `minimumShouldMatch` of `clauses` match.
    public static func anyOf(_ clauses: [LoamsFilter], minimumShouldMatch: String) -> LoamsFilter {
        return LoamsFilter(json: ["bool": ["should": clauses.map { $0.json }, "minimumShouldMatch": minimumShouldMatch]])
    }

    /// No clause matches (`bool.mustNot`).
    public static func not(_ clauses: [LoamsFilter]) -> LoamsFilter {
        return LoamsFilter(json: ["bool": ["mustNot": clauses.map { $0.json }]])
    }

    /// Every clause matches, without scoring (`bool.filter`).
    public static func filter(_ clauses: [LoamsFilter]) -> LoamsFilter {
        return LoamsFilter(json: ["bool": ["filter": clauses.map { $0.json }]])
    }

    public static func boost(_ clause: LoamsFilter, by boost: Double) -> LoamsFilter {
        return LoamsFilter(json: ["boost": ["query": clause.json, "boost": boost]])
    }

    public static func constantScore(_ clause: LoamsFilter, score: Double) -> LoamsFilter {
        return LoamsFilter(json: ["constantScore": ["query": clause.json, "score": score]])
    }

    private static func fuzziness(_ edits: String?) -> Any? {
        guard let edits = edits else { return nil }
        if edits == "auto" { return ["auto": [String: Any]()] }
        return ["edits": Int(edits) ?? 0]
    }

    private static func boosted(_ spec: String) -> [String: Any] {
        if let at = spec.lastIndex(of: "^"), at != spec.startIndex, let boost = Double(spec[spec.index(after: at)...]) {
            return ["field": String(spec[..<at]), "boost": boost]
        }
        return ["field": spec]
    }
}

/// How a hybrid search fuses its retrievers' rankings.
#if canImport(ObjectiveC)
@objcMembers
#endif
public final class LoamsFusion: NSObject {
    /// Reciprocal rank fusion; nil leaves `k` at the server's default (60).
    public static func rrf(_ k: NSNumber? = nil) -> [String: Any] {
        return ["rrf": k.map { ["k": $0.intValue] } ?? [String: Any]()]
    }

    /// Distribution-based score fusion.
    public static func dbsf() -> [String: Any] { return ["dbsf": [String: Any]()] }

    /// A weighted sum, one weight per retriever.
    public static func weighted(_ weights: [Double]) -> [String: Any] { return ["weightedSum": ["weights": weights]] }
}

/// One sort key; `order` is `asc` or `desc`.
#if canImport(ObjectiveC)
@objcMembers
#endif
public final class LoamsSort: NSObject {
    public static func score(_ order: String? = nil) -> [String: Any] { return ["score": orderJSON(order)] }

    /// The primary key: the tiebreaker `queryAll` requires.
    public static func pk(_ order: String? = nil) -> [String: Any] { return ["pk": orderJSON(order)] }

    /// `missing` is `first` or `last`.
    public static func field(_ name: String, order: String? = nil, missing: String? = nil) -> [String: Any] {
        return ["field": loamsCompact([("field", name), ("order", order.map(orderName)),
                                       ("missing", missing.map { "MISSING_ORDER_\($0.uppercased())" })])]
    }

    private static func orderJSON(_ order: String?) -> [String: Any] {
        return order.map { ["order": orderName($0)] } ?? [:]
    }

    private static func orderName(_ order: String) -> String {
        return order == "asc" ? "SORT_ORDER_ASC" : "SORT_ORDER_DESC"
    }
}

/// `strong`, `eventual`, or a token (checked here) as `atLeast`.
func loamsConsistencyJSON(_ level: String?) throws -> [String: Any]? {
    guard let level = level else { return nil }
    switch level {
    case "strong": return ["freshness": "FRESHNESS_STRONG"]
    case "eventual": return ["freshness": "FRESHNESS_EVENTUAL"]
    default: return ["atLeast": try LoamsConsistencyToken.parse(level).stringValue]
    }
}

/// A `SearchRequest`, verb by verb (SDKP §2.4). Each verb answers a new
/// builder, so a builder can be shared and extended:
///
///     LoamsSearch.on("notes")
///         .text("refund", fields: ["body"])
///         .vector("embedding", values: [1, 0, 0, 0])
///         .fuse(LoamsFusion.rrf(60))
///         .filter(LoamsFilter.eq("tags", value: "policy"))
///         .limit(2)
///
/// `json` is the request exactly as built (what the builder corpus pins);
/// `build()` checks it too, and the search module sends `wire` of it.
#if canImport(ObjectiveC)
@objcMembers
#endif
public final class LoamsSearch: NSObject, @unchecked Sendable {
    /// The page size `queryAll` uses when the request sets no `limit`.
    public static let defaultK = 10

    public let json: [String: Any]

    public init(json: [String: Any]) {
        self.json = json
        super.init()
    }

    public static func on(_ collection: String) -> LoamsSearch { return LoamsSearch(json: ["collection": collection]) }

    public func namespace(_ namespace: String) -> LoamsSearch { return with("namespace", namespace) }
    public func collection(_ collection: String) -> LoamsSearch { return with("collection", collection) }

    /// A text retriever over a string: a match on one field, a multi-match on
    /// several, a query string with none.
    public func text(_ text: String, fields: [String]? = nil, k: NSNumber? = nil) -> LoamsSearch {
        let fs = fields ?? []
        let query = fs.isEmpty ? LoamsFilter.queryString(text) : fs.count == 1 ? LoamsFilter.match(fs[0], text: text) : LoamsFilter.multiMatch(fs, text: text)
        return textQuery(query, k: k)
    }

    /// A text retriever over any query.
    public func textQuery(_ query: LoamsFilter, k: NSNumber? = nil) -> LoamsSearch {
        return retriever("text", loamsCompact([("query", query.json), ("k", k?.intValue)]))
    }

    /// A dense-vector (kNN) retriever; the ANN params are `AnnParams`.
    public func vector(_ field: String, values: [Double], k: NSNumber? = nil, exact: NSNumber? = nil,
                       nprobes: NSNumber? = nil, refineFactor: NSNumber? = nil, ef: NSNumber? = nil,
                       oversampling: NSNumber? = nil, distance: String? = nil, filter: LoamsFilter? = nil) -> LoamsSearch {
        let params = loamsCompact([("exact", exact?.boolValue), ("nprobes", nprobes?.intValue),
                                   ("refineFactor", refineFactor?.intValue), ("ef", ef?.intValue),
                                   ("oversampling", oversampling?.doubleValue), ("distance", distance)])
        return retriever("vector", loamsCompact([("field", field), ("query", values), ("k", k?.intValue),
                                                 ("params", params.isEmpty ? nil : params), ("filter", filter?.json)]))
    }

    /// A sparse-vector retriever over `indices` and their `values`.
    public func sparse(_ field: String, indices: [Int], values: [Double], k: NSNumber? = nil) -> LoamsSearch {
        return retriever("sparse", loamsCompact([("field", field), ("query", ["indices": indices, "values": values]),
                                                 ("k", k?.intValue)]))
    }

    public func fuse(_ fusion: [String: Any]) -> LoamsSearch { return with("fusion", fusion) }

    public func filter(_ filter: LoamsFilter) -> LoamsSearch { return with("filter", filter.json) }

    /// `LoamsSort.score()`, `LoamsSort.pk()`, `LoamsSort.field(…)`, or `"score"` / `"pk"`.
    public func sort(_ keys: [Any]) -> LoamsSearch {
        return with("sort", keys.map { key -> Any in
            if let name = key as? String { return name == "pk" ? LoamsSort.pk() : LoamsSort.score() }
            return key
        })
    }

    public func limit(_ limit: Int) -> LoamsSearch { return with("limit", limit) }
    public func offset(_ offset: Int) -> LoamsSearch { return with("offset", offset) }

    /// Continue after a hit: its `sortValues`.
    public func after(_ sortValues: [Any]) -> LoamsSearch { return with("searchAfter", sortValues) }

    public func minScore(_ score: Double) -> LoamsSearch { return with("scoreThreshold", score) }

    /// The projection: which `_source` paths (or `noSource`), vectors and typed fields to return.
    public func select(include: [String]? = nil, exclude: [String]? = nil, noSource: Bool = false,
                       vectors: [String]? = nil, fields: [String]? = nil) -> LoamsSearch {
        let source = loamsCompact([("include", include), ("exclude", exclude)])
        let src: Any? = noSource ? "none" : (source.isEmpty ? nil : source)
        return with("select", loamsCompact([("source", src), ("vectors", vectors), ("fields", fields)]))
    }

    public func highlight(_ field: String, pre: String? = nil, post: String? = nil,
                          fragmentSize: NSNumber? = nil, fragments: NSNumber? = nil) -> LoamsSearch {
        let existing = ((json["highlight"] as? [String: Any])?["fields"] as? [Any]) ?? []
        let entry = loamsCompact([("field", field), ("preTag", pre), ("postTag", post),
                                  ("fragmentSize", fragmentSize?.intValue), ("numberOfFragments", fragments?.intValue)])
        return with("highlight", ["fields": existing + [entry]])
    }

    public func groupBy(_ field: String, size: NSNumber? = nil, limit: NSNumber? = nil) -> LoamsSearch {
        return with("groupBy", loamsCompact([("field", field), ("groupSize", size?.intValue), ("limit", limit?.intValue)]))
    }

    /// Graph expansion of the hits (`expand_spec`). `direction` is `out`, `in` or
    /// `both`; `output` is `nested` or `flat`.
    public func expand(_ graph: String, fromField: String? = nil, hops: NSNumber? = nil, direction: String? = nil,
                       edgeTypes: [String]? = nil, limitPerSeed: NSNumber? = nil, limit: NSNumber? = nil,
                       output: String? = nil) -> LoamsSearch {
        return with("expandSpec", loamsCompact([
            ("graph", graph), ("fromField", fromField), ("hops", hops?.intValue),
            ("direction", direction.map { "EXPAND_DIRECTION_\($0.uppercased())" }),
            ("edgeTypes", edgeTypes), ("limitPerSeed", limitPerSeed?.intValue), ("limit", limit?.intValue),
            ("output", output.map { "EXPAND_OUTPUT_\($0.uppercased())" }),
        ]))
    }

    /// `strong`, `eventual`, or a token as `atLeast`. Throws for a malformed token.
    public func consistency(_ level: String) throws -> LoamsSearch {
        return with("consistency", try loamsConsistencyJSON(level))
    }

    /// Whether the last sort key is the primary key, which `queryAll` needs.
    public var hasTiebreak: Bool { return LoamsSearch.hasTiebreak(json) }

    public static func hasTiebreak(_ request: [String: Any]) -> Bool {
        guard let last = (request["sort"] as? [Any])?.last as? [String: Any] else { return false }
        return last["pk"] != nil
    }

    /// `json`, checked: a weighted fusion needs one weight per retriever.
    public func build() throws -> [String: Any] {
        return try LoamsSearch.check(json)
    }

    public static func check(_ request: [String: Any]) throws -> [String: Any] {
        let weights = ((request["fusion"] as? [String: Any])?["weightedSum"] as? [String: Any])?["weights"] as? [Any]
        let count = (request["retrievers"] as? [Any])?.count ?? 0
        if let weights = weights, weights.count != count {
            throw LoamsError(message: "a weighted fusion needs one weight per retriever: \(weights.count) weights for \(count) retrievers",
                             codeString: "invalid_argument", reason: nil, unknownReason: nil, httpStatus: 400,
                             rawDetails: [["type": "loams.errors.v1.ErrorInfo",
                                           "metadata": ["field": "fusion.weighted_sum.weights"]]])
        }
        return request
    }

    /// What the modules send: `build()`. A retriever's `k` goes out only when the
    /// caller gave one; the engine fills `max(10, offset + limit)` itself
    /// (SDKP-15, SDKP-18).
    public static func wire(_ request: [String: Any]) throws -> [String: Any] {
        return try check(request)
    }

    private func with(_ key: String, _ value: Any?) -> LoamsSearch {
        var next = json
        next[key] = value
        return LoamsSearch(json: next)
    }

    private func retriever(_ kind: String, _ body: [String: Any]) -> LoamsSearch {
        let existing = (json["retrievers"] as? [Any]) ?? []
        return with("retrievers", existing + [[kind: body]])
    }
}
