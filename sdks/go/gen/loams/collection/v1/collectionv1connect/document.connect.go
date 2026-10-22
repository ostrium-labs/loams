// Documents: the write, the three reads and the two filter writes (design §44
// §5.1, D602; API1 Task 3).
//
// This package replaces the M1.2/M1.5 native REST routes under
// `/v1/namespaces/{ns}/collections/{c}/documents` with RPCs on the one port
// (design §44 §4), and the plan's rulings are visible on the wire:
//
// - **Names are fields, not path segments.** A Connect path is
//   `/<package>.<Service>/<Method>`, so `namespace` and `collection` travel
//   in every request message and one path serves every collection (plan
//   ruling 4). A collection is named by its name or by an alias, exactly as
//   the REST route named it.
// - **proto3 JSON is the wire form**: field names are `lowerCamelCase`
//   (`report_existence` is `reportExistence`, `max_rows` is `maxRows`), an
//   enum answers its proto name in `UPPER_SNAKE`, a 64-bit integer answers a
//   decimal string (so it stays lossless in JavaScript), and a field at its
//   default is *absent* rather than `0`, `false`, `""` or `[]`.
// - **Unknown fields are ignored, not refused.** proto3 JSON says to skip a
//   field the message does not declare, so the REST routes' `deny_unknown_fields`
//   refusal (`{"patch": {}}` on a delete) has no spelling here. What a caller
//   can still be refused for is a *known* field of the wrong JSON type, and the
//   codec answers `invalid_argument` for it before a handler runs.
// - **Every failure carries a reason.** A failed RPC carries one
//   `loams.errors.v1.ErrorInfo` in its details whose `reason` is registered
//   in `docs/api/reasons.md`; a caller branches on the reason, never on the
//   message (D611). `index` names the op of a rejected write and `matched`
//   and `limit` the numbers a filter write refused on, so neither needs the
//   prose. Every reason this package raises is a registry row already.
//
// ## `WriteDocuments` is the first RPC that needs an idempotency key
//
// Every RPC in `loams.collection.v1` is idempotent by construction, so none
// of them carries one (plan ruling 2.4). A write is not: a client that
// retries after a lost answer would write twice. `idempotency_key` makes the
// retry a *replay* — the same token, the same per-op results and the same
// positions, which is what makes the operation safe to repeat at all (D610).
// The window is the immediate retry, and it is documented as such in
// `crates/loams/src/api/connect_documents.rs`; a caller that needs a longer
// window than the server keeps gets a `not_found`-shaped miss (a fresh write),
// not a wrong answer.
//
// ## The payload is JSON, deliberately
//
// A document's `source` is `google.protobuf.Struct`, and its vectors are
// `map<string, google.protobuf.Value>`. A document's payload is an open
// document: a new source key per application, and a vector shape no proto can
// enumerate (dense, sparse, multi-vector, learned). A `Struct` and a `Value`
// keep the native REST route's document JSON exactly, so moving a caller onto
// this RPC changes the envelope it posts in, not the document it posts — the
// same call `loams.collection.v1` made for a schema (plan ruling 2.1). The
// one cost is a `null`: a patch removes a vector by writing
// `{"embedding": null}`, and that is representable at all only because the
// value is a `Value` rather than a `ListValue`.
//
// `Document.fields` is a `Value` per name for the same reason: a field value
// is one of a string, a bool, an integer, a float or `{"date": "…"}` (Ruling
// 9), and the REST JSON already spells all five.
//
// ## A document id is a typed oneof
//
// `DocumentId` is a oneof of `uint` / `string` / `uuid`, not a
// `google.protobuf.Value`. A `Value` holds every number in a `double`, so a
// `u64` id would come back as `18446744073709552000` — a different document.
// The three arms are the three `PrimaryKey` variants, and `uuid` keeps the
// REST spelling `{"uuid": "0190f5c4-…"}` verbatim, which is why it is a
// separate arm rather than the `string` one.
//
// The proto3 JSON of a oneof is `{"uint": "1"}` / `{"string": "k-str"}` /
// `{"uuid": "…"}`: a bare JSON `1` has no oneof spelling, so each arm is named
// on the wire. Setting two arms at once is a decode failure.
//
// ## `filter` is a `Value`, not the typed IR
//
// The typed filter IR is API1 Task 4. Until it lands, `filter` is the native
// query JSON verbatim (`{"term": {"field": "tenant", "value": "c"}}`,
// `"match_all"`), which is what the REST route took and what a caller already
// has. `select` is a `Struct` for the same reason and because a projection's
// `source` has a JSON object form (`{"include": [], "exclude": []}`) that a
// `string` cannot hold.
//
// ## Where the tokens and the backlog live
//
// A write answers its consistency token in `token` and in the
// `loams-consistency-token` response header; a read answers `readToken` and
// the same header. The unapplied-data backlog (`loams-unapplied-records`,
// `loams-unapplied-bytes`) stays response headers, because that is the REST
// behaviour and a response half is headers by rule (design §44 §7.4). The one
// request header that stays is `loams-backpressure: off`, the write-budget
// override (M1.3 Task 15 rule 5): it is a transport-level switch with no
// message-level meaning, and an unrecognised value is `invalid_argument`.
//
// ## The handlers are thin
//
// Each one calls the same `CollectionService` entry point the REST route it
// replaces calls — `write`, `get_with_token`, `scroll_with_token`,
// `count_with_token`, `delete_by_filter`, `patch_by_filter` — and reuses the
// REST request's own parsing (`op_from_json`, `patch_spec_from_json`) by
// turning the request message back into the JSON those parse, so the two
// surfaces cannot drift. The behaviour is the behaviour
// `crates/loams/tests/it/` already pins; the REST routes stay until Task 9.
//
// ## Why this is a second file in `loams.collection.v1`, not its own package
//
// §44 §7.2's module catalogue puts both services in one package —
// `loams.collections`, `loams.documents` ⇢ `loams.collection.v1`
// `CollectionService`, `DocumentService` — and §8's `New protos:` sentence
// enumerates `loams.collection.v1` as covering Namespace, Collection,
// **Document** and Query. One package, four concerns, several files: the same
// shape `proto/loams/live/v1/` already uses with its five files. An earlier
// draft gave documents a `loams.document.v1` of their own, which no design
// section mentions; `crates/loams/tests/route_map.rs` now fails if any proto or
// `CATALOGUE` row declares a package §44 does not enumerate.
//
// Splitting the files still matters — `collection.proto` is the schema and scan
// plan, this file is the document data path, and they are read separately — but
// the package is the unit on the wire, in the SDK stubs and in `CATALOGUE`, and
// §44 fixes it.
//
// ## The SDK module, and why no method is in the SDK facade yet
//
// The service carries `loams.options.v1.ModuleOptions` (`loams.documents`),
// which is what §44 §7.3 asks of every public package. Note this file is in
// `loams.collection.v1`, so the module a caller reaches is named by the
// `module` option here, not by the package name.
//
// No method carries `FacadeOptions`, which is plan ruling 2.5's shape again:
// a `facade` option makes `protoc-gen-loams-facade` emit a call, and the call
// imports the package's message types from the generated stubs, which means
// every SDK mirror has to generate `loams.collection.v1` first. That is SDK1's
// stub task. Sharing the package also means the generated facades barely move:
// `PROTO_PACKAGES` already listed `loams.collection.v1`, so what changed is that
// a `loams.document.v1` line went away. `facade.ts`, `facade.py`, `facade.rs`
// and the three goldens are regenerated with it.

// Code generated by protoc-gen-connect-go. DO NOT EDIT.
//
// Source: loams/collection/v1/document.proto

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
	// DocumentServiceName is the fully-qualified name of the DocumentService service.
	DocumentServiceName = "loams.collection.v1.DocumentService"
)

// These constants are the fully-qualified names of the RPCs defined in this package. They're
// exposed at runtime as Spec.Procedure and as the final two segments of the HTTP route.
//
// Note that these are different from the fully-qualified method names used by
// google.golang.org/protobuf/reflect/protoreflect. To convert from these constants to
// reflection-formatted method names, remove the leading slash and convert the remaining slash to a
// period.
const (
	// DocumentServiceWriteDocumentsProcedure is the fully-qualified name of the DocumentService's
	// WriteDocuments RPC.
	DocumentServiceWriteDocumentsProcedure = "/loams.collection.v1.DocumentService/WriteDocuments"
	// DocumentServiceGetDocumentsProcedure is the fully-qualified name of the DocumentService's
	// GetDocuments RPC.
	DocumentServiceGetDocumentsProcedure = "/loams.collection.v1.DocumentService/GetDocuments"
	// DocumentServiceScrollDocumentsProcedure is the fully-qualified name of the DocumentService's
	// ScrollDocuments RPC.
	DocumentServiceScrollDocumentsProcedure = "/loams.collection.v1.DocumentService/ScrollDocuments"
	// DocumentServiceCountDocumentsProcedure is the fully-qualified name of the DocumentService's
	// CountDocuments RPC.
	DocumentServiceCountDocumentsProcedure = "/loams.collection.v1.DocumentService/CountDocuments"
	// DocumentServiceDeleteByFilterProcedure is the fully-qualified name of the DocumentService's
	// DeleteByFilter RPC.
	DocumentServiceDeleteByFilterProcedure = "/loams.collection.v1.DocumentService/DeleteByFilter"
	// DocumentServicePatchByFilterProcedure is the fully-qualified name of the DocumentService's
	// PatchByFilter RPC.
	DocumentServicePatchByFilterProcedure = "/loams.collection.v1.DocumentService/PatchByFilter"
)

// DocumentServiceClient is a client for the loams.collection.v1.DocumentService service.
type DocumentServiceClient interface {
	// An atomic write: every op of a request is applied or none is (Ruling 16).
	//
	// This is the one RPC here that is not naturally idempotent, so it carries
	// `idempotency_key`; a repeat of a keyed write replays the first answer
	// instead of writing again. The write is refused with `resource_exhausted`
	// when it would take the collection's unapplied-data budget over its
	// ceiling, and every answer to an admitted *or* a refused write carries the
	// backlog headers.
	WriteDocuments(context.Context, *connect.Request[v1.WriteDocumentsRequest]) (*connect.Response[v1.WriteDocumentsResponse], error)
	// The documents of `ids`, in request order.
	//
	// A document that is not there is an **entry with no fields**, which sits at
	// the requested id's position: proto3 JSON has no `null` for an element of a
	// `repeated` field of messages, and an empty entry is the same information
	// — the entry is there and it is empty.
	GetDocuments(context.Context, *connect.Request[v1.GetDocumentsRequest]) (*connect.Response[v1.GetDocumentsResponse], error)
	// One page of the collection's documents in primary-key order, and the id to
	// continue after. `next` is absent on the last page.
	ScrollDocuments(context.Context, *connect.Request[v1.ScrollDocumentsRequest]) (*connect.Response[v1.ScrollDocumentsResponse], error)
	// How many documents match, which is a read and not a free one.
	CountDocuments(context.Context, *connect.Request[v1.CountDocumentsRequest]) (*connect.Response[v1.CountDocumentsResponse], error)
	// Deletes every document matching `filter`, in batches, at one pinned
	// snapshot (M1.5 Task 9a, D87).
	//
	// A call matching more than `max_rows` fails with `invalid_argument` and
	// deletes nothing unless `allow_partial`, in which case it writes what fits
	// and answers the `cursor` that finishes it at the same pin. The pin travels
	// in `consistency` too: an `at_least` token makes the pin at least as new as
	// the token.
	DeleteByFilter(context.Context, *connect.Request[v1.DeleteByFilterRequest]) (*connect.Response[v1.FilterWriteResponse], error)
	// The same as `DeleteByFilter` with a `patch` applied to each match.
	//
	// A filter write patches the documents the filter matched and never creates
	// one, so `patch.upsert` is refused.
	PatchByFilter(context.Context, *connect.Request[v1.PatchByFilterRequest]) (*connect.Response[v1.FilterWriteResponse], error)
}

// NewDocumentServiceClient constructs a client for the loams.collection.v1.DocumentService service.
// By default, it uses the Connect protocol with the binary Protobuf Codec, asks for gzipped
// responses, and sends uncompressed requests. To use the gRPC or gRPC-Web protocols, supply the
// connect.WithGRPC() or connect.WithGRPCWeb() options.
//
// The URL supplied here should be the base URL for the Connect or gRPC server (for example,
// http://api.acme.com or https://acme.com/grpc).
func NewDocumentServiceClient(httpClient connect.HTTPClient, baseURL string, opts ...connect.ClientOption) DocumentServiceClient {
	baseURL = strings.TrimRight(baseURL, "/")
	documentServiceMethods := v1.File_loams_collection_v1_document_proto.Services().ByName("DocumentService").Methods()
	return &documentServiceClient{
		writeDocuments: connect.NewClient[v1.WriteDocumentsRequest, v1.WriteDocumentsResponse](
			httpClient,
			baseURL+DocumentServiceWriteDocumentsProcedure,
			connect.WithSchema(documentServiceMethods.ByName("WriteDocuments")),
			connect.WithClientOptions(opts...),
		),
		getDocuments: connect.NewClient[v1.GetDocumentsRequest, v1.GetDocumentsResponse](
			httpClient,
			baseURL+DocumentServiceGetDocumentsProcedure,
			connect.WithSchema(documentServiceMethods.ByName("GetDocuments")),
			connect.WithIdempotency(connect.IdempotencyNoSideEffects),
			connect.WithClientOptions(opts...),
		),
		scrollDocuments: connect.NewClient[v1.ScrollDocumentsRequest, v1.ScrollDocumentsResponse](
			httpClient,
			baseURL+DocumentServiceScrollDocumentsProcedure,
			connect.WithSchema(documentServiceMethods.ByName("ScrollDocuments")),
			connect.WithIdempotency(connect.IdempotencyNoSideEffects),
			connect.WithClientOptions(opts...),
		),
		countDocuments: connect.NewClient[v1.CountDocumentsRequest, v1.CountDocumentsResponse](
			httpClient,
			baseURL+DocumentServiceCountDocumentsProcedure,
			connect.WithSchema(documentServiceMethods.ByName("CountDocuments")),
			connect.WithIdempotency(connect.IdempotencyNoSideEffects),
			connect.WithClientOptions(opts...),
		),
		deleteByFilter: connect.NewClient[v1.DeleteByFilterRequest, v1.FilterWriteResponse](
			httpClient,
			baseURL+DocumentServiceDeleteByFilterProcedure,
			connect.WithSchema(documentServiceMethods.ByName("DeleteByFilter")),
			connect.WithClientOptions(opts...),
		),
		patchByFilter: connect.NewClient[v1.PatchByFilterRequest, v1.FilterWriteResponse](
			httpClient,
			baseURL+DocumentServicePatchByFilterProcedure,
			connect.WithSchema(documentServiceMethods.ByName("PatchByFilter")),
			connect.WithClientOptions(opts...),
		),
	}
}

// documentServiceClient implements DocumentServiceClient.
type documentServiceClient struct {
	writeDocuments  *connect.Client[v1.WriteDocumentsRequest, v1.WriteDocumentsResponse]
	getDocuments    *connect.Client[v1.GetDocumentsRequest, v1.GetDocumentsResponse]
	scrollDocuments *connect.Client[v1.ScrollDocumentsRequest, v1.ScrollDocumentsResponse]
	countDocuments  *connect.Client[v1.CountDocumentsRequest, v1.CountDocumentsResponse]
	deleteByFilter  *connect.Client[v1.DeleteByFilterRequest, v1.FilterWriteResponse]
	patchByFilter   *connect.Client[v1.PatchByFilterRequest, v1.FilterWriteResponse]
}

// WriteDocuments calls loams.collection.v1.DocumentService.WriteDocuments.
func (c *documentServiceClient) WriteDocuments(ctx context.Context, req *connect.Request[v1.WriteDocumentsRequest]) (*connect.Response[v1.WriteDocumentsResponse], error) {
	return c.writeDocuments.CallUnary(ctx, req)
}

// GetDocuments calls loams.collection.v1.DocumentService.GetDocuments.
func (c *documentServiceClient) GetDocuments(ctx context.Context, req *connect.Request[v1.GetDocumentsRequest]) (*connect.Response[v1.GetDocumentsResponse], error) {
	return c.getDocuments.CallUnary(ctx, req)
}

// ScrollDocuments calls loams.collection.v1.DocumentService.ScrollDocuments.
func (c *documentServiceClient) ScrollDocuments(ctx context.Context, req *connect.Request[v1.ScrollDocumentsRequest]) (*connect.Response[v1.ScrollDocumentsResponse], error) {
	return c.scrollDocuments.CallUnary(ctx, req)
}

// CountDocuments calls loams.collection.v1.DocumentService.CountDocuments.
func (c *documentServiceClient) CountDocuments(ctx context.Context, req *connect.Request[v1.CountDocumentsRequest]) (*connect.Response[v1.CountDocumentsResponse], error) {
	return c.countDocuments.CallUnary(ctx, req)
}

// DeleteByFilter calls loams.collection.v1.DocumentService.DeleteByFilter.
func (c *documentServiceClient) DeleteByFilter(ctx context.Context, req *connect.Request[v1.DeleteByFilterRequest]) (*connect.Response[v1.FilterWriteResponse], error) {
	return c.deleteByFilter.CallUnary(ctx, req)
}

// PatchByFilter calls loams.collection.v1.DocumentService.PatchByFilter.
func (c *documentServiceClient) PatchByFilter(ctx context.Context, req *connect.Request[v1.PatchByFilterRequest]) (*connect.Response[v1.FilterWriteResponse], error) {
	return c.patchByFilter.CallUnary(ctx, req)
}

// DocumentServiceHandler is an implementation of the loams.collection.v1.DocumentService service.
type DocumentServiceHandler interface {
	// An atomic write: every op of a request is applied or none is (Ruling 16).
	//
	// This is the one RPC here that is not naturally idempotent, so it carries
	// `idempotency_key`; a repeat of a keyed write replays the first answer
	// instead of writing again. The write is refused with `resource_exhausted`
	// when it would take the collection's unapplied-data budget over its
	// ceiling, and every answer to an admitted *or* a refused write carries the
	// backlog headers.
	WriteDocuments(context.Context, *connect.Request[v1.WriteDocumentsRequest]) (*connect.Response[v1.WriteDocumentsResponse], error)
	// The documents of `ids`, in request order.
	//
	// A document that is not there is an **entry with no fields**, which sits at
	// the requested id's position: proto3 JSON has no `null` for an element of a
	// `repeated` field of messages, and an empty entry is the same information
	// — the entry is there and it is empty.
	GetDocuments(context.Context, *connect.Request[v1.GetDocumentsRequest]) (*connect.Response[v1.GetDocumentsResponse], error)
	// One page of the collection's documents in primary-key order, and the id to
	// continue after. `next` is absent on the last page.
	ScrollDocuments(context.Context, *connect.Request[v1.ScrollDocumentsRequest]) (*connect.Response[v1.ScrollDocumentsResponse], error)
	// How many documents match, which is a read and not a free one.
	CountDocuments(context.Context, *connect.Request[v1.CountDocumentsRequest]) (*connect.Response[v1.CountDocumentsResponse], error)
	// Deletes every document matching `filter`, in batches, at one pinned
	// snapshot (M1.5 Task 9a, D87).
	//
	// A call matching more than `max_rows` fails with `invalid_argument` and
	// deletes nothing unless `allow_partial`, in which case it writes what fits
	// and answers the `cursor` that finishes it at the same pin. The pin travels
	// in `consistency` too: an `at_least` token makes the pin at least as new as
	// the token.
	DeleteByFilter(context.Context, *connect.Request[v1.DeleteByFilterRequest]) (*connect.Response[v1.FilterWriteResponse], error)
	// The same as `DeleteByFilter` with a `patch` applied to each match.
	//
	// A filter write patches the documents the filter matched and never creates
	// one, so `patch.upsert` is refused.
	PatchByFilter(context.Context, *connect.Request[v1.PatchByFilterRequest]) (*connect.Response[v1.FilterWriteResponse], error)
}

// NewDocumentServiceHandler builds an HTTP handler from the service implementation. It returns the
// path on which to mount the handler and the handler itself.
//
// By default, handlers support the Connect, gRPC, and gRPC-Web protocols with the binary Protobuf
// and JSON codecs. They also support gzip compression.
func NewDocumentServiceHandler(svc DocumentServiceHandler, opts ...connect.HandlerOption) (string, http.Handler) {
	documentServiceMethods := v1.File_loams_collection_v1_document_proto.Services().ByName("DocumentService").Methods()
	documentServiceWriteDocumentsHandler := connect.NewUnaryHandler(
		DocumentServiceWriteDocumentsProcedure,
		svc.WriteDocuments,
		connect.WithSchema(documentServiceMethods.ByName("WriteDocuments")),
		connect.WithHandlerOptions(opts...),
	)
	documentServiceGetDocumentsHandler := connect.NewUnaryHandler(
		DocumentServiceGetDocumentsProcedure,
		svc.GetDocuments,
		connect.WithSchema(documentServiceMethods.ByName("GetDocuments")),
		connect.WithIdempotency(connect.IdempotencyNoSideEffects),
		connect.WithHandlerOptions(opts...),
	)
	documentServiceScrollDocumentsHandler := connect.NewUnaryHandler(
		DocumentServiceScrollDocumentsProcedure,
		svc.ScrollDocuments,
		connect.WithSchema(documentServiceMethods.ByName("ScrollDocuments")),
		connect.WithIdempotency(connect.IdempotencyNoSideEffects),
		connect.WithHandlerOptions(opts...),
	)
	documentServiceCountDocumentsHandler := connect.NewUnaryHandler(
		DocumentServiceCountDocumentsProcedure,
		svc.CountDocuments,
		connect.WithSchema(documentServiceMethods.ByName("CountDocuments")),
		connect.WithIdempotency(connect.IdempotencyNoSideEffects),
		connect.WithHandlerOptions(opts...),
	)
	documentServiceDeleteByFilterHandler := connect.NewUnaryHandler(
		DocumentServiceDeleteByFilterProcedure,
		svc.DeleteByFilter,
		connect.WithSchema(documentServiceMethods.ByName("DeleteByFilter")),
		connect.WithHandlerOptions(opts...),
	)
	documentServicePatchByFilterHandler := connect.NewUnaryHandler(
		DocumentServicePatchByFilterProcedure,
		svc.PatchByFilter,
		connect.WithSchema(documentServiceMethods.ByName("PatchByFilter")),
		connect.WithHandlerOptions(opts...),
	)
	return "/loams.collection.v1.DocumentService/", http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch r.URL.Path {
		case DocumentServiceWriteDocumentsProcedure:
			documentServiceWriteDocumentsHandler.ServeHTTP(w, r)
		case DocumentServiceGetDocumentsProcedure:
			documentServiceGetDocumentsHandler.ServeHTTP(w, r)
		case DocumentServiceScrollDocumentsProcedure:
			documentServiceScrollDocumentsHandler.ServeHTTP(w, r)
		case DocumentServiceCountDocumentsProcedure:
			documentServiceCountDocumentsHandler.ServeHTTP(w, r)
		case DocumentServiceDeleteByFilterProcedure:
			documentServiceDeleteByFilterHandler.ServeHTTP(w, r)
		case DocumentServicePatchByFilterProcedure:
			documentServicePatchByFilterHandler.ServeHTTP(w, r)
		default:
			http.NotFound(w, r)
		}
	})
}

// UnimplementedDocumentServiceHandler returns CodeUnimplemented from all methods.
type UnimplementedDocumentServiceHandler struct{}

func (UnimplementedDocumentServiceHandler) WriteDocuments(context.Context, *connect.Request[v1.WriteDocumentsRequest]) (*connect.Response[v1.WriteDocumentsResponse], error) {
	return nil, connect.NewError(connect.CodeUnimplemented, errors.New("loams.collection.v1.DocumentService.WriteDocuments is not implemented"))
}

func (UnimplementedDocumentServiceHandler) GetDocuments(context.Context, *connect.Request[v1.GetDocumentsRequest]) (*connect.Response[v1.GetDocumentsResponse], error) {
	return nil, connect.NewError(connect.CodeUnimplemented, errors.New("loams.collection.v1.DocumentService.GetDocuments is not implemented"))
}

func (UnimplementedDocumentServiceHandler) ScrollDocuments(context.Context, *connect.Request[v1.ScrollDocumentsRequest]) (*connect.Response[v1.ScrollDocumentsResponse], error) {
	return nil, connect.NewError(connect.CodeUnimplemented, errors.New("loams.collection.v1.DocumentService.ScrollDocuments is not implemented"))
}

func (UnimplementedDocumentServiceHandler) CountDocuments(context.Context, *connect.Request[v1.CountDocumentsRequest]) (*connect.Response[v1.CountDocumentsResponse], error) {
	return nil, connect.NewError(connect.CodeUnimplemented, errors.New("loams.collection.v1.DocumentService.CountDocuments is not implemented"))
}

func (UnimplementedDocumentServiceHandler) DeleteByFilter(context.Context, *connect.Request[v1.DeleteByFilterRequest]) (*connect.Response[v1.FilterWriteResponse], error) {
	return nil, connect.NewError(connect.CodeUnimplemented, errors.New("loams.collection.v1.DocumentService.DeleteByFilter is not implemented"))
}

func (UnimplementedDocumentServiceHandler) PatchByFilter(context.Context, *connect.Request[v1.PatchByFilterRequest]) (*connect.Response[v1.FilterWriteResponse], error) {
	return nil, connect.NewError(connect.CodeUnimplemented, errors.New("loams.collection.v1.DocumentService.PatchByFilter is not implemented"))
}
