// Queries: the search IR as typed messages, and its answer (design §44 §5.1,
// D602; API1 Task 4).
//
// This is the third file of `loams.collection.v1`. It replaces the M1.2
// native REST route `POST /v1/namespaces/{ns}/query` — the `SearchRequest` of
// `crates/loams-query/src/ir.rs`, which is the IR the REST route parses
// verbatim — with `QueryService/Search` on the one port (design §44 §4).
//
// - **Names are fields, not path segments.** `namespace` and `collection`
//   travel in the request message and one path serves every collection (plan
//   ruling 4), exactly as `collection.proto` and `document.proto` do.
// - **proto3 JSON is the wire form**: field names are `lowerCamelCase`, an
//   enum answers its proto name in `UPPER_SNAKE`, a 64-bit integer answers a
//   decimal string, and a field at its default is *absent* rather than `0`,
//   `false`, `""` or `[]`.
//
// ## The one divergence that is unavoidable: `snake_case` → `lowerCamelCase`
//
// The REST IR is serde `snake_case` and proto3 JSON is `lowerCamelCase`, so
// `score_threshold` is `scoreThreshold`, `minimum_should_match` is
// `minimumShouldMatch`, `track_total_hits` is `trackTotalHits` and
// `must_not` is `mustNot`. Nothing here *renames* anything else: the IR's field
// names are the proto field names, so the two surfaces are one vocabulary with
// two spellings and no translation table to keep.
//
// Three rules follow from proto3 JSON rather than from the IR, and
// `crates/loams/tests/connect_query.rs` pins all three:
//
// - **A `null` is an absent field.** proto3 JSON has no `null` for a scalar or
//   a message field, and a field at its default is *absent*. The REST fixture
//   spells its defaults out (`"nprobes": null`, `"fusion": null`), and on this
//   surface the same value is the field being absent. That is why every
//   optional IR field is `optional` or a message rather than a sentinel: a
//   caller that must distinguish "not asked" from "asked for zero" needs the
//   presence, and proto3 gives it for `optional`.
// - **A oneof arm is `{"arm": {…}}`, and a unit arm is `{"arm": {}}`.** The IR
//   spells `Query::MatchAll` as the bare string `"match_all"`, which a proto
//   oneof has no spelling for, so `match_all` is an empty message. `WriteOp`
//   set this precedent in this package, and `Query`, `Retriever`, `Fusion`,
//   `SortKey` and `TrackTotalHits` follow it.
// - **A bare id is a `DocumentId`.** `Query::Ids` is a `repeated DocumentId`
//   and `Hit.pk` is one, for Task 3's reason: `google.protobuf.Value` holds
//   every number in a `double`, so a `u64::MAX` id would come back as
//   `18446744073709552000` — a different document.
//
// ## The §05 §4 body is a spelling of this request, not another request
//
// Design §05 §4's example body is accepted by the same RPC, and this file
// declares the four keys that reach it verbatim rather than through the typed
// messages:
//
// | §05 §4 | here |
// |---|---|
// | `"from": "collections.kb"` | `SearchRequest.from`, an alias of `collection` |
// | `"retrieve": [...]` | `SearchRequest.retrieve`, an alias of `retrievers` |
// | `"fuse": {"method": "rrf", "k": 60}` | `SearchRequest.fuse`, a `Struct`, verbatim |
// | `"select": ["id", "_score", "body"]` | `SearchRequest.select`, the projection `Struct` of Task 2/3 |
//
// `fuse` is a `Struct` rather than a `Fusion` on purpose. §05 §4 names its
// method with the string `"rrf"`, and ruling 2.1's shape for this package is
// the IR spelling rather than a new `FUSION_METHOD_*` enumeration, so the
// `Struct` keeps `method` as the string a reader already switches on
// verbatim — the same reason `Patch.mode` and `ScanColumn.role` are strings.
// A field-for-field verbatim decode of the whole §05 §4 body is impossible
// (a message field has no bare-string form), so these four keys are the
// minimal reading that keeps the plan's sentence true: `from` and `retrieve`
// are the field aliases, `fuse` and `select` are the two payloads the IR
// spells differently. Everything else of §05 §4 — `consistency`, the
// retrievers, `limit`, `offset` — is this message's own spelling.
//
// `from` and `retrieve` are *aliases*, not a second request: they are read
// into the same fields `collection` and `retrievers` hold, and a request that
// sets both is not two requests. `collection` wins over `from` and
// `retrievers` over `retrieve`, because the typed field is the one a generated
// SDK fills in and the alias exists for a caller migrating a §05 §4 body.
//
// ## `rerank` and `expand` are declared so they can be refused
//
// proto3 JSON says to skip a field the message does not declare. If `rerank`
// and `expand` were simply absent, a body carrying either would be accepted
// with the stage silently dropped — the one failure a caller cannot detect.
// They are therefore declared as `Struct`s that **every implementation
// refuses**: both stages need graph expansion (design §07) and arrive in M3,
// and a caller that sends one gets `invalid_argument` saying so, on both
// surfaces, from the same code that refuses it over REST. This is the only
// reason they are here, and the two `Struct`s exist to be rejected rather than
// to carry a payload.
//
// ## `track_total_hits` is a `Value`, and the reason is the REST spelling
//
// Every other field of this message is typed, and this one is not: the IR's
// `TrackTotalHits` has two spellings a caller may send — the serde
// externally-tagged bare string `"exact"` and the proto3 JSON message form
// `{"exact": {}}` — and a proto3 message field has only the second of them.
// Carrying the union as a `google.protobuf.Value` is what lets a body written
// against the REST IR and one written against this proto both mean "count
// exactly", instead of forcing every such caller to learn a second spelling.
// The handler maps the `Value` to the IR by its own rules
// (`crates/loams/src/api/connect_query_ir.rs`) and refuses anything else.
//
// ## `filter` here is the typed IR; on the document RPCs it is still a `Value`
//
// `ScrollDocumentsRequest.filter` and its three siblings stay
// `google.protobuf.Value`, and `document.proto` says why: Task 3 shipped them
// that way and eighteen green tests send the bare string `"match_all"`, which a
// typed oneof cannot spell. `SearchRequest.filter` is the typed [`Query`], so
// this is the one filter on the package a caller cannot degrade by sending a
// string the handler did not read.
//
// ## What stays a `Struct` or a `Value` here, and why
//
// The IR's open parts stay open, for `document.proto`'s reasons:
// `SearchRequest.select` (a projection's `source` has a JSON object form),
// `SearchRequest.fuse` (above), `SparseQuery.query` (a sparse vector is
// `{"indices": […], "values": […]}`) and `SearchRequest.aggregations` (an
// ES aggregation request, which Tantivy's own `Aggregations` owns). A field
// value, a sort value and a hit's `highlight`/`fields` entry are `Value`s
// because the IR's own JSON for one is a scalar or `{"date": "…"}` /
// `{"uuid": "…"}` (Ruling 9), and all three spellings fit a `Value`.
//
// ## Why `ScrollDocuments` stays unary
//
// Plan ruling 2 offers `page_token` as an *addition* to the shipped unary
// `DocumentService/ScrollDocuments`, and Q614 (server-streaming versus unary
// pagination) is unanswered. This file therefore does not change that RPC's
// streaming-ness — eighteen shipped tests would break for an open question —
// and `document.proto` gains `ScrollDocumentsRequest.page_token`, the
// AIP-158 spelling of the **same** `DocumentId` cursor `after` already is.
// True server-streaming, if Q614 says so, is a separate additive RPC.
//
// ## The handlers are thin
//
// `Search` calls the same `CollectionService::search` the REST route calls and
// nothing else: the request message is turned back into the native REST JSON
// and `loams_query::json::hybrid::parse_query_body` — the REST route's own
// parser — reads it, so a filter, a retriever, a fusion or a sort key cannot
// behave differently on the two surfaces. That is what
// `crates/loams/tests/connect_query.rs::routing_and_ranking_match_rest_for_every_fixture`
// checks, fifteen fixtures compared against each other rather than against a
// golden number.
//
// ## The SDK module, and why no method is in the SDK facade yet
//
// The service carries `loams.options.v1.ModuleOptions` (`loams.search`), which
// is what §44 §7.3 asks of every public package; §7.2 puts `loams.search` and
// `loams.vector` on this one RPC.
//
// No method carries `FacadeOptions`, which is plan ruling 2.5's shape again: a
// `facade` option makes `protoc-gen-loams-facade` emit a call, and the call
// imports the package's message types from the generated stubs, which means
// every SDK mirror has to generate `loams.collection.v1` first. That is SDK1's
// stub task, and Task 2 measured what going ahead of it costs.

// Code generated by protoc-gen-connect-go. DO NOT EDIT.
//
// Source: loams/collection/v1/query.proto

package collectionv1connect

import (
	connect "connectrpc.com/connect"
	context "context"
	errors "errors"
	v1 "loams.dev/go/gen/loams/collection/v1"
	http "net/http"
	strings "strings"
)

// This is a compile-time assertion to ensure that this generated file and the connect package are
// compatible. If you get a compiler error that this constant is not defined, this code was
// generated with a version of connect newer than the one compiled into your binary. You can fix the
// problem by either regenerating this code with an older version of connect or updating the connect
// version compiled into your binary.
const _ = connect.IsAtLeastVersion1_13_0

const (
	// QueryServiceName is the fully-qualified name of the QueryService service.
	QueryServiceName = "loams.collection.v1.QueryService"
)

// These constants are the fully-qualified names of the RPCs defined in this package. They're
// exposed at runtime as Spec.Procedure and as the final two segments of the HTTP route.
//
// Note that these are different from the fully-qualified method names used by
// google.golang.org/protobuf/reflect/protoreflect. To convert from these constants to
// reflection-formatted method names, remove the leading slash and convert the remaining slash to a
// period.
const (
	// QueryServiceSearchProcedure is the fully-qualified name of the QueryService's Search RPC.
	QueryServiceSearchProcedure = "/loams.collection.v1.QueryService/Search"
)

// QueryServiceClient is a client for the loams.collection.v1.QueryService service.
type QueryServiceClient interface {
	// One search over one collection. The whole IR is this request: the
	// retrievers to run, how to fuse them, the filter, the sort and the window.
	//
	// It is `NO_SIDE_EFFECTS` — it reads and counts — so an SDK may retry it
	// after a lost answer. It is still a `POST`, because a search is not free and
	// the request is a body.
	Search(context.Context, *connect.Request[v1.SearchRequest]) (*connect.Response[v1.SearchResponse], error)
}

// NewQueryServiceClient constructs a client for the loams.collection.v1.QueryService service. By
// default, it uses the Connect protocol with the binary Protobuf Codec, asks for gzipped responses,
// and sends uncompressed requests. To use the gRPC or gRPC-Web protocols, supply the
// connect.WithGRPC() or connect.WithGRPCWeb() options.
//
// The URL supplied here should be the base URL for the Connect or gRPC server (for example,
// http://api.acme.com or https://acme.com/grpc).
func NewQueryServiceClient(httpClient connect.HTTPClient, baseURL string, opts ...connect.ClientOption) QueryServiceClient {
	baseURL = strings.TrimRight(baseURL, "/")
	queryServiceMethods := v1.File_loams_collection_v1_query_proto.Services().ByName("QueryService").Methods()
	return &queryServiceClient{
		search: connect.NewClient[v1.SearchRequest, v1.SearchResponse](
			httpClient,
			baseURL+QueryServiceSearchProcedure,
			connect.WithSchema(queryServiceMethods.ByName("Search")),
			connect.WithIdempotency(connect.IdempotencyNoSideEffects),
			connect.WithClientOptions(opts...),
		),
	}
}

// queryServiceClient implements QueryServiceClient.
type queryServiceClient struct {
	search *connect.Client[v1.SearchRequest, v1.SearchResponse]
}

// Search calls loams.collection.v1.QueryService.Search.
func (c *queryServiceClient) Search(ctx context.Context, req *connect.Request[v1.SearchRequest]) (*connect.Response[v1.SearchResponse], error) {
	return c.search.CallUnary(ctx, req)
}

// QueryServiceHandler is an implementation of the loams.collection.v1.QueryService service.
type QueryServiceHandler interface {
	// One search over one collection. The whole IR is this request: the
	// retrievers to run, how to fuse them, the filter, the sort and the window.
	//
	// It is `NO_SIDE_EFFECTS` — it reads and counts — so an SDK may retry it
	// after a lost answer. It is still a `POST`, because a search is not free and
	// the request is a body.
	Search(context.Context, *connect.Request[v1.SearchRequest]) (*connect.Response[v1.SearchResponse], error)
}

// NewQueryServiceHandler builds an HTTP handler from the service implementation. It returns the
// path on which to mount the handler and the handler itself.
//
// By default, handlers support the Connect, gRPC, and gRPC-Web protocols with the binary Protobuf
// and JSON codecs. They also support gzip compression.
func NewQueryServiceHandler(svc QueryServiceHandler, opts ...connect.HandlerOption) (string, http.Handler) {
	queryServiceMethods := v1.File_loams_collection_v1_query_proto.Services().ByName("QueryService").Methods()
	queryServiceSearchHandler := connect.NewUnaryHandler(
		QueryServiceSearchProcedure,
		svc.Search,
		connect.WithSchema(queryServiceMethods.ByName("Search")),
		connect.WithIdempotency(connect.IdempotencyNoSideEffects),
		connect.WithHandlerOptions(opts...),
	)
	return "/loams.collection.v1.QueryService/", http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch r.URL.Path {
		case QueryServiceSearchProcedure:
			queryServiceSearchHandler.ServeHTTP(w, r)
		default:
			http.NotFound(w, r)
		}
	})
}

// UnimplementedQueryServiceHandler returns CodeUnimplemented from all methods.
type UnimplementedQueryServiceHandler struct{}

func (UnimplementedQueryServiceHandler) Search(context.Context, *connect.Request[v1.SearchRequest]) (*connect.Response[v1.SearchResponse], error) {
	return nil, connect.NewError(connect.CodeUnimplemented, errors.New("loams.collection.v1.QueryService.Search is not implemented"))
}
