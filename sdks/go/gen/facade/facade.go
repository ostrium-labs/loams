// Package facade is the SDK surface of design §44 §7.3 (D606, D730): the
// module catalogue, the binding table the runtime dispatches through, the
// reason registry, and the Connect clients `protoc-gen-connect-go` wrote.
//
// # Provenance — read this before editing
//
// The Go renderer does not exist yet. `crates/loams-facade-gen` ships
// `typescript.rs` only; `src/go.rs`, `sdks/templates/go/` and the `go` case in
// `scripts/sdk/gen.sh` are the generator agent's (SDK1 Task 3, wave 1) and were
// not in `origin/dev` when this was written. So this file is the **Q604
// hand-written-facade fallback**, which design §44 §7.3 explicitly allows:
//
//	"If the plugin proves too costly for a language, that language falls back
//	to a hand-written facade checked by the same conformance suite (D606,
//	Q604)."
//
// It is a transcription of `sdks/typescript/packages/client/src/gen/facade.ts`
// and of the annotations in `proto/`, field for field, so the runtime behind it
// is surface-agnostic: when the renderer lands this file is deleted and
// `scripts/sdk/gen.sh go` writes a replacement in the same package with the
// same exported names. **Do not grow it.** If a call is missing, the proto is
// missing the `loams.options.v1` annotation — see `docs/design/13-decision-log.md`
// Q604.
//
// The generated stubs this file's clients are built from are real: they come
// from `protoc-gen-go` and `protoc-gen-connect-go` over `proto/` (D604; the
// Go SDK is generated from the protos, not a REST wrapper).
package facade

import (
	connect "connectrpc.com/connect"

	collectionv1connect "loams.dev/go/gen/loams/collection/v1/collectionv1connect"
	errorsv1 "loams.dev/go/gen/loams/errors/v1"
	graphv1connect "loams.dev/go/gen/loams/graph/v1/graphv1connect"
	instancev1connect "loams.dev/go/gen/loams/instance/v1/instancev1connect"
	livev1connect "loams.dev/go/gen/loams/live/v1/livev1connect"
	sqlv1connect "loams.dev/go/gen/loams/sql/v1/sqlv1connect"
	streamv1connect "loams.dev/go/gen/loams/stream/v1/streamv1connect"
)

// ProtoRev is the proto revision this SDK was generated from
// (`LOAMS_PROTO_REV`, design §44 §10.3). `loams.System().Version` checks it
// against the server's `GetInstance.ApiVersions`. Pre-1.0, so it is the API
// major rather than a release tag.
const ProtoRev = "v1"

// IdempotencyLevel is `idempotency_level` off the proto method. It is what the
// retry class is derived from (D610), so it is generated rather than guessed.
type IdempotencyLevel string

// The three `idempotency_level` values a Loams method may carry.
const (
	// NoSideEffects is `NO_SIDE_EFFECTS`: a read, retryable on its own.
	NoSideEffects IdempotencyLevel = "no_side_effects"
	// Idempotent is `IDEMPOTENT`: repeating it is the same call.
	Idempotent IdempotencyLevel = "idempotent"
	// None carries no idempotency level: a mutation, which may only be retried
	// when it carries an idempotency key.
	None IdempotencyLevel = "none"
)

// RetryClass says whether the SDK may retry a call on its own.
type RetryClass string

const (
	// RetrySafe is a read or an idempotent RPC: the SDK retries it.
	RetrySafe RetryClass = "safe"
	// RetryManual is a mutation: the SDK retries it only once it carries an
	// idempotency key.
	RetryManual RetryClass = "manual"
)

// Streaming says whether a call answers with one message or with a stream.
type Streaming string

const (
	// Unary is one request, one response.
	Unary Streaming = "unary"
	// Server is a server stream. There is no client streaming and no bidi
	// (D420).
	Server Streaming = "server"
)

// Pagination names the two fields a paged call pages on, as
// `FacadeOptions.pagination` ("<items>:<next page token>") does. The field
// names are the generated Go struct field names, because that is what the
// iterator sets and reads.
type Pagination struct {
	// ItemsField is the response's repeated field, in Go's PascalCase.
	ItemsField string
	// NextPageTokenField is the response's token field, in Go's PascalCase.
	NextPageTokenField string
	// PageSizeField is the request's `page_size` field, in Go's PascalCase.
	PageSizeField string
	// PageTokenField is the request's token field, in Go's PascalCase.
	PageTokenField string
}

// CallBinding is one facade call, as the runtime dispatches it.
type CallBinding struct {
	// Module is the SDK module the call is exposed on, in snake_case.
	Module string
	// Name is the Go method name, in PascalCase (design §44 §7.1).
	Name string
	// ProtoName is the name `FacadeOptions` gave it, verbatim.
	ProtoName string
	// Method is the method that backs the call.
	Method string
	// RPC is `package.Service/Method`, which is the path curl and grpcurl use.
	RPC string
	// Service is the fully qualified service name.
	Service string
	// Package is the proto package, which is the module catalogue's key
	// (design §44 §4, D600).
	Package string
	// Idempotency is the method's `idempotency_level`.
	Idempotency IdempotencyLevel
	// Retry is the class derived from Idempotency, or from
	// `FacadeOptions.retry_safe`.
	Retry RetryClass
	// Streaming is Unary or Server.
	Streaming Streaming
	// Pagination is nil for a call that does not page.
	Pagination *Pagination
	// TakesIdempotencyKey says whether the request *message* declares an
	// `idempotency_key` field. It is a property of the message rather than of
	// the call, and it is what makes a mutation retryable (D610), so it is
	// read from the generated schema rather than from what a caller passed.
	TakesIdempotencyKey bool
}

// ModuleBinding is one SDK module and its calls.
type ModuleBinding struct {
	// Name is the module's name in snake_case, as `loams.<name>`.
	Name string
	// Summary is one line for the module's reference docs.
	Summary string
	// Service is the service behind the module.
	Service string
	// Package is the proto package, which is what `GetInstance.Services[]`
	// keys on.
	Package string
	// Unstable says the package's wire contract may still change, so `buf
	// breaking` skips it and an SDK marks the module experimental (§44
	// §10.3).
	Unstable bool
	// Derived is true for a second facade name for the same RPCs, which has no
	// summary of its own.
	Derived bool
	// Calls are the module's facade calls.
	Calls []CallBinding
}

// Modules is every annotated service, as the module catalogue, ordered by
// module name.
var Modules = []ModuleBinding{
	{
		Name:    "collections",
		Summary: "Collections: schema, aliases, versions, scan plans and the hot tier.",
		Service: collectionv1connect.CollectionServiceName,
		Package: "loams.collection.v1",
		Calls: []CallBinding{
			{
				Module: "collections", Name: "AddFields", ProtoName: "add_fields",
				Method: "AddFields", RPC: collectionv1connect.CollectionServiceName + "/AddFields",
				Service: collectionv1connect.CollectionServiceName, Package: "loams.collection.v1",
				Idempotency: None, Retry: RetrySafe, Streaming: Unary,
			},
			{
				Module: "collections", Name: "Create", ProtoName: "create",
				Method: "CreateCollection", RPC: collectionv1connect.CollectionServiceName + "/CreateCollection",
				Service: collectionv1connect.CollectionServiceName, Package: "loams.collection.v1",
				Idempotency: None, Retry: RetrySafe, Streaming: Unary,
			},
			{
				Module: "collections", Name: "Drop", ProtoName: "drop",
				Method: "DropCollection", RPC: collectionv1connect.CollectionServiceName + "/DropCollection",
				Service: collectionv1connect.CollectionServiceName, Package: "loams.collection.v1",
				Idempotency: None, Retry: RetrySafe, Streaming: Unary,
			},
			{
				Module: "collections", Name: "Get", ProtoName: "get",
				Method: "GetCollection", RPC: collectionv1connect.CollectionServiceName + "/GetCollection",
				Service: collectionv1connect.CollectionServiceName, Package: "loams.collection.v1",
				Idempotency: None, Retry: RetrySafe, Streaming: Unary,
			},
			{
				Module: "collections", Name: "List", ProtoName: "list",
				Method: "ListCollections", RPC: collectionv1connect.CollectionServiceName + "/ListCollections",
				Service: collectionv1connect.CollectionServiceName, Package: "loams.collection.v1",
				Idempotency: None, Retry: RetrySafe, Streaming: Unary,
				Pagination: &Pagination{
					ItemsField: "Collections", NextPageTokenField: "NextPageToken",
					PageSizeField: "PageSize", PageTokenField: "PageToken",
				},
			},
			{
				Module: "collections", Name: "ListVersions", ProtoName: "list_versions",
				Method: "ListVersions", RPC: collectionv1connect.CollectionServiceName + "/ListVersions",
				Service: collectionv1connect.CollectionServiceName, Package: "loams.collection.v1",
				Idempotency: None, Retry: RetrySafe, Streaming: Unary,
				Pagination: &Pagination{
					ItemsField: "Versions", NextPageTokenField: "NextPageToken",
					PageSizeField: "PageSize", PageTokenField: "PageToken",
				},
			},
			{
				Module: "collections", Name: "Scan", ProtoName: "scan",
				Method: "Scan", RPC: collectionv1connect.CollectionServiceName + "/Scan",
				Service: collectionv1connect.CollectionServiceName, Package: "loams.collection.v1",
				Idempotency: None, Retry: RetryManual, Streaming: Unary,
			},
			{
				Module: "collections", Name: "SetHot", ProtoName: "set_hot",
				Method: "SetHot", RPC: collectionv1connect.CollectionServiceName + "/SetHot",
				Service: collectionv1connect.CollectionServiceName, Package: "loams.collection.v1",
				Idempotency: None, Retry: RetrySafe, Streaming: Unary,
			},
			{
				Module: "collections", Name: "UpdateAliases", ProtoName: "update_aliases",
				Method: "UpdateAliases", RPC: collectionv1connect.CollectionServiceName + "/UpdateAliases",
				Service: collectionv1connect.CollectionServiceName, Package: "loams.collection.v1",
				Idempotency: None, Retry: RetrySafe, Streaming: Unary,
			},
			{
				Module: "collections", Name: "Warm", ProtoName: "warm",
				Method: "WarmCollection", RPC: collectionv1connect.CollectionServiceName + "/WarmCollection",
				Service: collectionv1connect.CollectionServiceName, Package: "loams.collection.v1",
				Idempotency: None, Retry: RetrySafe, Streaming: Unary,
			},
		},
	},
	{
		Name:    "documents",
		Summary: "Documents: write, get, scroll, count, delete by filter and patch by filter.",
		Service: collectionv1connect.DocumentServiceName,
		Package: "loams.collection.v1",
		Calls: []CallBinding{
			{
				Module: "documents", Name: "Count", ProtoName: "count",
				Method: "CountDocuments", RPC: collectionv1connect.DocumentServiceName + "/CountDocuments",
				Service: collectionv1connect.DocumentServiceName, Package: "loams.collection.v1",
				Idempotency: None, Retry: RetrySafe, Streaming: Unary,
			},
			{
				Module: "documents", Name: "DeleteByFilter", ProtoName: "delete_by_filter",
				Method: "DeleteByFilter", RPC: collectionv1connect.DocumentServiceName + "/DeleteByFilter",
				Service: collectionv1connect.DocumentServiceName, Package: "loams.collection.v1",
				Idempotency: None, Retry: RetryManual, Streaming: Unary,
			},
			{
				Module: "documents", Name: "Get", ProtoName: "get",
				Method: "GetDocuments", RPC: collectionv1connect.DocumentServiceName + "/GetDocuments",
				Service: collectionv1connect.DocumentServiceName, Package: "loams.collection.v1",
				Idempotency: None, Retry: RetrySafe, Streaming: Unary,
			},
			{
				Module: "documents", Name: "PatchByFilter", ProtoName: "patch_by_filter",
				Method: "PatchByFilter", RPC: collectionv1connect.DocumentServiceName + "/PatchByFilter",
				Service: collectionv1connect.DocumentServiceName, Package: "loams.collection.v1",
				Idempotency: None, Retry: RetryManual, Streaming: Unary,
			},
			{
				Module: "documents", Name: "Scroll", ProtoName: "scroll",
				Method: "ScrollDocuments", RPC: collectionv1connect.DocumentServiceName + "/ScrollDocuments",
				Service: collectionv1connect.DocumentServiceName, Package: "loams.collection.v1",
				Idempotency: None, Retry: RetrySafe, Streaming: Unary,
			},
			{
				Module: "documents", Name: "Write", ProtoName: "write",
				Method: "WriteDocuments", RPC: collectionv1connect.DocumentServiceName + "/WriteDocuments",
				Service: collectionv1connect.DocumentServiceName, Package: "loams.collection.v1",
				Idempotency: None, Retry: RetryManual, Streaming: Unary,
				TakesIdempotencyKey: true,
			},
		},
	},
	{
		Name:     "graph",
		Summary:  "Loams Graph: stored property graphs, queried with GQL.",
		Service:  graphv1connect.GraphAdminServiceName,
		Package:  "loams.graph.v1",
		Unstable: true,
		Calls: []CallBinding{
			{
				Module: "graph", Name: "Create", ProtoName: "create",
				Method: "CreateGraph", RPC: graphv1connect.GraphAdminServiceName + "/CreateGraph",
				Service: graphv1connect.GraphAdminServiceName, Package: "loams.graph.v1",
				Idempotency: None, Retry: RetryManual, Streaming: Unary,
				TakesIdempotencyKey: true,
			},
			{
				Module: "graph", Name: "Delete", ProtoName: "delete",
				Method: "DeleteGraph", RPC: graphv1connect.GraphAdminServiceName + "/DeleteGraph",
				Service: graphv1connect.GraphAdminServiceName, Package: "loams.graph.v1",
				Idempotency: None, Retry: RetryManual, Streaming: Unary,
				TakesIdempotencyKey: true,
			},
			{
				Module: "graph", Name: "Explain", ProtoName: "explain",
				Method: "Explain", RPC: graphv1connect.GraphServiceName + "/Explain",
				Service: graphv1connect.GraphServiceName, Package: "loams.graph.v1",
				Idempotency: None, Retry: RetrySafe, Streaming: Unary,
			},
			{
				Module: "graph", Name: "Get", ProtoName: "get",
				Method: "GetGraph", RPC: graphv1connect.GraphAdminServiceName + "/GetGraph",
				Service: graphv1connect.GraphAdminServiceName, Package: "loams.graph.v1",
				Idempotency: None, Retry: RetrySafe, Streaming: Unary,
			},
			{
				Module: "graph", Name: "List", ProtoName: "list",
				Method: "ListGraphs", RPC: graphv1connect.GraphAdminServiceName + "/ListGraphs",
				Service: graphv1connect.GraphAdminServiceName, Package: "loams.graph.v1",
				Idempotency: None, Retry: RetrySafe, Streaming: Unary,
				Pagination: &Pagination{
					ItemsField: "Graphs", NextPageTokenField: "NextPageToken",
					PageSizeField: "PageSize", PageTokenField: "PageToken",
				},
			},
			{
				Module: "graph", Name: "Query", ProtoName: "query",
				Method: "Execute", RPC: graphv1connect.GraphServiceName + "/Execute",
				Service: graphv1connect.GraphServiceName, Package: "loams.graph.v1",
				Idempotency: None, Retry: RetryManual, Streaming: Unary,
				TakesIdempotencyKey: true,
			},
			{
				Module: "graph", Name: "QueryBatch", ProtoName: "query_batch",
				Method: "ExecuteBatch", RPC: graphv1connect.GraphServiceName + "/ExecuteBatch",
				Service: graphv1connect.GraphServiceName, Package: "loams.graph.v1",
				Idempotency: None, Retry: RetryManual, Streaming: Unary,
				TakesIdempotencyKey: true,
			},
			{
				Module: "graph", Name: "QueryStream", ProtoName: "query_stream",
				Method: "ExecuteStream", RPC: graphv1connect.GraphServiceName + "/ExecuteStream",
				Service: graphv1connect.GraphServiceName, Package: "loams.graph.v1",
				Idempotency: None, Retry: RetryManual, Streaming: Server,
			},
			{
				Module: "graph", Name: "Schema", ProtoName: "schema",
				Method: "GetSchema", RPC: graphv1connect.GraphAdminServiceName + "/GetSchema",
				Service: graphv1connect.GraphAdminServiceName, Package: "loams.graph.v1",
				Idempotency: None, Retry: RetrySafe, Streaming: Unary,
			},
			{
				Module: "graph", Name: "Update", ProtoName: "update",
				Method: "UpdateGraph", RPC: graphv1connect.GraphAdminServiceName + "/UpdateGraph",
				Service: graphv1connect.GraphAdminServiceName, Package: "loams.graph.v1",
				Idempotency: None, Retry: RetryManual, Streaming: Unary,
				TakesIdempotencyKey: true,
			},
		},
	},
	{
		Name:    "instance",
		Summary: "What this instance is, and who the caller is on it.",
		Service: instancev1connect.InstanceServiceName,
		Package: "loams.instance.v1",
		Calls: []CallBinding{
			{
				Module: "instance", Name: "GetInstance", ProtoName: "getInstance",
				Method: "GetInstance", RPC: instancev1connect.InstanceServiceName + "/GetInstance",
				Service: instancev1connect.InstanceServiceName, Package: "loams.instance.v1",
				Idempotency: NoSideEffects, Retry: RetrySafe, Streaming: Unary,
			},
			{
				Module: "instance", Name: "WhoAmI", ProtoName: "whoAmI",
				Method: "WhoAmI", RPC: instancev1connect.InstanceServiceName + "/WhoAmI",
				Service: instancev1connect.InstanceServiceName, Package: "loams.instance.v1",
				Idempotency: NoSideEffects, Retry: RetrySafe, Streaming: Unary,
			},
		},
	},
	{
		Name:     "live",
		Summary:  "Live sync: watch a query set over a server stream.",
		Service:  livev1connect.LiveServiceName,
		Package:  "loams.live.v1",
		Unstable: true,
		Calls: []CallBinding{
			{
				Module: "live", Name: "Authenticate", ProtoName: "authenticate",
				Method: "Authenticate", RPC: livev1connect.LiveServiceName + "/Authenticate",
				Service: livev1connect.LiveServiceName, Package: "loams.live.v1",
				Idempotency: None, Retry: RetryManual, Streaming: Unary,
			},
			{
				Module: "live", Name: "ModifyQuerySet", ProtoName: "modifyQuerySet",
				Method: "ModifyQuerySet", RPC: livev1connect.LiveServiceName + "/ModifyQuerySet",
				Service: livev1connect.LiveServiceName, Package: "loams.live.v1",
				Idempotency: None, Retry: RetryManual, Streaming: Unary,
			},
			{
				Module: "live", Name: "Watch", ProtoName: "watch",
				Method: "Watch", RPC: livev1connect.LiveServiceName + "/Watch",
				Service: livev1connect.LiveServiceName, Package: "loams.live.v1",
				Idempotency: None, Retry: RetryManual, Streaming: Server,
			},
		},
	},
	{
		Name:     "live.admin",
		Summary:  "Live administration: the app directory (LV1 Task 13).",
		Service:  livev1connect.LiveAdminServiceName,
		Package:  "loams.live.v1",
		Unstable: true,
		// The app-directory calls only. Deploy is still reached as
		// `tables.Deploy`; the rest of `LiveAdminService` is not in this
		// facade yet (see the provenance note above).
		Calls: []CallBinding{
			{
				Module: "live.admin", Name: "CreateApp", ProtoName: "createApp",
				Method: "CreateApp", RPC: livev1connect.LiveAdminServiceName + "/CreateApp",
				Service: livev1connect.LiveAdminServiceName, Package: "loams.live.v1",
				Idempotency: None, Retry: RetryManual, Streaming: Unary,
			},
			{
				Module: "live.admin", Name: "DeleteApp", ProtoName: "deleteApp",
				Method: "DeleteApp", RPC: livev1connect.LiveAdminServiceName + "/DeleteApp",
				Service: livev1connect.LiveAdminServiceName, Package: "loams.live.v1",
				Idempotency: None, Retry: RetryManual, Streaming: Unary,
			},
			{
				Module: "live.admin", Name: "GetApp", ProtoName: "getApp",
				Method: "GetApp", RPC: livev1connect.LiveAdminServiceName + "/GetApp",
				Service: livev1connect.LiveAdminServiceName, Package: "loams.live.v1",
				Idempotency: None, Retry: RetrySafe, Streaming: Unary,
			},
			{
				Module: "live.admin", Name: "ListApps", ProtoName: "listApps",
				Method: "ListApps", RPC: livev1connect.LiveAdminServiceName + "/ListApps",
				Service: livev1connect.LiveAdminServiceName, Package: "loams.live.v1",
				Idempotency: None, Retry: RetrySafe, Streaming: Unary,
				Pagination: &Pagination{
					ItemsField: "Apps", NextPageTokenField: "NextPageToken",
					PageSizeField: "PageSize", PageTokenField: "PageToken",
				},
			},
			{
				Module: "live.admin", Name: "UpdateApp", ProtoName: "updateApp",
				Method: "UpdateApp", RPC: livev1connect.LiveAdminServiceName + "/UpdateApp",
				Service: livev1connect.LiveAdminServiceName, Package: "loams.live.v1",
				Idempotency: None, Retry: RetryManual, Streaming: Unary,
			},
		},
	},
	{
		Name:    "namespaces",
		Summary: "Namespaces: the container every collection and stream lives in.",
		Service: collectionv1connect.NamespaceServiceName,
		Package: "loams.collection.v1",
		Calls: []CallBinding{
			{
				Module: "namespaces", Name: "Create", ProtoName: "create",
				Method: "CreateNamespace", RPC: collectionv1connect.NamespaceServiceName + "/CreateNamespace",
				Service: collectionv1connect.NamespaceServiceName, Package: "loams.collection.v1",
				Idempotency: None, Retry: RetrySafe, Streaming: Unary,
			},
			{
				Module: "namespaces", Name: "List", ProtoName: "list",
				Method: "ListNamespaces", RPC: collectionv1connect.NamespaceServiceName + "/ListNamespaces",
				Service: collectionv1connect.NamespaceServiceName, Package: "loams.collection.v1",
				Idempotency: None, Retry: RetrySafe, Streaming: Unary,
				Pagination: &Pagination{
					ItemsField: "Namespaces", NextPageTokenField: "NextPageToken",
					PageSizeField: "PageSize", PageTokenField: "PageToken",
				},
			},
		},
	},
	{
		Name:    "search",
		Summary: "Queries: one search over one collection, dense, sparse, text or hybrid.",
		Service: collectionv1connect.QueryServiceName,
		Package: "loams.collection.v1",
		Calls: []CallBinding{
			{
				Module: "search", Name: "Query", ProtoName: "query",
				Method: "Search", RPC: collectionv1connect.QueryServiceName + "/Search",
				Service: collectionv1connect.QueryServiceName, Package: "loams.collection.v1",
				Idempotency: None, Retry: RetrySafe, Streaming: Unary,
			},
		},
	},
	{
		Name:    "sql",
		Summary: "SQL: one read-only statement over a namespace's collections, with parameters.",
		Service: sqlv1connect.SqlServiceName,
		Package: "loams.sql.v1",
		Calls: []CallBinding{
			{
				Module: "sql", Name: "Query", ProtoName: "query",
				Method: "Query", RPC: sqlv1connect.SqlServiceName + "/Query",
				Service: sqlv1connect.SqlServiceName, Package: "loams.sql.v1",
				Idempotency: NoSideEffects, Retry: RetrySafe, Streaming: Unary,
			},
			{
				Module: "sql", Name: "QueryArrow", ProtoName: "query_arrow",
				Method: "QueryArrow", RPC: sqlv1connect.SqlServiceName + "/QueryArrow",
				Service: sqlv1connect.SqlServiceName, Package: "loams.sql.v1",
				Idempotency: NoSideEffects, Retry: RetrySafe, Streaming: Unary,
			},
		},
	},
	{
		Name:    "streams",
		Summary: "Streams: create, list and describe a namespace's logs; produce and fetch records and CloudEvents.",
		Service: streamv1connect.StreamServiceName,
		Package: "loams.stream.v1",
		Calls: []CallBinding{
			{
				Module: "streams", Name: "Create", ProtoName: "create",
				Method: "CreateStream", RPC: streamv1connect.StreamServiceName + "/CreateStream",
				Service: streamv1connect.StreamServiceName, Package: "loams.stream.v1",
				Idempotency: None, Retry: RetrySafe, Streaming: Unary,
			},
			{
				Module: "streams", Name: "Fetch", ProtoName: "fetch",
				Method: "Fetch", RPC: streamv1connect.StreamServiceName + "/Fetch",
				Service: streamv1connect.StreamServiceName, Package: "loams.stream.v1",
				Idempotency: NoSideEffects, Retry: RetrySafe, Streaming: Unary,
			},
			{
				Module: "streams", Name: "FetchEvents", ProtoName: "fetch_events",
				Method: "FetchCloudEvents", RPC: streamv1connect.StreamServiceName + "/FetchCloudEvents",
				Service: streamv1connect.StreamServiceName, Package: "loams.stream.v1",
				Idempotency: NoSideEffects, Retry: RetrySafe, Streaming: Unary,
			},
			{
				Module: "streams", Name: "Get", ProtoName: "get",
				Method: "DescribeStream", RPC: streamv1connect.StreamServiceName + "/DescribeStream",
				Service: streamv1connect.StreamServiceName, Package: "loams.stream.v1",
				Idempotency: NoSideEffects, Retry: RetrySafe, Streaming: Unary,
			},
			{
				Module: "streams", Name: "List", ProtoName: "list",
				Method: "ListStreams", RPC: streamv1connect.StreamServiceName + "/ListStreams",
				Service: streamv1connect.StreamServiceName, Package: "loams.stream.v1",
				Idempotency: NoSideEffects, Retry: RetrySafe, Streaming: Unary,
				Pagination: &Pagination{
					ItemsField: "Streams", NextPageTokenField: "NextPageToken",
					PageSizeField: "PageSize", PageTokenField: "PageToken",
				},
			},
			{
				Module: "streams", Name: "Produce", ProtoName: "produce",
				Method: "Produce", RPC: streamv1connect.StreamServiceName + "/Produce",
				Service: streamv1connect.StreamServiceName, Package: "loams.stream.v1",
				Idempotency: None, Retry: RetryManual, Streaming: Unary,
			},
			{
				Module: "streams", Name: "ProduceEvents", ProtoName: "produce_events",
				Method: "ProduceCloudEvents", RPC: streamv1connect.StreamServiceName + "/ProduceCloudEvents",
				Service: streamv1connect.StreamServiceName, Package: "loams.stream.v1",
				Idempotency: None, Retry: RetryManual, Streaming: Unary,
			},
		},
	},
	{
		Name:     "tables",
		Service:  livev1connect.LiveServiceName,
		Package:  "loams.live.v1",
		Unstable: true,
		Derived:  true,
		Calls: []CallBinding{
			{
				Module: "tables", Name: "Deploy", ProtoName: "deploy",
				// Deploy moved to LiveAdminService (LV1 Task 10); `tables`
				// keeps the call, now routed there.
				Method: "Deploy", RPC: livev1connect.LiveAdminServiceName + "/Deploy",
				Service: livev1connect.LiveAdminServiceName, Package: "loams.live.v1",
				Idempotency: None, Retry: RetryManual, Streaming: Unary,
			},
			{
				Module: "tables", Name: "Mutate", ProtoName: "mutate",
				Method: "Mutate", RPC: livev1connect.LiveServiceName + "/Mutate",
				Service: livev1connect.LiveServiceName, Package: "loams.live.v1",
				Idempotency: None, Retry: RetryManual, Streaming: Unary,
				// `MutateRequest.idempotency_key` (live.proto:163). Read off
				// the generated message, not guessed: `DeployRequest` has no
				// such field and must not be keyed.
				TakesIdempotencyKey: true,
			},
			{
				Module: "tables", Name: "Query", ProtoName: "query",
				Method: "Query", RPC: livev1connect.LiveServiceName + "/Query",
				Service: livev1connect.LiveServiceName, Package: "loams.live.v1",
				Idempotency: None, Retry: RetryManual, Streaming: Unary,
			},
		},
	},
	{
		Name:    "vector",
		Service: collectionv1connect.QueryServiceName,
		Package: "loams.collection.v1",
		Derived: true,
		Calls: []CallBinding{
			{
				Module: "vector", Name: "Search", ProtoName: "search",
				Method: "Search", RPC: collectionv1connect.QueryServiceName + "/Search",
				Service: collectionv1connect.QueryServiceName, Package: "loams.collection.v1",
				Idempotency: None, Retry: RetrySafe, Streaming: Unary,
			},
		},
	},
}

// ProtoPackages is every proto package in the module, as
// `GetInstance.ApiVersions` names them.
var ProtoPackages = []string{
	"google.protobuf",
	"loams.approvals.v1",
	"loams.devices.v1",
	"loams.errors.v1",
	"loams.instance.v1",
	"loams.live.v1",
	"loams.notifications.v1",
	"loams.operations.v1",
	"loams.options.v1",
}

// ErrorInfoTypeURL is the `type` a server puts in an error detail. connect-go
// prepends the default Any prefix to a bare name, so a detail that arrives as
// `loams.errors.v1.ErrorInfo` is looked up here.
const ErrorInfoTypeURL = "loams.errors.v1.ErrorInfo"

// ErrorInfoType is the Go type of that detail.
type ErrorInfoType = errorsv1.ErrorInfo

// NewServiceClients builds one Connect client per annotated service, from the
// stubs `protoc-gen-connect-go` wrote over the same protos. This is what makes
// the facade generated rather than hand-written: the SDK never writes a client,
// it wraps the generated ones.
func NewServiceClients(httpClient connect.HTTPClient, baseURL string, opts ...connect.ClientOption) map[string]any {
	return map[string]any{
		instancev1connect.InstanceServiceName: instancev1connect.NewInstanceServiceClient(httpClient, baseURL, opts...),
		livev1connect.LiveServiceName:         livev1connect.NewLiveServiceClient(httpClient, baseURL, opts...),
		livev1connect.LiveAdminServiceName:    livev1connect.NewLiveAdminServiceClient(httpClient, baseURL, opts...),

		// SDKP: the app data planes.
		collectionv1connect.NamespaceServiceName:  collectionv1connect.NewNamespaceServiceClient(httpClient, baseURL, opts...),
		collectionv1connect.CollectionServiceName: collectionv1connect.NewCollectionServiceClient(httpClient, baseURL, opts...),
		collectionv1connect.DocumentServiceName:   collectionv1connect.NewDocumentServiceClient(httpClient, baseURL, opts...),
		collectionv1connect.QueryServiceName:      collectionv1connect.NewQueryServiceClient(httpClient, baseURL, opts...),
		graphv1connect.GraphAdminServiceName:      graphv1connect.NewGraphAdminServiceClient(httpClient, baseURL, opts...),
		graphv1connect.GraphServiceName:           graphv1connect.NewGraphServiceClient(httpClient, baseURL, opts...),
		sqlv1connect.SqlServiceName:               sqlv1connect.NewSqlServiceClient(httpClient, baseURL, opts...),
		streamv1connect.StreamServiceName:         streamv1connect.NewStreamServiceClient(httpClient, baseURL, opts...),
	}
}
