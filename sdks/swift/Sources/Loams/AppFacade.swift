// The app-building API's bindings (SDKP §2.2–§2.5).
//
// `loams.collection.v1` and `loams.graph.v1` carry no method-level
// `FacadeOptions` yet (SDKP P0a adds them), so these rows are transcribed from
// the plan's tables: module and call names, and the retry class each table
// gives. When P0a lands, `swift_facade_matches_index` checks every row against
// `sdks/fixtures/facade-index.json`.

import Foundation

enum LoamsAppService {
    static let namespaces = "loams.collection.v1.NamespaceService"
    static let collections = "loams.collection.v1.CollectionService"
    static let documents = "loams.collection.v1.DocumentService"
    static let query = "loams.collection.v1.QueryService"
    static let graphAdmin = "loams.graph.v1.GraphAdminService"
    static let graph = "loams.graph.v1.GraphService"
}

private func appCall(
    _ module: String,
    _ name: String,
    _ protoName: String,
    _ service: String,
    _ method: String,
    _ idempotency: IdempotencyLevel,
    _ retry: RetryClass,
    streaming: Streaming = .unary,
    page items: String? = nil,
    keyed: Bool = false
) -> CallBinding {
    CallBinding(
        module: module,
        name: name,
        protoName: protoName,
        method: method,
        rpc: service + "/" + method,
        service: service,
        package: String(service[..<service.lastIndex(of: ".")!]),
        idempotency: idempotency,
        retry: retry,
        streaming: streaming,
        pagination: items.map { Pagination(itemsField: $0, nextPageTokenField: "next_page_token") },
        takesIdempotencyKey: keyed
    )
}

private func appModule(
    _ name: String,
    _ summary: String,
    _ service: String,
    unstable: Bool = false,
    _ calls: [CallBinding]
) -> ModuleBinding {
    ModuleBinding(
        name: name,
        summary: summary,
        service: service,
        package: String(service[..<service.lastIndex(of: ".")!]),
        unstable: unstable,
        calls: calls
    )
}

let loamsAppModules: [ModuleBinding] = {
    let ns = LoamsAppService.namespaces
    let col = LoamsAppService.collections
    let doc = LoamsAppService.documents
    let q = LoamsAppService.query
    let ga = LoamsAppService.graphAdmin
    let g = LoamsAppService.graph
    let sql = "loams.sql.v1.SqlService"
    let st = "loams.stream.v1.StreamService"
    return [
        appModule("namespaces", "Namespaces: the container every collection and stream lives in.", ns, [
            appCall("namespaces", "Create", "create", ns, "CreateNamespace", .none, .safe),
            appCall("namespaces", "List", "list", ns, "ListNamespaces", .noSideEffects, .safe, page: "namespaces"),
        ]),
        appModule("collections", "Collections: schema, aliases, versions, scan plans and the hot tier.", col, [
            appCall("collections", "Create", "create", col, "CreateCollection", .none, .safe),
            appCall("collections", "List", "list", col, "ListCollections", .noSideEffects, .safe, page: "collections"),
            appCall("collections", "Get", "get", col, "GetCollection", .noSideEffects, .safe),
            appCall("collections", "Drop", "drop", col, "DropCollection", .none, .safe),
            appCall("collections", "AddFields", "addFields", col, "AddFields", .none, .safe),
            appCall("collections", "ListVersions", "listVersions", col, "ListVersions", .noSideEffects, .safe, page: "versions"),
            appCall("collections", "UpdateAliases", "updateAliases", col, "UpdateAliases", .none, .safe),
            appCall("collections", "Scan", "scan", col, "Scan", .none, .manual),
            appCall("collections", "SetHot", "setHot", col, "SetHot", .none, .safe),
            appCall("collections", "Warm", "warm", col, "WarmCollection", .none, .safe),
        ]),
        appModule("documents", "Documents: write, get, scroll, count, delete by filter and patch by filter.", doc, [
            appCall("documents", "Write", "write", doc, "WriteDocuments", .none, .manual, keyed: true),
            appCall("documents", "Get", "get", doc, "GetDocuments", .noSideEffects, .safe),
            appCall("documents", "Scroll", "scroll", doc, "ScrollDocuments", .noSideEffects, .safe),
            appCall("documents", "Count", "count", doc, "CountDocuments", .noSideEffects, .safe),
            appCall("documents", "DeleteByFilter", "deleteByFilter", doc, "DeleteByFilter", .none, .manual),
            appCall("documents", "PatchByFilter", "patchByFilter", doc, "PatchByFilter", .none, .manual),
        ]),
        // Q613: one RPC, two facade entries.
        appModule("search", "Queries: one search over one collection, dense, sparse, text or hybrid.", q, [
            appCall("search", "Query", "query", q, "Search", .noSideEffects, .safe),
        ]),
        appModule("vector", "Vector search: the search RPC with a vector's shape.", q, [
            appCall("vector", "Search", "search", q, "Search", .noSideEffects, .safe),
        ]),
        appModule("graph", "Loams Graph: stored property graphs, queried with GQL.", g, unstable: true, [
            appCall("graph", "Create", "create", ga, "CreateGraph", .none, .manual, keyed: true),
            appCall("graph", "Get", "get", ga, "GetGraph", .noSideEffects, .safe),
            appCall("graph", "List", "list", ga, "ListGraphs", .noSideEffects, .safe, page: "graphs"),
            appCall("graph", "Update", "update", ga, "UpdateGraph", .none, .manual),
            appCall("graph", "Delete", "delete", ga, "DeleteGraph", .none, .manual),
            appCall("graph", "Schema", "schema", ga, "GetSchema", .noSideEffects, .safe),
            appCall("graph", "Query", "query", g, "Execute", .none, .manual, keyed: true),
            appCall("graph", "QueryBatch", "queryBatch", g, "ExecuteBatch", .none, .manual, keyed: true),
            appCall("graph", "QueryStream", "queryStream", g, "ExecuteStream", .none, .manual, streaming: .server),
            appCall("graph", "Explain", "explain", g, "Explain", .none, .safe),
        ]),
        // Part C: loams.sql.v1 and loams.stream.v1 (SDKP P0b and P0c).
        appModule("sql", "SQL: one read-only statement over a namespace's collections, with parameters.", sql, [
            appCall("sql", "Query", "query", sql, "Query", .noSideEffects, .safe),
            appCall("sql", "QueryArrow", "queryArrow", sql, "QueryArrow", .noSideEffects, .safe),
        ]),
        appModule("streams", "Streams: create, list and describe a namespace's logs; produce and fetch.", st, [
            appCall("streams", "Create", "create", st, "CreateStream", .none, .safe),
            appCall("streams", "Get", "get", st, "DescribeStream", .noSideEffects, .safe),
            appCall("streams", "List", "list", st, "ListStreams", .noSideEffects, .safe, page: "streams"),
            appCall("streams", "Produce", "produce", st, "Produce", .none, .manual),
            appCall("streams", "Fetch", "fetch", st, "Fetch", .noSideEffects, .safe),
            appCall("streams", "ProduceEvents", "produceEvents", st, "ProduceCloudEvents", .none, .manual),
            appCall("streams", "FetchEvents", "fetchEvents", st, "FetchCloudEvents", .noSideEffects, .safe),
        ]),
    ]
}()

/// The packages R11 (default namespace) applies to: every request is scoped to
/// a namespace there (SDKP §2.1).
let loamsNamespacedPackages: Set<String> = [
    "loams.collection.v1", "loams.graph.v1", "loams.sql.v1", "loams.stream.v1",
]

/// RPCs of those packages whose request has no top-level `namespace`.
let loamsNoTopLevelNamespace: Set<String> = [
    LoamsAppService.graphAdmin + "/UpdateGraph",
    LoamsAppService.graphAdmin + "/GetEngineInfo",
    LoamsAppService.graph + "/ExecuteStream",
    LoamsAppService.namespaces + "/ListNamespaces",
]

/// The collection reads whose request has a `consistency` field R4's session
/// fills (`at_least`) when the caller left it unset. Graph reads are not here:
/// a graph token names one graph's own stream, so a merged collection token is
/// refused there.
let loamsSessionReads: Set<String> = [
    LoamsAppService.documents + "/GetDocuments",
    LoamsAppService.documents + "/ScrollDocuments",
    LoamsAppService.documents + "/CountDocuments",
    LoamsAppService.query + "/Search",
    "loams.sql.v1.SqlService/Query",
    "loams.sql.v1.SqlService/QueryArrow",
]
