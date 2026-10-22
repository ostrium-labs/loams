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

// Code generated by protoc-gen-go. DO NOT EDIT.
// versions:
// 	protoc-gen-go v1.36.11
// 	protoc        (unknown)
// source: loams/collection/v1/query.proto

package collectionv1

import (
	protoreflect "google.golang.org/protobuf/reflect/protoreflect"
	protoimpl "google.golang.org/protobuf/runtime/protoimpl"
	emptypb "google.golang.org/protobuf/types/known/emptypb"
	structpb "google.golang.org/protobuf/types/known/structpb"
	_ "loams.dev/go/gen/loams/options/v1"
	reflect "reflect"
	sync "sync"
	unsafe "unsafe"
)

const (
	// Verify that this generated code is sufficiently up-to-date.
	_ = protoimpl.EnforceVersion(20 - protoimpl.MinVersion)
	// Verify that runtime/protoimpl is sufficiently up-to-date.
	_ = protoimpl.EnforceVersion(protoimpl.MaxVersion - 20)
)

// `or` / `and`, as the IR spells them.
//
// Declared for the two fields whose operator the IR spells differently from the
// fields above: `MultiMatchQuery.operator` and
// `QueryStringQuery.default_operator`, which keep this enum and therefore
// answer (and are sent) `BOOL_OPERATOR_OR` / `BOOL_OPERATOR_AND`.
type BoolOperator int32

const (
	// `or`, which is the IR's default for every operator that defaults.
	BoolOperator_BOOL_OPERATOR_UNSPECIFIED BoolOperator = 0
	BoolOperator_BOOL_OPERATOR_OR          BoolOperator = 1
	BoolOperator_BOOL_OPERATOR_AND         BoolOperator = 2
)

// Enum value maps for BoolOperator.
var (
	BoolOperator_name = map[int32]string{
		0: "BOOL_OPERATOR_UNSPECIFIED",
		1: "BOOL_OPERATOR_OR",
		2: "BOOL_OPERATOR_AND",
	}
	BoolOperator_value = map[string]int32{
		"BOOL_OPERATOR_UNSPECIFIED": 0,
		"BOOL_OPERATOR_OR":          1,
		"BOOL_OPERATOR_AND":         2,
	}
)

func (x BoolOperator) Enum() *BoolOperator {
	p := new(BoolOperator)
	*p = x
	return p
}

func (x BoolOperator) String() string {
	return protoimpl.X.EnumStringOf(x.Descriptor(), protoreflect.EnumNumber(x))
}

func (BoolOperator) Descriptor() protoreflect.EnumDescriptor {
	return file_loams_collection_v1_query_proto_enumTypes[0].Descriptor()
}

func (BoolOperator) Type() protoreflect.EnumType {
	return &file_loams_collection_v1_query_proto_enumTypes[0]
}

func (x BoolOperator) Number() protoreflect.EnumNumber {
	return protoreflect.EnumNumber(x)
}

// Deprecated: Use BoolOperator.Descriptor instead.
func (BoolOperator) EnumDescriptor() ([]byte, []int) {
	return file_loams_collection_v1_query_proto_rawDescGZIP(), []int{0}
}

type MultiMatchKind int32

const (
	// `best_fields`, the IR's default.
	MultiMatchKind_MULTI_MATCH_KIND_UNSPECIFIED   MultiMatchKind = 0
	MultiMatchKind_MULTI_MATCH_KIND_BEST_FIELDS   MultiMatchKind = 1
	MultiMatchKind_MULTI_MATCH_KIND_MOST_FIELDS   MultiMatchKind = 2
	MultiMatchKind_MULTI_MATCH_KIND_CROSS_FIELDS  MultiMatchKind = 3
	MultiMatchKind_MULTI_MATCH_KIND_PHRASE        MultiMatchKind = 4
	MultiMatchKind_MULTI_MATCH_KIND_PHRASE_PREFIX MultiMatchKind = 5
)

// Enum value maps for MultiMatchKind.
var (
	MultiMatchKind_name = map[int32]string{
		0: "MULTI_MATCH_KIND_UNSPECIFIED",
		1: "MULTI_MATCH_KIND_BEST_FIELDS",
		2: "MULTI_MATCH_KIND_MOST_FIELDS",
		3: "MULTI_MATCH_KIND_CROSS_FIELDS",
		4: "MULTI_MATCH_KIND_PHRASE",
		5: "MULTI_MATCH_KIND_PHRASE_PREFIX",
	}
	MultiMatchKind_value = map[string]int32{
		"MULTI_MATCH_KIND_UNSPECIFIED":   0,
		"MULTI_MATCH_KIND_BEST_FIELDS":   1,
		"MULTI_MATCH_KIND_MOST_FIELDS":   2,
		"MULTI_MATCH_KIND_CROSS_FIELDS":  3,
		"MULTI_MATCH_KIND_PHRASE":        4,
		"MULTI_MATCH_KIND_PHRASE_PREFIX": 5,
	}
)

func (x MultiMatchKind) Enum() *MultiMatchKind {
	p := new(MultiMatchKind)
	*p = x
	return p
}

func (x MultiMatchKind) String() string {
	return protoimpl.X.EnumStringOf(x.Descriptor(), protoreflect.EnumNumber(x))
}

func (MultiMatchKind) Descriptor() protoreflect.EnumDescriptor {
	return file_loams_collection_v1_query_proto_enumTypes[1].Descriptor()
}

func (MultiMatchKind) Type() protoreflect.EnumType {
	return &file_loams_collection_v1_query_proto_enumTypes[1]
}

func (x MultiMatchKind) Number() protoreflect.EnumNumber {
	return protoreflect.EnumNumber(x)
}

// Deprecated: Use MultiMatchKind.Descriptor instead.
func (MultiMatchKind) EnumDescriptor() ([]byte, []int) {
	return file_loams_collection_v1_query_proto_rawDescGZIP(), []int{1}
}

type SortOrder int32

const (
	// Ascending for a key and a field, descending for a score; each is the IR's
	// default for that key.
	SortOrder_SORT_ORDER_UNSPECIFIED SortOrder = 0
	SortOrder_SORT_ORDER_ASC         SortOrder = 1
	SortOrder_SORT_ORDER_DESC        SortOrder = 2
)

// Enum value maps for SortOrder.
var (
	SortOrder_name = map[int32]string{
		0: "SORT_ORDER_UNSPECIFIED",
		1: "SORT_ORDER_ASC",
		2: "SORT_ORDER_DESC",
	}
	SortOrder_value = map[string]int32{
		"SORT_ORDER_UNSPECIFIED": 0,
		"SORT_ORDER_ASC":         1,
		"SORT_ORDER_DESC":        2,
	}
)

func (x SortOrder) Enum() *SortOrder {
	p := new(SortOrder)
	*p = x
	return p
}

func (x SortOrder) String() string {
	return protoimpl.X.EnumStringOf(x.Descriptor(), protoreflect.EnumNumber(x))
}

func (SortOrder) Descriptor() protoreflect.EnumDescriptor {
	return file_loams_collection_v1_query_proto_enumTypes[2].Descriptor()
}

func (SortOrder) Type() protoreflect.EnumType {
	return &file_loams_collection_v1_query_proto_enumTypes[2]
}

func (x SortOrder) Number() protoreflect.EnumNumber {
	return protoreflect.EnumNumber(x)
}

// Deprecated: Use SortOrder.Descriptor instead.
func (SortOrder) EnumDescriptor() ([]byte, []int) {
	return file_loams_collection_v1_query_proto_rawDescGZIP(), []int{2}
}

type MissingOrder int32

const (
	// Last, which is the IR's default.
	MissingOrder_MISSING_ORDER_UNSPECIFIED MissingOrder = 0
	MissingOrder_MISSING_ORDER_FIRST       MissingOrder = 1
	MissingOrder_MISSING_ORDER_LAST        MissingOrder = 2
)

// Enum value maps for MissingOrder.
var (
	MissingOrder_name = map[int32]string{
		0: "MISSING_ORDER_UNSPECIFIED",
		1: "MISSING_ORDER_FIRST",
		2: "MISSING_ORDER_LAST",
	}
	MissingOrder_value = map[string]int32{
		"MISSING_ORDER_UNSPECIFIED": 0,
		"MISSING_ORDER_FIRST":       1,
		"MISSING_ORDER_LAST":        2,
	}
)

func (x MissingOrder) Enum() *MissingOrder {
	p := new(MissingOrder)
	*p = x
	return p
}

func (x MissingOrder) String() string {
	return protoimpl.X.EnumStringOf(x.Descriptor(), protoreflect.EnumNumber(x))
}

func (MissingOrder) Descriptor() protoreflect.EnumDescriptor {
	return file_loams_collection_v1_query_proto_enumTypes[3].Descriptor()
}

func (MissingOrder) Type() protoreflect.EnumType {
	return &file_loams_collection_v1_query_proto_enumTypes[3]
}

func (x MissingOrder) Number() protoreflect.EnumNumber {
	return protoreflect.EnumNumber(x)
}

// Deprecated: Use MissingOrder.Descriptor instead.
func (MissingOrder) EnumDescriptor() ([]byte, []int) {
	return file_loams_collection_v1_query_proto_rawDescGZIP(), []int{3}
}

// Which way an expansion walks the edges.
type ExpandDirection int32

const (
	ExpandDirection_EXPAND_DIRECTION_UNSPECIFIED ExpandDirection = 0
	ExpandDirection_EXPAND_DIRECTION_OUT         ExpandDirection = 1
	ExpandDirection_EXPAND_DIRECTION_IN          ExpandDirection = 2
	ExpandDirection_EXPAND_DIRECTION_BOTH        ExpandDirection = 3
)

// Enum value maps for ExpandDirection.
var (
	ExpandDirection_name = map[int32]string{
		0: "EXPAND_DIRECTION_UNSPECIFIED",
		1: "EXPAND_DIRECTION_OUT",
		2: "EXPAND_DIRECTION_IN",
		3: "EXPAND_DIRECTION_BOTH",
	}
	ExpandDirection_value = map[string]int32{
		"EXPAND_DIRECTION_UNSPECIFIED": 0,
		"EXPAND_DIRECTION_OUT":         1,
		"EXPAND_DIRECTION_IN":          2,
		"EXPAND_DIRECTION_BOTH":        3,
	}
)

func (x ExpandDirection) Enum() *ExpandDirection {
	p := new(ExpandDirection)
	*p = x
	return p
}

func (x ExpandDirection) String() string {
	return protoimpl.X.EnumStringOf(x.Descriptor(), protoreflect.EnumNumber(x))
}

func (ExpandDirection) Descriptor() protoreflect.EnumDescriptor {
	return file_loams_collection_v1_query_proto_enumTypes[4].Descriptor()
}

func (ExpandDirection) Type() protoreflect.EnumType {
	return &file_loams_collection_v1_query_proto_enumTypes[4]
}

func (x ExpandDirection) Number() protoreflect.EnumNumber {
	return protoreflect.EnumNumber(x)
}

// Deprecated: Use ExpandDirection.Descriptor instead.
func (ExpandDirection) EnumDescriptor() ([]byte, []int) {
	return file_loams_collection_v1_query_proto_rawDescGZIP(), []int{4}
}

// How the neighbors come back.
type ExpandOutput int32

const (
	ExpandOutput_EXPAND_OUTPUT_UNSPECIFIED ExpandOutput = 0
	// Under each seed hit, in `Hit.neighbors`.
	ExpandOutput_EXPAND_OUTPUT_NESTED ExpandOutput = 1
	// Seeds and neighbors in one ranked list, each neighbor a hit with
	// `Hit.graph`.
	ExpandOutput_EXPAND_OUTPUT_FLAT ExpandOutput = 2
)

// Enum value maps for ExpandOutput.
var (
	ExpandOutput_name = map[int32]string{
		0: "EXPAND_OUTPUT_UNSPECIFIED",
		1: "EXPAND_OUTPUT_NESTED",
		2: "EXPAND_OUTPUT_FLAT",
	}
	ExpandOutput_value = map[string]int32{
		"EXPAND_OUTPUT_UNSPECIFIED": 0,
		"EXPAND_OUTPUT_NESTED":      1,
		"EXPAND_OUTPUT_FLAT":        2,
	}
)

func (x ExpandOutput) Enum() *ExpandOutput {
	p := new(ExpandOutput)
	*p = x
	return p
}

func (x ExpandOutput) String() string {
	return protoimpl.X.EnumStringOf(x.Descriptor(), protoreflect.EnumNumber(x))
}

func (ExpandOutput) Descriptor() protoreflect.EnumDescriptor {
	return file_loams_collection_v1_query_proto_enumTypes[5].Descriptor()
}

func (ExpandOutput) Type() protoreflect.EnumType {
	return &file_loams_collection_v1_query_proto_enumTypes[5]
}

func (x ExpandOutput) Number() protoreflect.EnumNumber {
	return protoreflect.EnumNumber(x)
}

// Deprecated: Use ExpandOutput.Descriptor instead.
func (ExpandOutput) EnumDescriptor() ([]byte, []int) {
	return file_loams_collection_v1_query_proto_rawDescGZIP(), []int{5}
}

type TotalRelation int32

const (
	TotalRelation_TOTAL_RELATION_UNSPECIFIED TotalRelation = 0
	// `eq`: the count is exact.
	TotalRelation_TOTAL_RELATION_EQ TotalRelation = 1
	// `gte`: at least this many matched.
	TotalRelation_TOTAL_RELATION_GTE TotalRelation = 2
)

// Enum value maps for TotalRelation.
var (
	TotalRelation_name = map[int32]string{
		0: "TOTAL_RELATION_UNSPECIFIED",
		1: "TOTAL_RELATION_EQ",
		2: "TOTAL_RELATION_GTE",
	}
	TotalRelation_value = map[string]int32{
		"TOTAL_RELATION_UNSPECIFIED": 0,
		"TOTAL_RELATION_EQ":          1,
		"TOTAL_RELATION_GTE":         2,
	}
)

func (x TotalRelation) Enum() *TotalRelation {
	p := new(TotalRelation)
	*p = x
	return p
}

func (x TotalRelation) String() string {
	return protoimpl.X.EnumStringOf(x.Descriptor(), protoreflect.EnumNumber(x))
}

func (TotalRelation) Descriptor() protoreflect.EnumDescriptor {
	return file_loams_collection_v1_query_proto_enumTypes[6].Descriptor()
}

func (TotalRelation) Type() protoreflect.EnumType {
	return &file_loams_collection_v1_query_proto_enumTypes[6]
}

func (x TotalRelation) Number() protoreflect.EnumNumber {
	return protoreflect.EnumNumber(x)
}

// Deprecated: Use TotalRelation.Descriptor instead.
func (TotalRelation) EnumDescriptor() ([]byte, []int) {
	return file_loams_collection_v1_query_proto_rawDescGZIP(), []int{6}
}

type SearchRequest struct {
	state     protoimpl.MessageState `protogen:"open.v1"`
	Namespace string                 `protobuf:"bytes,1,opt,name=namespace,proto3" json:"namespace,omitempty"`
	// A collection name or an alias. The alias of `from` is this field's other
	// name; a request that sets both names one collection, and `collection`
	// wins.
	Collection string `protobuf:"bytes,2,opt,name=collection,proto3" json:"collection,omitempty"`
	// How fresh the read must be. Absent is `FRESHNESS_STRONG`, the same default
	// the REST route reads, and the `loams-consistency-token` request header
	// still merges into it by the same rule.
	Consistency *Consistency `protobuf:"bytes,3,opt,name=consistency,proto3" json:"consistency,omitempty"`
	// The sources of ranked candidates. Empty with a `filter` is a filter-only
	// query; empty with neither matches nothing.
	Retrievers []*Retriever `protobuf:"bytes,4,rep,name=retrievers,proto3" json:"retrievers,omitempty"`
	// How the ranked lists are combined. Absent is the engine's own combination,
	// which is RRF.
	Fusion *Fusion `protobuf:"bytes,5,opt,name=fusion,proto3" json:"fusion,omitempty"`
	// The filter every retriever's candidates must match.
	Filter *Query `protobuf:"bytes,6,opt,name=filter,proto3" json:"filter,omitempty"`
	// The effective sort; the primary key ascending always breaks ties (Ruling
	// 10).
	Sort []*SortKey `protobuf:"bytes,7,rep,name=sort,proto3" json:"sort,omitempty"`
	// How many hits to skip before the window. `offset` is the IR's, and it is
	// capped by the engine's own limits.
	Offset uint32 `protobuf:"varint,8,opt,name=offset,proto3" json:"offset,omitempty"`
	// The window size; absent is 10, the IR's default. `optional`, so an
	// explicit `0` is a window of nothing rather than the default.
	Limit *uint32 `protobuf:"varint,9,opt,name=limit,proto3,oneof" json:"limit,omitempty"`
	// The sort values of the last hit of the previous page, for a deep page
	// jump. Each is the IR's own JSON: `null`, a bool, a number, a string or
	// `{"uuid": "…"}`.
	SearchAfter []*structpb.Value `protobuf:"bytes,10,rep,name=search_after,json=searchAfter,proto3" json:"search_after,omitempty"`
	// Hits scoring below this are dropped. `optional`, so `0` is a threshold and
	// not an absent one.
	ScoreThreshold *float32 `protobuf:"fixed32,11,opt,name=score_threshold,json=scoreThreshold,proto3,oneof" json:"score_threshold,omitempty"`
	// What to return of each hit, as the REST route's projection JSON
	// (`{"source": "all" | "none" | {"include": [], "exclude": []},
	// "vectors": [], "fields": []}`). Absent is everything, and `id` and
	// `_score` are answered whatever it says.
	Select *structpb.Struct `protobuf:"bytes,12,opt,name=select,proto3" json:"select,omitempty"`
	// An ES aggregation request, carried verbatim as Tantivy's own
	// `Aggregations` JSON owns it.
	Aggregations *structpb.Value `protobuf:"bytes,13,opt,name=aggregations,proto3" json:"aggregations,omitempty"`
	// Text fields to highlight, and how.
	Highlight *Highlight `protobuf:"bytes,14,opt,name=highlight,proto3" json:"highlight,omitempty"`
	// Groups the hits by a field value instead of answering a flat page.
	GroupBy *GroupBy `protobuf:"bytes,15,opt,name=group_by,json=groupBy,proto3" json:"group_by,omitempty"`
	// Whether and how far to count the matches. A `Value` and not a message,
	// for the reason the header gives.
	TrackTotalHits *structpb.Value `protobuf:"bytes,16,opt,name=track_total_hits,json=trackTotalHits,proto3" json:"track_total_hits,omitempty"`
	// **Deprecated; always `invalid_argument`.** `rerank_spec` carries the
	// stage (GR1 Task 20). The field stays so a body carrying it is refused
	// rather than silently dropped.
	//
	// Deprecated: Marked as deprecated in loams/collection/v1/query.proto.
	Rerank *structpb.Struct `protobuf:"bytes,17,opt,name=rerank,proto3" json:"rerank,omitempty"`
	// **Deprecated; always `invalid_argument`.** `expand_spec` carries the
	// stage; see `rerank`.
	//
	// Deprecated: Marked as deprecated in loams/collection/v1/query.proto.
	Expand *structpb.Struct `protobuf:"bytes,18,opt,name=expand,proto3" json:"expand,omitempty"`
	// Design §05 §4's `from`, verbatim: `"collections.<name>"` or `"<name>"`.
	// An alias of `collection` (see the header).
	From string `protobuf:"bytes,19,opt,name=from,proto3" json:"from,omitempty"`
	// Design §05 §4's `retrieve`, verbatim, same element shape. An alias of
	// `retrievers`.
	Retrieve []*Retriever `protobuf:"bytes,20,rep,name=retrieve,proto3" json:"retrieve,omitempty"`
	// Design §05 §4's `fuse`, verbatim: `{"method": "rrf", "k": 60}`. A `Struct`
	// for the reason the header gives, and an alias of `fusion`.
	Fuse *structpb.Struct `protobuf:"bytes,21,opt,name=fuse,proto3" json:"fuse,omitempty"`
	// Walks a Loams Graph graph from the top hits (design §07 §5.2, GR1 Task
	// 20). Needs the server's `graph` feature.
	ExpandSpec *ExpandSpec `protobuf:"bytes,22,opt,name=expand_spec,json=expandSpec,proto3" json:"expand_spec,omitempty"`
	// Re-scores the final candidates, seeds and neighbors (§07 §5.2).
	RerankSpec    *RerankSpec `protobuf:"bytes,23,opt,name=rerank_spec,json=rerankSpec,proto3" json:"rerank_spec,omitempty"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *SearchRequest) Reset() {
	*x = SearchRequest{}
	mi := &file_loams_collection_v1_query_proto_msgTypes[0]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *SearchRequest) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*SearchRequest) ProtoMessage() {}

func (x *SearchRequest) ProtoReflect() protoreflect.Message {
	mi := &file_loams_collection_v1_query_proto_msgTypes[0]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use SearchRequest.ProtoReflect.Descriptor instead.
func (*SearchRequest) Descriptor() ([]byte, []int) {
	return file_loams_collection_v1_query_proto_rawDescGZIP(), []int{0}
}

func (x *SearchRequest) GetNamespace() string {
	if x != nil {
		return x.Namespace
	}
	return ""
}

func (x *SearchRequest) GetCollection() string {
	if x != nil {
		return x.Collection
	}
	return ""
}

func (x *SearchRequest) GetConsistency() *Consistency {
	if x != nil {
		return x.Consistency
	}
	return nil
}

func (x *SearchRequest) GetRetrievers() []*Retriever {
	if x != nil {
		return x.Retrievers
	}
	return nil
}

func (x *SearchRequest) GetFusion() *Fusion {
	if x != nil {
		return x.Fusion
	}
	return nil
}

func (x *SearchRequest) GetFilter() *Query {
	if x != nil {
		return x.Filter
	}
	return nil
}

func (x *SearchRequest) GetSort() []*SortKey {
	if x != nil {
		return x.Sort
	}
	return nil
}

func (x *SearchRequest) GetOffset() uint32 {
	if x != nil {
		return x.Offset
	}
	return 0
}

func (x *SearchRequest) GetLimit() uint32 {
	if x != nil && x.Limit != nil {
		return *x.Limit
	}
	return 0
}

func (x *SearchRequest) GetSearchAfter() []*structpb.Value {
	if x != nil {
		return x.SearchAfter
	}
	return nil
}

func (x *SearchRequest) GetScoreThreshold() float32 {
	if x != nil && x.ScoreThreshold != nil {
		return *x.ScoreThreshold
	}
	return 0
}

func (x *SearchRequest) GetSelect() *structpb.Struct {
	if x != nil {
		return x.Select
	}
	return nil
}

func (x *SearchRequest) GetAggregations() *structpb.Value {
	if x != nil {
		return x.Aggregations
	}
	return nil
}

func (x *SearchRequest) GetHighlight() *Highlight {
	if x != nil {
		return x.Highlight
	}
	return nil
}

func (x *SearchRequest) GetGroupBy() *GroupBy {
	if x != nil {
		return x.GroupBy
	}
	return nil
}

func (x *SearchRequest) GetTrackTotalHits() *structpb.Value {
	if x != nil {
		return x.TrackTotalHits
	}
	return nil
}

// Deprecated: Marked as deprecated in loams/collection/v1/query.proto.
func (x *SearchRequest) GetRerank() *structpb.Struct {
	if x != nil {
		return x.Rerank
	}
	return nil
}

// Deprecated: Marked as deprecated in loams/collection/v1/query.proto.
func (x *SearchRequest) GetExpand() *structpb.Struct {
	if x != nil {
		return x.Expand
	}
	return nil
}

func (x *SearchRequest) GetFrom() string {
	if x != nil {
		return x.From
	}
	return ""
}

func (x *SearchRequest) GetRetrieve() []*Retriever {
	if x != nil {
		return x.Retrieve
	}
	return nil
}

func (x *SearchRequest) GetFuse() *structpb.Struct {
	if x != nil {
		return x.Fuse
	}
	return nil
}

func (x *SearchRequest) GetExpandSpec() *ExpandSpec {
	if x != nil {
		return x.ExpandSpec
	}
	return nil
}

func (x *SearchRequest) GetRerankSpec() *RerankSpec {
	if x != nil {
		return x.RerankSpec
	}
	return nil
}

// One source of ranked candidates. A proto oneof, so the JSON is the REST
// route's `{"vector": {…}}` / `{"text": {…}}` / `{"fused": {…}}` /
// `{"rescore": {…}}` / `{"sparse": {…}}` unchanged, and a retriever that is
// none of the five is `invalid_argument`.
type Retriever struct {
	state protoimpl.MessageState `protogen:"open.v1"`
	// Types that are valid to be assigned to Retriever:
	//
	//	*Retriever_Vector
	//	*Retriever_Text
	//	*Retriever_Fused
	//	*Retriever_Rescore
	//	*Retriever_Sparse
	Retriever     isRetriever_Retriever `protobuf_oneof:"retriever"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *Retriever) Reset() {
	*x = Retriever{}
	mi := &file_loams_collection_v1_query_proto_msgTypes[1]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *Retriever) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*Retriever) ProtoMessage() {}

func (x *Retriever) ProtoReflect() protoreflect.Message {
	mi := &file_loams_collection_v1_query_proto_msgTypes[1]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use Retriever.ProtoReflect.Descriptor instead.
func (*Retriever) Descriptor() ([]byte, []int) {
	return file_loams_collection_v1_query_proto_rawDescGZIP(), []int{1}
}

func (x *Retriever) GetRetriever() isRetriever_Retriever {
	if x != nil {
		return x.Retriever
	}
	return nil
}

func (x *Retriever) GetVector() *VectorQuery {
	if x != nil {
		if x, ok := x.Retriever.(*Retriever_Vector); ok {
			return x.Vector
		}
	}
	return nil
}

func (x *Retriever) GetText() *TextQuery {
	if x != nil {
		if x, ok := x.Retriever.(*Retriever_Text); ok {
			return x.Text
		}
	}
	return nil
}

func (x *Retriever) GetFused() *FusedQuery {
	if x != nil {
		if x, ok := x.Retriever.(*Retriever_Fused); ok {
			return x.Fused
		}
	}
	return nil
}

func (x *Retriever) GetRescore() *RescoreQuery {
	if x != nil {
		if x, ok := x.Retriever.(*Retriever_Rescore); ok {
			return x.Rescore
		}
	}
	return nil
}

func (x *Retriever) GetSparse() *SparseQuery {
	if x != nil {
		if x, ok := x.Retriever.(*Retriever_Sparse); ok {
			return x.Sparse
		}
	}
	return nil
}

type isRetriever_Retriever interface {
	isRetriever_Retriever()
}

type Retriever_Vector struct {
	Vector *VectorQuery `protobuf:"bytes,1,opt,name=vector,proto3,oneof"`
}

type Retriever_Text struct {
	Text *TextQuery `protobuf:"bytes,2,opt,name=text,proto3,oneof"`
}

type Retriever_Fused struct {
	// Runs its own inputs and fuses them, which makes it a subtree of
	// retrievers rather than one stage.
	Fused *FusedQuery `protobuf:"bytes,3,opt,name=fused,proto3,oneof"`
}

type Retriever_Rescore struct {
	// Re-ranks an input's candidates against a second vector.
	Rescore *RescoreQuery `protobuf:"bytes,4,opt,name=rescore,proto3,oneof"`
}

type Retriever_Sparse struct {
	// Exact sparse-vector search (overview A29).
	Sparse *SparseQuery `protobuf:"bytes,5,opt,name=sparse,proto3,oneof"`
}

func (*Retriever_Vector) isRetriever_Retriever() {}

func (*Retriever_Text) isRetriever_Retriever() {}

func (*Retriever_Fused) isRetriever_Retriever() {}

func (*Retriever_Rescore) isRetriever_Retriever() {}

func (*Retriever_Sparse) isRetriever_Retriever() {}

type VectorQuery struct {
	state protoimpl.MessageState `protogen:"open.v1"`
	// The dense vector column to search.
	Field string `protobuf:"bytes,1,opt,name=field,proto3" json:"field,omitempty"`
	// The query vector, as `[1.0, 0.0, 0.0]`.
	Query []float32 `protobuf:"fixed32,2,rep,packed,name=query,proto3" json:"query,omitempty"`
	// How many candidates this retriever contributes before fusion.
	K      uint32     `protobuf:"varint,3,opt,name=k,proto3" json:"k,omitempty"`
	Params *AnnParams `protobuf:"bytes,4,opt,name=params,proto3" json:"params,omitempty"`
	// Narrows this retriever's candidates, over and above the request's
	// `filter`.
	Filter        *Query `protobuf:"bytes,5,opt,name=filter,proto3" json:"filter,omitempty"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *VectorQuery) Reset() {
	*x = VectorQuery{}
	mi := &file_loams_collection_v1_query_proto_msgTypes[2]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *VectorQuery) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*VectorQuery) ProtoMessage() {}

func (x *VectorQuery) ProtoReflect() protoreflect.Message {
	mi := &file_loams_collection_v1_query_proto_msgTypes[2]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use VectorQuery.ProtoReflect.Descriptor instead.
func (*VectorQuery) Descriptor() ([]byte, []int) {
	return file_loams_collection_v1_query_proto_rawDescGZIP(), []int{2}
}

func (x *VectorQuery) GetField() string {
	if x != nil {
		return x.Field
	}
	return ""
}

func (x *VectorQuery) GetQuery() []float32 {
	if x != nil {
		return x.Query
	}
	return nil
}

func (x *VectorQuery) GetK() uint32 {
	if x != nil {
		return x.K
	}
	return 0
}

func (x *VectorQuery) GetParams() *AnnParams {
	if x != nil {
		return x.Params
	}
	return nil
}

func (x *VectorQuery) GetFilter() *Query {
	if x != nil {
		return x.Filter
	}
	return nil
}

type TextQuery struct {
	state protoimpl.MessageState `protogen:"open.v1"`
	// The query over the collection's text fields.
	Query         *Query `protobuf:"bytes,1,opt,name=query,proto3" json:"query,omitempty"`
	K             uint32 `protobuf:"varint,2,opt,name=k,proto3" json:"k,omitempty"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *TextQuery) Reset() {
	*x = TextQuery{}
	mi := &file_loams_collection_v1_query_proto_msgTypes[3]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *TextQuery) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*TextQuery) ProtoMessage() {}

func (x *TextQuery) ProtoReflect() protoreflect.Message {
	mi := &file_loams_collection_v1_query_proto_msgTypes[3]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use TextQuery.ProtoReflect.Descriptor instead.
func (*TextQuery) Descriptor() ([]byte, []int) {
	return file_loams_collection_v1_query_proto_rawDescGZIP(), []int{3}
}

func (x *TextQuery) GetQuery() *Query {
	if x != nil {
		return x.Query
	}
	return nil
}

func (x *TextQuery) GetK() uint32 {
	if x != nil {
		return x.K
	}
	return 0
}

type FusedQuery struct {
	state         protoimpl.MessageState `protogen:"open.v1"`
	Inputs        []*Retriever           `protobuf:"bytes,1,rep,name=inputs,proto3" json:"inputs,omitempty"`
	Fusion        *Fusion                `protobuf:"bytes,2,opt,name=fusion,proto3" json:"fusion,omitempty"`
	K             uint32                 `protobuf:"varint,3,opt,name=k,proto3" json:"k,omitempty"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *FusedQuery) Reset() {
	*x = FusedQuery{}
	mi := &file_loams_collection_v1_query_proto_msgTypes[4]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *FusedQuery) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*FusedQuery) ProtoMessage() {}

func (x *FusedQuery) ProtoReflect() protoreflect.Message {
	mi := &file_loams_collection_v1_query_proto_msgTypes[4]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use FusedQuery.ProtoReflect.Descriptor instead.
func (*FusedQuery) Descriptor() ([]byte, []int) {
	return file_loams_collection_v1_query_proto_rawDescGZIP(), []int{4}
}

func (x *FusedQuery) GetInputs() []*Retriever {
	if x != nil {
		return x.Inputs
	}
	return nil
}

func (x *FusedQuery) GetFusion() *Fusion {
	if x != nil {
		return x.Fusion
	}
	return nil
}

func (x *FusedQuery) GetK() uint32 {
	if x != nil {
		return x.K
	}
	return 0
}

type RescoreQuery struct {
	state protoimpl.MessageState `protogen:"open.v1"`
	// The retriever whose candidates are re-ranked.
	Input *Retriever `protobuf:"bytes,1,opt,name=input,proto3" json:"input,omitempty"`
	// The vector column to re-score against.
	Field         string    `protobuf:"bytes,2,opt,name=field,proto3" json:"field,omitempty"`
	Query         []float32 `protobuf:"fixed32,3,rep,packed,name=query,proto3" json:"query,omitempty"`
	K             uint32    `protobuf:"varint,4,opt,name=k,proto3" json:"k,omitempty"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *RescoreQuery) Reset() {
	*x = RescoreQuery{}
	mi := &file_loams_collection_v1_query_proto_msgTypes[5]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *RescoreQuery) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*RescoreQuery) ProtoMessage() {}

func (x *RescoreQuery) ProtoReflect() protoreflect.Message {
	mi := &file_loams_collection_v1_query_proto_msgTypes[5]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use RescoreQuery.ProtoReflect.Descriptor instead.
func (*RescoreQuery) Descriptor() ([]byte, []int) {
	return file_loams_collection_v1_query_proto_rawDescGZIP(), []int{5}
}

func (x *RescoreQuery) GetInput() *Retriever {
	if x != nil {
		return x.Input
	}
	return nil
}

func (x *RescoreQuery) GetField() string {
	if x != nil {
		return x.Field
	}
	return ""
}

func (x *RescoreQuery) GetQuery() []float32 {
	if x != nil {
		return x.Query
	}
	return nil
}

func (x *RescoreQuery) GetK() uint32 {
	if x != nil {
		return x.K
	}
	return 0
}

type SparseQuery struct {
	state protoimpl.MessageState `protogen:"open.v1"`
	// The sparse vector column to search.
	Field string `protobuf:"bytes,1,opt,name=field,proto3" json:"field,omitempty"`
	// The query vector, as the sparse vector's own JSON:
	// `{"indices": [5], "values": [1.0]}`.
	Query         *structpb.Struct `protobuf:"bytes,2,opt,name=query,proto3" json:"query,omitempty"`
	K             uint32           `protobuf:"varint,3,opt,name=k,proto3" json:"k,omitempty"`
	Filter        *Query           `protobuf:"bytes,4,opt,name=filter,proto3" json:"filter,omitempty"`
	Params        *SparseParams    `protobuf:"bytes,5,opt,name=params,proto3" json:"params,omitempty"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *SparseQuery) Reset() {
	*x = SparseQuery{}
	mi := &file_loams_collection_v1_query_proto_msgTypes[6]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *SparseQuery) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*SparseQuery) ProtoMessage() {}

func (x *SparseQuery) ProtoReflect() protoreflect.Message {
	mi := &file_loams_collection_v1_query_proto_msgTypes[6]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use SparseQuery.ProtoReflect.Descriptor instead.
func (*SparseQuery) Descriptor() ([]byte, []int) {
	return file_loams_collection_v1_query_proto_rawDescGZIP(), []int{6}
}

func (x *SparseQuery) GetField() string {
	if x != nil {
		return x.Field
	}
	return ""
}

func (x *SparseQuery) GetQuery() *structpb.Struct {
	if x != nil {
		return x.Query
	}
	return nil
}

func (x *SparseQuery) GetK() uint32 {
	if x != nil {
		return x.K
	}
	return 0
}

func (x *SparseQuery) GetFilter() *Query {
	if x != nil {
		return x.Filter
	}
	return nil
}

func (x *SparseQuery) GetParams() *SparseParams {
	if x != nil {
		return x.Params
	}
	return nil
}

// The parameters of a dense-vector retriever. Every field absent is the
// engine's default, which is why they are `optional` rather than sent as
// zeroes.
type AnnParams struct {
	state protoimpl.MessageState `protogen:"open.v1"`
	// Search the whole index exactly instead of approximately. This is not a
	// free read.
	Exact bool `protobuf:"varint,1,opt,name=exact,proto3" json:"exact,omitempty"`
	// HNSW layers probed.
	Nprobes *uint32 `protobuf:"varint,2,opt,name=nprobes,proto3,oneof" json:"nprobes,omitempty"`
	// Candidates the approximate pass re-ranks exactly.
	RefineFactor *uint32 `protobuf:"varint,3,opt,name=refine_factor,json=refineFactor,proto3,oneof" json:"refine_factor,omitempty"`
	// The approximate search's beam width.
	Ef *uint32 `protobuf:"varint,4,opt,name=ef,proto3,oneof" json:"ef,omitempty"`
	// Fractions of each candidate's cells to visit (multi-vector).
	Oversampling *float32 `protobuf:"fixed32,5,opt,name=oversampling,proto3,oneof" json:"oversampling,omitempty"`
	// A metric override — `cosine`, `dot`, `euclid` or `manhattan` — as the REST
	// route spells it; only with `exact`. A string rather than an enum so the
	// REST spelling survives verbatim (the same call `ScanColumn.distance`
	// makes).
	Distance      string `protobuf:"bytes,6,opt,name=distance,proto3" json:"distance,omitempty"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *AnnParams) Reset() {
	*x = AnnParams{}
	mi := &file_loams_collection_v1_query_proto_msgTypes[7]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *AnnParams) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*AnnParams) ProtoMessage() {}

func (x *AnnParams) ProtoReflect() protoreflect.Message {
	mi := &file_loams_collection_v1_query_proto_msgTypes[7]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use AnnParams.ProtoReflect.Descriptor instead.
func (*AnnParams) Descriptor() ([]byte, []int) {
	return file_loams_collection_v1_query_proto_rawDescGZIP(), []int{7}
}

func (x *AnnParams) GetExact() bool {
	if x != nil {
		return x.Exact
	}
	return false
}

func (x *AnnParams) GetNprobes() uint32 {
	if x != nil && x.Nprobes != nil {
		return *x.Nprobes
	}
	return 0
}

func (x *AnnParams) GetRefineFactor() uint32 {
	if x != nil && x.RefineFactor != nil {
		return *x.RefineFactor
	}
	return 0
}

func (x *AnnParams) GetEf() uint32 {
	if x != nil && x.Ef != nil {
		return *x.Ef
	}
	return 0
}

func (x *AnnParams) GetOversampling() float32 {
	if x != nil && x.Oversampling != nil {
		return *x.Oversampling
	}
	return 0
}

func (x *AnnParams) GetDistance() string {
	if x != nil {
		return x.Distance
	}
	return ""
}

// The parameters of a sparse retriever.
type SparseParams struct {
	state protoimpl.MessageState `protogen:"open.v1"`
	// The rows the IDF statistics count. Absent is every live row.
	IdfCorpus     *Query `protobuf:"bytes,1,opt,name=idf_corpus,json=idfCorpus,proto3" json:"idf_corpus,omitempty"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *SparseParams) Reset() {
	*x = SparseParams{}
	mi := &file_loams_collection_v1_query_proto_msgTypes[8]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *SparseParams) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*SparseParams) ProtoMessage() {}

func (x *SparseParams) ProtoReflect() protoreflect.Message {
	mi := &file_loams_collection_v1_query_proto_msgTypes[8]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use SparseParams.ProtoReflect.Descriptor instead.
func (*SparseParams) Descriptor() ([]byte, []int) {
	return file_loams_collection_v1_query_proto_rawDescGZIP(), []int{8}
}

func (x *SparseParams) GetIdfCorpus() *Query {
	if x != nil {
		return x.IdfCorpus
	}
	return nil
}

// How ranked lists are combined (overview §6.6, Rulings 4 and 5). A proto
// oneof, so the JSON is the IR's `{"rrf": {"k": 60}}` / `{"dbsf": {}}` /
// `{"weightedSum": {"weights": [0.5, 0.5]}}` — `dbsf` is a unit arm and so is an
// empty message, because the REST spelling `"dbsf"` has no oneof form.
type Fusion struct {
	state protoimpl.MessageState `protogen:"open.v1"`
	// Types that are valid to be assigned to Fusion:
	//
	//	*Fusion_Rrf
	//	*Fusion_Dbsf
	//	*Fusion_WeightedSum
	Fusion        isFusion_Fusion `protobuf_oneof:"fusion"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *Fusion) Reset() {
	*x = Fusion{}
	mi := &file_loams_collection_v1_query_proto_msgTypes[9]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *Fusion) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*Fusion) ProtoMessage() {}

func (x *Fusion) ProtoReflect() protoreflect.Message {
	mi := &file_loams_collection_v1_query_proto_msgTypes[9]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use Fusion.ProtoReflect.Descriptor instead.
func (*Fusion) Descriptor() ([]byte, []int) {
	return file_loams_collection_v1_query_proto_rawDescGZIP(), []int{9}
}

func (x *Fusion) GetFusion() isFusion_Fusion {
	if x != nil {
		return x.Fusion
	}
	return nil
}

func (x *Fusion) GetRrf() *Rrf {
	if x != nil {
		if x, ok := x.Fusion.(*Fusion_Rrf); ok {
			return x.Rrf
		}
	}
	return nil
}

func (x *Fusion) GetDbsf() *emptypb.Empty {
	if x != nil {
		if x, ok := x.Fusion.(*Fusion_Dbsf); ok {
			return x.Dbsf
		}
	}
	return nil
}

func (x *Fusion) GetWeightedSum() *WeightedSum {
	if x != nil {
		if x, ok := x.Fusion.(*Fusion_WeightedSum); ok {
			return x.WeightedSum
		}
	}
	return nil
}

type isFusion_Fusion interface {
	isFusion_Fusion()
}

type Fusion_Rrf struct {
	Rrf *Rrf `protobuf:"bytes,1,opt,name=rrf,proto3,oneof"`
}

type Fusion_Dbsf struct {
	// Distance-based score fusion, with the engine's own parameters.
	Dbsf *emptypb.Empty `protobuf:"bytes,2,opt,name=dbsf,proto3,oneof"`
}

type Fusion_WeightedSum struct {
	WeightedSum *WeightedSum `protobuf:"bytes,3,opt,name=weighted_sum,json=weightedSum,proto3,oneof"`
}

func (*Fusion_Rrf) isFusion_Fusion() {}

func (*Fusion_Dbsf) isFusion_Fusion() {}

func (*Fusion_WeightedSum) isFusion_Fusion() {}

type Rrf struct {
	state protoimpl.MessageState `protogen:"open.v1"`
	// The RRF constant; absent is 60, the IR's default, so `optional` rather
	// than a `0` the engine would have to read as "no smoothing at all".
	K             *uint32 `protobuf:"varint,1,opt,name=k,proto3,oneof" json:"k,omitempty"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *Rrf) Reset() {
	*x = Rrf{}
	mi := &file_loams_collection_v1_query_proto_msgTypes[10]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *Rrf) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*Rrf) ProtoMessage() {}

func (x *Rrf) ProtoReflect() protoreflect.Message {
	mi := &file_loams_collection_v1_query_proto_msgTypes[10]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use Rrf.ProtoReflect.Descriptor instead.
func (*Rrf) Descriptor() ([]byte, []int) {
	return file_loams_collection_v1_query_proto_rawDescGZIP(), []int{10}
}

func (x *Rrf) GetK() uint32 {
	if x != nil && x.K != nil {
		return *x.K
	}
	return 0
}

type WeightedSum struct {
	state protoimpl.MessageState `protogen:"open.v1"`
	// One weight per input retriever, in the order the retrievers are given.
	Weights       []float32 `protobuf:"fixed32,1,rep,packed,name=weights,proto3" json:"weights,omitempty"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *WeightedSum) Reset() {
	*x = WeightedSum{}
	mi := &file_loams_collection_v1_query_proto_msgTypes[11]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *WeightedSum) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*WeightedSum) ProtoMessage() {}

func (x *WeightedSum) ProtoReflect() protoreflect.Message {
	mi := &file_loams_collection_v1_query_proto_msgTypes[11]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use WeightedSum.ProtoReflect.Descriptor instead.
func (*WeightedSum) Descriptor() ([]byte, []int) {
	return file_loams_collection_v1_query_proto_rawDescGZIP(), []int{11}
}

func (x *WeightedSum) GetWeights() []float32 {
	if x != nil {
		return x.Weights
	}
	return nil
}

// A query over the collection's fields: a filter, a text retriever's query, or
// a sparse retriever's IDF corpus. A proto oneof, so the JSON is the IR's own
// externally-tagged spelling with the one difference proto3 JSON forces: a unit
// arm is `{"matchAll": {}}` rather than `"match_all"`, and the arms with an
// underscore in them are `lowerCamelCase` (`matchPhrase`, `multiMatch`,
// `valuesCount`, `isNull`, `isEmpty`, `queryString`, `constantScore`,
// `weightedSum` — see the header).
type Query struct {
	state protoimpl.MessageState `protogen:"open.v1"`
	// Types that are valid to be assigned to Query:
	//
	//	*Query_MatchAll
	//	*Query_MatchNone
	//	*Query_Match
	//	*Query_MatchPhrase
	//	*Query_MultiMatch
	//	*Query_Term
	//	*Query_Terms
	//	*Query_Range
	//	*Query_Exists
	//	*Query_IsNull
	//	*Query_IsEmpty
	//	*Query_ValuesCount
	//	*Query_Prefix
	//	*Query_Wildcard
	//	*Query_Fuzzy
	//	*Query_QueryString
	//	*Query_Bool
	//	*Query_Boost
	//	*Query_ConstantScore
	Query isQuery_Query `protobuf_oneof:"query"`
	// The documents with these ids, and no others.
	//
	// **Not a oneof arm**, and the reason is a wire spelling rather than a
	// modelling preference: a proto3 JSON oneof arm is always `{"arm": {…}}`, and
	// the value here is a **list**, so an arm could only be spelled
	// `{"ids": {"ids": [1, 2]}}` — a wrapper around one field that no IR has and a
	// caller would have to learn separately. As a plain field the list sits exactly
	// where the REST IR puts it: `{"ids": [1, {"uuid": "…"}]}`, which is
	// `{"ids": [{"uint": "1"}, {"uuid": "…"}]}` in proto3 JSON.
	//
	// So `Query` is a oneof **plus** this field, and a request that sets both is
	// `invalid_argument` rather than letting one silently win: two keys naming two
	// different filters is a request that cannot be answered. That refusal is the
	// handler's, not the codec's, and it names the field.
	//
	// Each element is a `google.protobuf.Value`, not a `DocumentId`, because a
	// `repeated` field cannot be a oneof arm in protobuf at all, and because the
	// `DocumentId` shape `{"uint": "1"}` **is** a `Value` — a one-key struct whose
	// values are strings, so a `u64::MAX` id stays exact and `{"uuid": "…"}` keeps
	// the REST spelling verbatim. The handler reads each back through the same
	// `loams_query::json::pk::from_json` `GetDocumentsRequest.ids` goes through, so
	// the three arms and their refusals are the document RPCs'. A whole number
	// (`{"uint": 1}` or a bare `1`, exact up to 2^53) is a `uint` id too, and a
	// bare string is a `string` id, as the REST route spells them.
	Ids           []*structpb.Value `protobuf:"bytes,16,rep,name=ids,proto3" json:"ids,omitempty"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *Query) Reset() {
	*x = Query{}
	mi := &file_loams_collection_v1_query_proto_msgTypes[12]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *Query) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*Query) ProtoMessage() {}

func (x *Query) ProtoReflect() protoreflect.Message {
	mi := &file_loams_collection_v1_query_proto_msgTypes[12]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use Query.ProtoReflect.Descriptor instead.
func (*Query) Descriptor() ([]byte, []int) {
	return file_loams_collection_v1_query_proto_rawDescGZIP(), []int{12}
}

func (x *Query) GetQuery() isQuery_Query {
	if x != nil {
		return x.Query
	}
	return nil
}

func (x *Query) GetMatchAll() *emptypb.Empty {
	if x != nil {
		if x, ok := x.Query.(*Query_MatchAll); ok {
			return x.MatchAll
		}
	}
	return nil
}

func (x *Query) GetMatchNone() *emptypb.Empty {
	if x != nil {
		if x, ok := x.Query.(*Query_MatchNone); ok {
			return x.MatchNone
		}
	}
	return nil
}

func (x *Query) GetMatch() *MatchQuery {
	if x != nil {
		if x, ok := x.Query.(*Query_Match); ok {
			return x.Match
		}
	}
	return nil
}

func (x *Query) GetMatchPhrase() *MatchPhraseQuery {
	if x != nil {
		if x, ok := x.Query.(*Query_MatchPhrase); ok {
			return x.MatchPhrase
		}
	}
	return nil
}

func (x *Query) GetMultiMatch() *MultiMatchQuery {
	if x != nil {
		if x, ok := x.Query.(*Query_MultiMatch); ok {
			return x.MultiMatch
		}
	}
	return nil
}

func (x *Query) GetTerm() *TermQuery {
	if x != nil {
		if x, ok := x.Query.(*Query_Term); ok {
			return x.Term
		}
	}
	return nil
}

func (x *Query) GetTerms() *TermsQuery {
	if x != nil {
		if x, ok := x.Query.(*Query_Terms); ok {
			return x.Terms
		}
	}
	return nil
}

func (x *Query) GetRange() *RangeQuery {
	if x != nil {
		if x, ok := x.Query.(*Query_Range); ok {
			return x.Range
		}
	}
	return nil
}

func (x *Query) GetExists() *FieldQuery {
	if x != nil {
		if x, ok := x.Query.(*Query_Exists); ok {
			return x.Exists
		}
	}
	return nil
}

func (x *Query) GetIsNull() *FieldQuery {
	if x != nil {
		if x, ok := x.Query.(*Query_IsNull); ok {
			return x.IsNull
		}
	}
	return nil
}

func (x *Query) GetIsEmpty() *FieldQuery {
	if x != nil {
		if x, ok := x.Query.(*Query_IsEmpty); ok {
			return x.IsEmpty
		}
	}
	return nil
}

func (x *Query) GetValuesCount() *ValuesCountQuery {
	if x != nil {
		if x, ok := x.Query.(*Query_ValuesCount); ok {
			return x.ValuesCount
		}
	}
	return nil
}

func (x *Query) GetPrefix() *PrefixQuery {
	if x != nil {
		if x, ok := x.Query.(*Query_Prefix); ok {
			return x.Prefix
		}
	}
	return nil
}

func (x *Query) GetWildcard() *WildcardQuery {
	if x != nil {
		if x, ok := x.Query.(*Query_Wildcard); ok {
			return x.Wildcard
		}
	}
	return nil
}

func (x *Query) GetFuzzy() *FuzzyQuery {
	if x != nil {
		if x, ok := x.Query.(*Query_Fuzzy); ok {
			return x.Fuzzy
		}
	}
	return nil
}

func (x *Query) GetQueryString() *QueryStringQuery {
	if x != nil {
		if x, ok := x.Query.(*Query_QueryString); ok {
			return x.QueryString
		}
	}
	return nil
}

func (x *Query) GetBool() *BoolQuery {
	if x != nil {
		if x, ok := x.Query.(*Query_Bool); ok {
			return x.Bool
		}
	}
	return nil
}

func (x *Query) GetBoost() *BoostQuery {
	if x != nil {
		if x, ok := x.Query.(*Query_Boost); ok {
			return x.Boost
		}
	}
	return nil
}

func (x *Query) GetConstantScore() *ConstantScoreQuery {
	if x != nil {
		if x, ok := x.Query.(*Query_ConstantScore); ok {
			return x.ConstantScore
		}
	}
	return nil
}

func (x *Query) GetIds() []*structpb.Value {
	if x != nil {
		return x.Ids
	}
	return nil
}

type isQuery_Query interface {
	isQuery_Query()
}

type Query_MatchAll struct {
	// Every document.
	MatchAll *emptypb.Empty `protobuf:"bytes,1,opt,name=match_all,json=matchAll,proto3,oneof"`
}

type Query_MatchNone struct {
	// No document.
	MatchNone *emptypb.Empty `protobuf:"bytes,2,opt,name=match_none,json=matchNone,proto3,oneof"`
}

type Query_Match struct {
	Match *MatchQuery `protobuf:"bytes,3,opt,name=match,proto3,oneof"`
}

type Query_MatchPhrase struct {
	MatchPhrase *MatchPhraseQuery `protobuf:"bytes,4,opt,name=match_phrase,json=matchPhrase,proto3,oneof"`
}

type Query_MultiMatch struct {
	MultiMatch *MultiMatchQuery `protobuf:"bytes,5,opt,name=multi_match,json=multiMatch,proto3,oneof"`
}

type Query_Term struct {
	// The field equals one value.
	Term *TermQuery `protobuf:"bytes,6,opt,name=term,proto3,oneof"`
}

type Query_Terms struct {
	// The field equals any of these values.
	Terms *TermsQuery `protobuf:"bytes,7,opt,name=terms,proto3,oneof"`
}

type Query_Range struct {
	Range *RangeQuery `protobuf:"bytes,8,opt,name=range,proto3,oneof"`
}

type Query_Exists struct {
	// The field is present, whatever its value.
	Exists *FieldQuery `protobuf:"bytes,9,opt,name=exists,proto3,oneof"`
}

type Query_IsNull struct {
	IsNull *FieldQuery `protobuf:"bytes,10,opt,name=is_null,json=isNull,proto3,oneof"`
}

type Query_IsEmpty struct {
	IsEmpty *FieldQuery `protobuf:"bytes,11,opt,name=is_empty,json=isEmpty,proto3,oneof"`
}

type Query_ValuesCount struct {
	ValuesCount *ValuesCountQuery `protobuf:"bytes,12,opt,name=values_count,json=valuesCount,proto3,oneof"`
}

type Query_Prefix struct {
	Prefix *PrefixQuery `protobuf:"bytes,13,opt,name=prefix,proto3,oneof"`
}

type Query_Wildcard struct {
	Wildcard *WildcardQuery `protobuf:"bytes,14,opt,name=wildcard,proto3,oneof"`
}

type Query_Fuzzy struct {
	Fuzzy *FuzzyQuery `protobuf:"bytes,15,opt,name=fuzzy,proto3,oneof"`
}

type Query_QueryString struct {
	QueryString *QueryStringQuery `protobuf:"bytes,17,opt,name=query_string,json=queryString,proto3,oneof"`
}

type Query_Bool struct {
	Bool *BoolQuery `protobuf:"bytes,18,opt,name=bool,proto3,oneof"`
}

type Query_Boost struct {
	Boost *BoostQuery `protobuf:"bytes,19,opt,name=boost,proto3,oneof"`
}

type Query_ConstantScore struct {
	ConstantScore *ConstantScoreQuery `protobuf:"bytes,20,opt,name=constant_score,json=constantScore,proto3,oneof"`
}

func (*Query_MatchAll) isQuery_Query() {}

func (*Query_MatchNone) isQuery_Query() {}

func (*Query_Match) isQuery_Query() {}

func (*Query_MatchPhrase) isQuery_Query() {}

func (*Query_MultiMatch) isQuery_Query() {}

func (*Query_Term) isQuery_Query() {}

func (*Query_Terms) isQuery_Query() {}

func (*Query_Range) isQuery_Query() {}

func (*Query_Exists) isQuery_Query() {}

func (*Query_IsNull) isQuery_Query() {}

func (*Query_IsEmpty) isQuery_Query() {}

func (*Query_ValuesCount) isQuery_Query() {}

func (*Query_Prefix) isQuery_Query() {}

func (*Query_Wildcard) isQuery_Query() {}

func (*Query_Fuzzy) isQuery_Query() {}

func (*Query_QueryString) isQuery_Query() {}

func (*Query_Bool) isQuery_Query() {}

func (*Query_Boost) isQuery_Query() {}

func (*Query_ConstantScore) isQuery_Query() {}

type MatchQuery struct {
	state protoimpl.MessageState `protogen:"open.v1"`
	Field string                 `protobuf:"bytes,1,opt,name=field,proto3" json:"field,omitempty"`
	Text  string                 `protobuf:"bytes,2,opt,name=text,proto3" json:"text,omitempty"`
	// Whether every term must match: `or` or `and`, absent being `or`.
	//
	// A **string**, not the `BoolOperator` enum, and this is the one enum-shaped
	// field in the file that is not one: the IR's spelling is `"or"` / `"and"`
	// and that is what §05 §4's example body and the native REST route both
	// write, so an enum here would have the codec refuse `"operator": "or"` as an
	// unknown variant *before a handler runs* — a refusal with no reason and no
	// field. A string keeps the REST spelling verbatim, which is the same call
	// `Patch.mode` and `ScanColumn.role` make for the same reason; `or` and `and`
	// are the accepted values and anything else is `invalid_argument`.
	Operator string `protobuf:"bytes,3,opt,name=operator,proto3" json:"operator,omitempty"`
	// How many of the terms must match, as the IR's own string form (`"2"`,
	// `"75%"`).
	MinimumShouldMatch *string    `protobuf:"bytes,4,opt,name=minimum_should_match,json=minimumShouldMatch,proto3,oneof" json:"minimum_should_match,omitempty"`
	Fuzziness          *Fuzziness `protobuf:"bytes,5,opt,name=fuzziness,proto3" json:"fuzziness,omitempty"`
	// The analyzer to run, over the field's own.
	Analyzer      string `protobuf:"bytes,6,opt,name=analyzer,proto3" json:"analyzer,omitempty"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *MatchQuery) Reset() {
	*x = MatchQuery{}
	mi := &file_loams_collection_v1_query_proto_msgTypes[13]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *MatchQuery) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*MatchQuery) ProtoMessage() {}

func (x *MatchQuery) ProtoReflect() protoreflect.Message {
	mi := &file_loams_collection_v1_query_proto_msgTypes[13]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use MatchQuery.ProtoReflect.Descriptor instead.
func (*MatchQuery) Descriptor() ([]byte, []int) {
	return file_loams_collection_v1_query_proto_rawDescGZIP(), []int{13}
}

func (x *MatchQuery) GetField() string {
	if x != nil {
		return x.Field
	}
	return ""
}

func (x *MatchQuery) GetText() string {
	if x != nil {
		return x.Text
	}
	return ""
}

func (x *MatchQuery) GetOperator() string {
	if x != nil {
		return x.Operator
	}
	return ""
}

func (x *MatchQuery) GetMinimumShouldMatch() string {
	if x != nil && x.MinimumShouldMatch != nil {
		return *x.MinimumShouldMatch
	}
	return ""
}

func (x *MatchQuery) GetFuzziness() *Fuzziness {
	if x != nil {
		return x.Fuzziness
	}
	return nil
}

func (x *MatchQuery) GetAnalyzer() string {
	if x != nil {
		return x.Analyzer
	}
	return ""
}

type MatchPhraseQuery struct {
	state protoimpl.MessageState `protogen:"open.v1"`
	Field string                 `protobuf:"bytes,1,opt,name=field,proto3" json:"field,omitempty"`
	Text  string                 `protobuf:"bytes,2,opt,name=text,proto3" json:"text,omitempty"`
	// How far apart the terms may be and still match, in positions.
	Slop          uint32 `protobuf:"varint,3,opt,name=slop,proto3" json:"slop,omitempty"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *MatchPhraseQuery) Reset() {
	*x = MatchPhraseQuery{}
	mi := &file_loams_collection_v1_query_proto_msgTypes[14]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *MatchPhraseQuery) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*MatchPhraseQuery) ProtoMessage() {}

func (x *MatchPhraseQuery) ProtoReflect() protoreflect.Message {
	mi := &file_loams_collection_v1_query_proto_msgTypes[14]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use MatchPhraseQuery.ProtoReflect.Descriptor instead.
func (*MatchPhraseQuery) Descriptor() ([]byte, []int) {
	return file_loams_collection_v1_query_proto_rawDescGZIP(), []int{14}
}

func (x *MatchPhraseQuery) GetField() string {
	if x != nil {
		return x.Field
	}
	return ""
}

func (x *MatchPhraseQuery) GetText() string {
	if x != nil {
		return x.Text
	}
	return ""
}

func (x *MatchPhraseQuery) GetSlop() uint32 {
	if x != nil {
		return x.Slop
	}
	return 0
}

type MultiMatchQuery struct {
	state protoimpl.MessageState `protogen:"open.v1"`
	// The fields to search, each with its boost. None is every indexed text
	// field at boost 1, as Elasticsearch's `index.query.default_field` of `*`.
	Fields []*MultiMatchField `protobuf:"bytes,1,rep,name=fields,proto3" json:"fields,omitempty"`
	Text   string             `protobuf:"bytes,2,opt,name=text,proto3" json:"text,omitempty"`
	// How the fields are combined; absent is `MULTI_MATCH_KIND_BEST_FIELDS`.
	Kind     MultiMatchKind `protobuf:"varint,3,opt,name=kind,proto3,enum=loams.collection.v1.MultiMatchKind" json:"kind,omitempty"`
	Operator BoolOperator   `protobuf:"varint,4,opt,name=operator,proto3,enum=loams.collection.v1.BoolOperator" json:"operator,omitempty"`
	// How much a best-fields match counts against a most-fields one; absent is
	// the engine's own.
	TieBreaker    *float32 `protobuf:"fixed32,5,opt,name=tie_breaker,json=tieBreaker,proto3,oneof" json:"tie_breaker,omitempty"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *MultiMatchQuery) Reset() {
	*x = MultiMatchQuery{}
	mi := &file_loams_collection_v1_query_proto_msgTypes[15]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *MultiMatchQuery) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*MultiMatchQuery) ProtoMessage() {}

func (x *MultiMatchQuery) ProtoReflect() protoreflect.Message {
	mi := &file_loams_collection_v1_query_proto_msgTypes[15]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use MultiMatchQuery.ProtoReflect.Descriptor instead.
func (*MultiMatchQuery) Descriptor() ([]byte, []int) {
	return file_loams_collection_v1_query_proto_rawDescGZIP(), []int{15}
}

func (x *MultiMatchQuery) GetFields() []*MultiMatchField {
	if x != nil {
		return x.Fields
	}
	return nil
}

func (x *MultiMatchQuery) GetText() string {
	if x != nil {
		return x.Text
	}
	return ""
}

func (x *MultiMatchQuery) GetKind() MultiMatchKind {
	if x != nil {
		return x.Kind
	}
	return MultiMatchKind_MULTI_MATCH_KIND_UNSPECIFIED
}

func (x *MultiMatchQuery) GetOperator() BoolOperator {
	if x != nil {
		return x.Operator
	}
	return BoolOperator_BOOL_OPERATOR_UNSPECIFIED
}

func (x *MultiMatchQuery) GetTieBreaker() float32 {
	if x != nil && x.TieBreaker != nil {
		return *x.TieBreaker
	}
	return 0
}

type MultiMatchField struct {
	state protoimpl.MessageState `protogen:"open.v1"`
	Field string                 `protobuf:"bytes,1,opt,name=field,proto3" json:"field,omitempty"`
	// Absent (0) is 1, as a bare field name is in Elasticsearch.
	Boost         float32 `protobuf:"fixed32,2,opt,name=boost,proto3" json:"boost,omitempty"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *MultiMatchField) Reset() {
	*x = MultiMatchField{}
	mi := &file_loams_collection_v1_query_proto_msgTypes[16]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *MultiMatchField) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*MultiMatchField) ProtoMessage() {}

func (x *MultiMatchField) ProtoReflect() protoreflect.Message {
	mi := &file_loams_collection_v1_query_proto_msgTypes[16]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use MultiMatchField.ProtoReflect.Descriptor instead.
func (*MultiMatchField) Descriptor() ([]byte, []int) {
	return file_loams_collection_v1_query_proto_rawDescGZIP(), []int{16}
}

func (x *MultiMatchField) GetField() string {
	if x != nil {
		return x.Field
	}
	return ""
}

func (x *MultiMatchField) GetBoost() float32 {
	if x != nil {
		return x.Boost
	}
	return 0
}

type TermQuery struct {
	state protoimpl.MessageState `protogen:"open.v1"`
	Field string                 `protobuf:"bytes,1,opt,name=field,proto3" json:"field,omitempty"`
	// The value, as the IR's own JSON: a string, a bool, a number or
	// `{"date": "<RFC 3339>"}` (Ruling 9).
	Value         *structpb.Value `protobuf:"bytes,2,opt,name=value,proto3" json:"value,omitempty"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *TermQuery) Reset() {
	*x = TermQuery{}
	mi := &file_loams_collection_v1_query_proto_msgTypes[17]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *TermQuery) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*TermQuery) ProtoMessage() {}

func (x *TermQuery) ProtoReflect() protoreflect.Message {
	mi := &file_loams_collection_v1_query_proto_msgTypes[17]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use TermQuery.ProtoReflect.Descriptor instead.
func (*TermQuery) Descriptor() ([]byte, []int) {
	return file_loams_collection_v1_query_proto_rawDescGZIP(), []int{17}
}

func (x *TermQuery) GetField() string {
	if x != nil {
		return x.Field
	}
	return ""
}

func (x *TermQuery) GetValue() *structpb.Value {
	if x != nil {
		return x.Value
	}
	return nil
}

type TermsQuery struct {
	state         protoimpl.MessageState `protogen:"open.v1"`
	Field         string                 `protobuf:"bytes,1,opt,name=field,proto3" json:"field,omitempty"`
	Values        []*structpb.Value      `protobuf:"bytes,2,rep,name=values,proto3" json:"values,omitempty"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *TermsQuery) Reset() {
	*x = TermsQuery{}
	mi := &file_loams_collection_v1_query_proto_msgTypes[18]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *TermsQuery) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*TermsQuery) ProtoMessage() {}

func (x *TermsQuery) ProtoReflect() protoreflect.Message {
	mi := &file_loams_collection_v1_query_proto_msgTypes[18]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use TermsQuery.ProtoReflect.Descriptor instead.
func (*TermsQuery) Descriptor() ([]byte, []int) {
	return file_loams_collection_v1_query_proto_rawDescGZIP(), []int{18}
}

func (x *TermsQuery) GetField() string {
	if x != nil {
		return x.Field
	}
	return ""
}

func (x *TermsQuery) GetValues() []*structpb.Value {
	if x != nil {
		return x.Values
	}
	return nil
}

type RangeQuery struct {
	state protoimpl.MessageState `protogen:"open.v1"`
	Field string                 `protobuf:"bytes,1,opt,name=field,proto3" json:"field,omitempty"`
	// The bounds, each a value of the field's own kind and each optional: an
	// absent bound is unbounded on that side. `gt`/`gte` and `lt`/`lte` both set
	// is `invalid_argument`.
	Gt            *structpb.Value `protobuf:"bytes,2,opt,name=gt,proto3" json:"gt,omitempty"`
	Gte           *structpb.Value `protobuf:"bytes,3,opt,name=gte,proto3" json:"gte,omitempty"`
	Lt            *structpb.Value `protobuf:"bytes,4,opt,name=lt,proto3" json:"lt,omitempty"`
	Lte           *structpb.Value `protobuf:"bytes,5,opt,name=lte,proto3" json:"lte,omitempty"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *RangeQuery) Reset() {
	*x = RangeQuery{}
	mi := &file_loams_collection_v1_query_proto_msgTypes[19]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *RangeQuery) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*RangeQuery) ProtoMessage() {}

func (x *RangeQuery) ProtoReflect() protoreflect.Message {
	mi := &file_loams_collection_v1_query_proto_msgTypes[19]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use RangeQuery.ProtoReflect.Descriptor instead.
func (*RangeQuery) Descriptor() ([]byte, []int) {
	return file_loams_collection_v1_query_proto_rawDescGZIP(), []int{19}
}

func (x *RangeQuery) GetField() string {
	if x != nil {
		return x.Field
	}
	return ""
}

func (x *RangeQuery) GetGt() *structpb.Value {
	if x != nil {
		return x.Gt
	}
	return nil
}

func (x *RangeQuery) GetGte() *structpb.Value {
	if x != nil {
		return x.Gte
	}
	return nil
}

func (x *RangeQuery) GetLt() *structpb.Value {
	if x != nil {
		return x.Lt
	}
	return nil
}

func (x *RangeQuery) GetLte() *structpb.Value {
	if x != nil {
		return x.Lte
	}
	return nil
}

// `exists`, `is_null` and `is_empty`: one message, because a caller asks the
// same question of a field in each case.
type FieldQuery struct {
	state         protoimpl.MessageState `protogen:"open.v1"`
	Field         string                 `protobuf:"bytes,1,opt,name=field,proto3" json:"field,omitempty"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *FieldQuery) Reset() {
	*x = FieldQuery{}
	mi := &file_loams_collection_v1_query_proto_msgTypes[20]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *FieldQuery) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*FieldQuery) ProtoMessage() {}

func (x *FieldQuery) ProtoReflect() protoreflect.Message {
	mi := &file_loams_collection_v1_query_proto_msgTypes[20]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use FieldQuery.ProtoReflect.Descriptor instead.
func (*FieldQuery) Descriptor() ([]byte, []int) {
	return file_loams_collection_v1_query_proto_rawDescGZIP(), []int{20}
}

func (x *FieldQuery) GetField() string {
	if x != nil {
		return x.Field
	}
	return ""
}

// How many values a field's array (or token) column holds.
type ValuesCountQuery struct {
	state         protoimpl.MessageState `protogen:"open.v1"`
	Field         string                 `protobuf:"bytes,1,opt,name=field,proto3" json:"field,omitempty"`
	Gt            *uint64                `protobuf:"varint,2,opt,name=gt,proto3,oneof" json:"gt,omitempty"`
	Gte           *uint64                `protobuf:"varint,3,opt,name=gte,proto3,oneof" json:"gte,omitempty"`
	Lt            *uint64                `protobuf:"varint,4,opt,name=lt,proto3,oneof" json:"lt,omitempty"`
	Lte           *uint64                `protobuf:"varint,5,opt,name=lte,proto3,oneof" json:"lte,omitempty"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *ValuesCountQuery) Reset() {
	*x = ValuesCountQuery{}
	mi := &file_loams_collection_v1_query_proto_msgTypes[21]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *ValuesCountQuery) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*ValuesCountQuery) ProtoMessage() {}

func (x *ValuesCountQuery) ProtoReflect() protoreflect.Message {
	mi := &file_loams_collection_v1_query_proto_msgTypes[21]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use ValuesCountQuery.ProtoReflect.Descriptor instead.
func (*ValuesCountQuery) Descriptor() ([]byte, []int) {
	return file_loams_collection_v1_query_proto_rawDescGZIP(), []int{21}
}

func (x *ValuesCountQuery) GetField() string {
	if x != nil {
		return x.Field
	}
	return ""
}

func (x *ValuesCountQuery) GetGt() uint64 {
	if x != nil && x.Gt != nil {
		return *x.Gt
	}
	return 0
}

func (x *ValuesCountQuery) GetGte() uint64 {
	if x != nil && x.Gte != nil {
		return *x.Gte
	}
	return 0
}

func (x *ValuesCountQuery) GetLt() uint64 {
	if x != nil && x.Lt != nil {
		return *x.Lt
	}
	return 0
}

func (x *ValuesCountQuery) GetLte() uint64 {
	if x != nil && x.Lte != nil {
		return *x.Lte
	}
	return 0
}

type PrefixQuery struct {
	state         protoimpl.MessageState `protogen:"open.v1"`
	Field         string                 `protobuf:"bytes,1,opt,name=field,proto3" json:"field,omitempty"`
	Value         string                 `protobuf:"bytes,2,opt,name=value,proto3" json:"value,omitempty"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *PrefixQuery) Reset() {
	*x = PrefixQuery{}
	mi := &file_loams_collection_v1_query_proto_msgTypes[22]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *PrefixQuery) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*PrefixQuery) ProtoMessage() {}

func (x *PrefixQuery) ProtoReflect() protoreflect.Message {
	mi := &file_loams_collection_v1_query_proto_msgTypes[22]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use PrefixQuery.ProtoReflect.Descriptor instead.
func (*PrefixQuery) Descriptor() ([]byte, []int) {
	return file_loams_collection_v1_query_proto_rawDescGZIP(), []int{22}
}

func (x *PrefixQuery) GetField() string {
	if x != nil {
		return x.Field
	}
	return ""
}

func (x *PrefixQuery) GetValue() string {
	if x != nil {
		return x.Value
	}
	return ""
}

type WildcardQuery struct {
	state protoimpl.MessageState `protogen:"open.v1"`
	Field string                 `protobuf:"bytes,1,opt,name=field,proto3" json:"field,omitempty"`
	// With `*` and `?`, as the REST route spells it.
	Pattern       string `protobuf:"bytes,2,opt,name=pattern,proto3" json:"pattern,omitempty"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *WildcardQuery) Reset() {
	*x = WildcardQuery{}
	mi := &file_loams_collection_v1_query_proto_msgTypes[23]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *WildcardQuery) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*WildcardQuery) ProtoMessage() {}

func (x *WildcardQuery) ProtoReflect() protoreflect.Message {
	mi := &file_loams_collection_v1_query_proto_msgTypes[23]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use WildcardQuery.ProtoReflect.Descriptor instead.
func (*WildcardQuery) Descriptor() ([]byte, []int) {
	return file_loams_collection_v1_query_proto_rawDescGZIP(), []int{23}
}

func (x *WildcardQuery) GetField() string {
	if x != nil {
		return x.Field
	}
	return ""
}

func (x *WildcardQuery) GetPattern() string {
	if x != nil {
		return x.Pattern
	}
	return ""
}

type FuzzyQuery struct {
	state         protoimpl.MessageState `protogen:"open.v1"`
	Field         string                 `protobuf:"bytes,1,opt,name=field,proto3" json:"field,omitempty"`
	Value         string                 `protobuf:"bytes,2,opt,name=value,proto3" json:"value,omitempty"`
	Fuzziness     *Fuzziness             `protobuf:"bytes,3,opt,name=fuzziness,proto3" json:"fuzziness,omitempty"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *FuzzyQuery) Reset() {
	*x = FuzzyQuery{}
	mi := &file_loams_collection_v1_query_proto_msgTypes[24]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *FuzzyQuery) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*FuzzyQuery) ProtoMessage() {}

func (x *FuzzyQuery) ProtoReflect() protoreflect.Message {
	mi := &file_loams_collection_v1_query_proto_msgTypes[24]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use FuzzyQuery.ProtoReflect.Descriptor instead.
func (*FuzzyQuery) Descriptor() ([]byte, []int) {
	return file_loams_collection_v1_query_proto_rawDescGZIP(), []int{24}
}

func (x *FuzzyQuery) GetField() string {
	if x != nil {
		return x.Field
	}
	return ""
}

func (x *FuzzyQuery) GetValue() string {
	if x != nil {
		return x.Value
	}
	return ""
}

func (x *FuzzyQuery) GetFuzziness() *Fuzziness {
	if x != nil {
		return x.Fuzziness
	}
	return nil
}

type QueryStringQuery struct {
	state protoimpl.MessageState `protogen:"open.v1"`
	// The Lucene query language, as the REST route's `query_string` takes it.
	Query string `protobuf:"bytes,1,opt,name=query,proto3" json:"query,omitempty"`
	// The fields to search when the query names none. None is every indexed
	// text field, as Elasticsearch's `index.query.default_field` of `*`.
	DefaultFields   []string     `protobuf:"bytes,2,rep,name=default_fields,json=defaultFields,proto3" json:"default_fields,omitempty"`
	DefaultOperator BoolOperator `protobuf:"varint,3,opt,name=default_operator,json=defaultOperator,proto3,enum=loams.collection.v1.BoolOperator" json:"default_operator,omitempty"`
	unknownFields   protoimpl.UnknownFields
	sizeCache       protoimpl.SizeCache
}

func (x *QueryStringQuery) Reset() {
	*x = QueryStringQuery{}
	mi := &file_loams_collection_v1_query_proto_msgTypes[25]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *QueryStringQuery) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*QueryStringQuery) ProtoMessage() {}

func (x *QueryStringQuery) ProtoReflect() protoreflect.Message {
	mi := &file_loams_collection_v1_query_proto_msgTypes[25]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use QueryStringQuery.ProtoReflect.Descriptor instead.
func (*QueryStringQuery) Descriptor() ([]byte, []int) {
	return file_loams_collection_v1_query_proto_rawDescGZIP(), []int{25}
}

func (x *QueryStringQuery) GetQuery() string {
	if x != nil {
		return x.Query
	}
	return ""
}

func (x *QueryStringQuery) GetDefaultFields() []string {
	if x != nil {
		return x.DefaultFields
	}
	return nil
}

func (x *QueryStringQuery) GetDefaultOperator() BoolOperator {
	if x != nil {
		return x.DefaultOperator
	}
	return BoolOperator_BOOL_OPERATOR_UNSPECIFIED
}

type BoolQuery struct {
	state protoimpl.MessageState `protogen:"open.v1"`
	// Every one of these must match.
	Must []*Query `protobuf:"bytes,1,rep,name=must,proto3" json:"must,omitempty"`
	// At least `minimum_should_match` of these must match, and scoring is their
	// sum.
	Should []*Query `protobuf:"bytes,2,rep,name=should,proto3" json:"should,omitempty"`
	// None of these may match.
	MustNot []*Query `protobuf:"bytes,3,rep,name=must_not,json=mustNot,proto3" json:"must_not,omitempty"`
	// These must match and do not score.
	Filter             []*Query `protobuf:"bytes,4,rep,name=filter,proto3" json:"filter,omitempty"`
	MinimumShouldMatch *string  `protobuf:"bytes,5,opt,name=minimum_should_match,json=minimumShouldMatch,proto3,oneof" json:"minimum_should_match,omitempty"`
	unknownFields      protoimpl.UnknownFields
	sizeCache          protoimpl.SizeCache
}

func (x *BoolQuery) Reset() {
	*x = BoolQuery{}
	mi := &file_loams_collection_v1_query_proto_msgTypes[26]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *BoolQuery) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*BoolQuery) ProtoMessage() {}

func (x *BoolQuery) ProtoReflect() protoreflect.Message {
	mi := &file_loams_collection_v1_query_proto_msgTypes[26]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use BoolQuery.ProtoReflect.Descriptor instead.
func (*BoolQuery) Descriptor() ([]byte, []int) {
	return file_loams_collection_v1_query_proto_rawDescGZIP(), []int{26}
}

func (x *BoolQuery) GetMust() []*Query {
	if x != nil {
		return x.Must
	}
	return nil
}

func (x *BoolQuery) GetShould() []*Query {
	if x != nil {
		return x.Should
	}
	return nil
}

func (x *BoolQuery) GetMustNot() []*Query {
	if x != nil {
		return x.MustNot
	}
	return nil
}

func (x *BoolQuery) GetFilter() []*Query {
	if x != nil {
		return x.Filter
	}
	return nil
}

func (x *BoolQuery) GetMinimumShouldMatch() string {
	if x != nil && x.MinimumShouldMatch != nil {
		return *x.MinimumShouldMatch
	}
	return ""
}

type BoostQuery struct {
	state protoimpl.MessageState `protogen:"open.v1"`
	// The query whose score is multiplied.
	Query         *Query  `protobuf:"bytes,1,opt,name=query,proto3" json:"query,omitempty"`
	Boost         float32 `protobuf:"fixed32,2,opt,name=boost,proto3" json:"boost,omitempty"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *BoostQuery) Reset() {
	*x = BoostQuery{}
	mi := &file_loams_collection_v1_query_proto_msgTypes[27]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *BoostQuery) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*BoostQuery) ProtoMessage() {}

func (x *BoostQuery) ProtoReflect() protoreflect.Message {
	mi := &file_loams_collection_v1_query_proto_msgTypes[27]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use BoostQuery.ProtoReflect.Descriptor instead.
func (*BoostQuery) Descriptor() ([]byte, []int) {
	return file_loams_collection_v1_query_proto_rawDescGZIP(), []int{27}
}

func (x *BoostQuery) GetQuery() *Query {
	if x != nil {
		return x.Query
	}
	return nil
}

func (x *BoostQuery) GetBoost() float32 {
	if x != nil {
		return x.Boost
	}
	return 0
}

type ConstantScoreQuery struct {
	state protoimpl.MessageState `protogen:"open.v1"`
	// The query that decides membership; its own score is discarded.
	Query *Query `protobuf:"bytes,1,opt,name=query,proto3" json:"query,omitempty"`
	// The score every match is given.
	Score         float32 `protobuf:"fixed32,2,opt,name=score,proto3" json:"score,omitempty"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *ConstantScoreQuery) Reset() {
	*x = ConstantScoreQuery{}
	mi := &file_loams_collection_v1_query_proto_msgTypes[28]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *ConstantScoreQuery) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*ConstantScoreQuery) ProtoMessage() {}

func (x *ConstantScoreQuery) ProtoReflect() protoreflect.Message {
	mi := &file_loams_collection_v1_query_proto_msgTypes[28]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use ConstantScoreQuery.ProtoReflect.Descriptor instead.
func (*ConstantScoreQuery) Descriptor() ([]byte, []int) {
	return file_loams_collection_v1_query_proto_rawDescGZIP(), []int{28}
}

func (x *ConstantScoreQuery) GetQuery() *Query {
	if x != nil {
		return x.Query
	}
	return nil
}

func (x *ConstantScoreQuery) GetScore() float32 {
	if x != nil {
		return x.Score
	}
	return 0
}

// How far a fuzzy match may edit. A oneof, so the REST spellings `"auto"` and
// an integer 0–2 become `{"auto": {}}` and `{"edits": 1}` — a bare string has
// no oneof form, the same reason `match_all` is an empty message.
type Fuzziness struct {
	state protoimpl.MessageState `protogen:"open.v1"`
	// Types that are valid to be assigned to Fuzziness:
	//
	//	*Fuzziness_Auto
	//	*Fuzziness_Edits
	Fuzziness     isFuzziness_Fuzziness `protobuf_oneof:"fuzziness"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *Fuzziness) Reset() {
	*x = Fuzziness{}
	mi := &file_loams_collection_v1_query_proto_msgTypes[29]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *Fuzziness) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*Fuzziness) ProtoMessage() {}

func (x *Fuzziness) ProtoReflect() protoreflect.Message {
	mi := &file_loams_collection_v1_query_proto_msgTypes[29]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use Fuzziness.ProtoReflect.Descriptor instead.
func (*Fuzziness) Descriptor() ([]byte, []int) {
	return file_loams_collection_v1_query_proto_rawDescGZIP(), []int{29}
}

func (x *Fuzziness) GetFuzziness() isFuzziness_Fuzziness {
	if x != nil {
		return x.Fuzziness
	}
	return nil
}

func (x *Fuzziness) GetAuto() *emptypb.Empty {
	if x != nil {
		if x, ok := x.Fuzziness.(*Fuzziness_Auto); ok {
			return x.Auto
		}
	}
	return nil
}

func (x *Fuzziness) GetEdits() uint32 {
	if x != nil {
		if x, ok := x.Fuzziness.(*Fuzziness_Edits); ok {
			return x.Edits
		}
	}
	return 0
}

type isFuzziness_Fuzziness interface {
	isFuzziness_Fuzziness()
}

type Fuzziness_Auto struct {
	// The engine picks from the term's length.
	Auto *emptypb.Empty `protobuf:"bytes,1,opt,name=auto,proto3,oneof"`
}

type Fuzziness_Edits struct {
	// An exact number of edits, 0 to 2.
	Edits uint32 `protobuf:"varint,2,opt,name=edits,proto3,oneof"`
}

func (*Fuzziness_Auto) isFuzziness_Fuzziness() {}

func (*Fuzziness_Edits) isFuzziness_Fuzziness() {}

// One key of the effective sort; the primary key ascending always breaks ties
// (Ruling 10). A proto oneof, so the JSON is `{"score": {"order": …}}` /
// `{"pk": {"order": …}}` / `{"field": {"field": …, "order": …,
// "missing": …}}` — the same shape the IR's serde produces, and
// `{"pk": {}}` remains a valid way to say "primary key, ascending".
type SortKey struct {
	state protoimpl.MessageState `protogen:"open.v1"`
	// Types that are valid to be assigned to Key:
	//
	//	*SortKey_Score
	//	*SortKey_Pk
	//	*SortKey_Field
	Key           isSortKey_Key `protobuf_oneof:"key"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *SortKey) Reset() {
	*x = SortKey{}
	mi := &file_loams_collection_v1_query_proto_msgTypes[30]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *SortKey) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*SortKey) ProtoMessage() {}

func (x *SortKey) ProtoReflect() protoreflect.Message {
	mi := &file_loams_collection_v1_query_proto_msgTypes[30]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use SortKey.ProtoReflect.Descriptor instead.
func (*SortKey) Descriptor() ([]byte, []int) {
	return file_loams_collection_v1_query_proto_rawDescGZIP(), []int{30}
}

func (x *SortKey) GetKey() isSortKey_Key {
	if x != nil {
		return x.Key
	}
	return nil
}

func (x *SortKey) GetScore() *ScoreSort {
	if x != nil {
		if x, ok := x.Key.(*SortKey_Score); ok {
			return x.Score
		}
	}
	return nil
}

func (x *SortKey) GetPk() *PrimaryKeySort {
	if x != nil {
		if x, ok := x.Key.(*SortKey_Pk); ok {
			return x.Pk
		}
	}
	return nil
}

func (x *SortKey) GetField() *FieldSort {
	if x != nil {
		if x, ok := x.Key.(*SortKey_Field); ok {
			return x.Field
		}
	}
	return nil
}

type isSortKey_Key interface {
	isSortKey_Key()
}

type SortKey_Score struct {
	Score *ScoreSort `protobuf:"bytes,1,opt,name=score,proto3,oneof"`
}

type SortKey_Pk struct {
	Pk *PrimaryKeySort `protobuf:"bytes,2,opt,name=pk,proto3,oneof"`
}

type SortKey_Field struct {
	Field *FieldSort `protobuf:"bytes,3,opt,name=field,proto3,oneof"`
}

func (*SortKey_Score) isSortKey_Key() {}

func (*SortKey_Pk) isSortKey_Key() {}

func (*SortKey_Field) isSortKey_Key() {}

type ScoreSort struct {
	state protoimpl.MessageState `protogen:"open.v1"`
	// Absent is descending, which is what sorting by score means.
	Order         SortOrder `protobuf:"varint,1,opt,name=order,proto3,enum=loams.collection.v1.SortOrder" json:"order,omitempty"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *ScoreSort) Reset() {
	*x = ScoreSort{}
	mi := &file_loams_collection_v1_query_proto_msgTypes[31]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *ScoreSort) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*ScoreSort) ProtoMessage() {}

func (x *ScoreSort) ProtoReflect() protoreflect.Message {
	mi := &file_loams_collection_v1_query_proto_msgTypes[31]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use ScoreSort.ProtoReflect.Descriptor instead.
func (*ScoreSort) Descriptor() ([]byte, []int) {
	return file_loams_collection_v1_query_proto_rawDescGZIP(), []int{31}
}

func (x *ScoreSort) GetOrder() SortOrder {
	if x != nil {
		return x.Order
	}
	return SortOrder_SORT_ORDER_UNSPECIFIED
}

type PrimaryKeySort struct {
	state protoimpl.MessageState `protogen:"open.v1"`
	// Absent is ascending.
	Order         SortOrder `protobuf:"varint,1,opt,name=order,proto3,enum=loams.collection.v1.SortOrder" json:"order,omitempty"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *PrimaryKeySort) Reset() {
	*x = PrimaryKeySort{}
	mi := &file_loams_collection_v1_query_proto_msgTypes[32]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *PrimaryKeySort) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*PrimaryKeySort) ProtoMessage() {}

func (x *PrimaryKeySort) ProtoReflect() protoreflect.Message {
	mi := &file_loams_collection_v1_query_proto_msgTypes[32]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use PrimaryKeySort.ProtoReflect.Descriptor instead.
func (*PrimaryKeySort) Descriptor() ([]byte, []int) {
	return file_loams_collection_v1_query_proto_rawDescGZIP(), []int{32}
}

func (x *PrimaryKeySort) GetOrder() SortOrder {
	if x != nil {
		return x.Order
	}
	return SortOrder_SORT_ORDER_UNSPECIFIED
}

type FieldSort struct {
	state protoimpl.MessageState `protogen:"open.v1"`
	Field string                 `protobuf:"bytes,1,opt,name=field,proto3" json:"field,omitempty"`
	// Absent is ascending.
	Order SortOrder `protobuf:"varint,2,opt,name=order,proto3,enum=loams.collection.v1.SortOrder" json:"order,omitempty"`
	// Where a document without the field sorts. Absent is last.
	Missing       MissingOrder `protobuf:"varint,3,opt,name=missing,proto3,enum=loams.collection.v1.MissingOrder" json:"missing,omitempty"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *FieldSort) Reset() {
	*x = FieldSort{}
	mi := &file_loams_collection_v1_query_proto_msgTypes[33]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *FieldSort) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*FieldSort) ProtoMessage() {}

func (x *FieldSort) ProtoReflect() protoreflect.Message {
	mi := &file_loams_collection_v1_query_proto_msgTypes[33]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use FieldSort.ProtoReflect.Descriptor instead.
func (*FieldSort) Descriptor() ([]byte, []int) {
	return file_loams_collection_v1_query_proto_rawDescGZIP(), []int{33}
}

func (x *FieldSort) GetField() string {
	if x != nil {
		return x.Field
	}
	return ""
}

func (x *FieldSort) GetOrder() SortOrder {
	if x != nil {
		return x.Order
	}
	return SortOrder_SORT_ORDER_UNSPECIFIED
}

func (x *FieldSort) GetMissing() MissingOrder {
	if x != nil {
		return x.Missing
	}
	return MissingOrder_MISSING_ORDER_UNSPECIFIED
}

// Highlighting of text fields (ES `highlight`).
type Highlight struct {
	state         protoimpl.MessageState `protogen:"open.v1"`
	Fields        []*HighlightField      `protobuf:"bytes,1,rep,name=fields,proto3" json:"fields,omitempty"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *Highlight) Reset() {
	*x = Highlight{}
	mi := &file_loams_collection_v1_query_proto_msgTypes[34]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *Highlight) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*Highlight) ProtoMessage() {}

func (x *Highlight) ProtoReflect() protoreflect.Message {
	mi := &file_loams_collection_v1_query_proto_msgTypes[34]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use Highlight.ProtoReflect.Descriptor instead.
func (*Highlight) Descriptor() ([]byte, []int) {
	return file_loams_collection_v1_query_proto_rawDescGZIP(), []int{34}
}

func (x *Highlight) GetFields() []*HighlightField {
	if x != nil {
		return x.Fields
	}
	return nil
}

type HighlightField struct {
	state protoimpl.MessageState `protogen:"open.v1"`
	Field string                 `protobuf:"bytes,1,opt,name=field,proto3" json:"field,omitempty"`
	// What wraps a matched term. Absent is the IR's `<em>` / `</em>`, which is
	// why both are `optional`: proto3 cannot tell an absent string from `""`,
	// and `""` is a legitimate tag.
	PreTag  *string `protobuf:"bytes,2,opt,name=pre_tag,json=preTag,proto3,oneof" json:"pre_tag,omitempty"`
	PostTag *string `protobuf:"bytes,3,opt,name=post_tag,json=postTag,proto3,oneof" json:"post_tag,omitempty"`
	// The size of one fragment in characters; absent is the IR's 100.
	FragmentSize *uint32 `protobuf:"varint,4,opt,name=fragment_size,json=fragmentSize,proto3,oneof" json:"fragment_size,omitempty"`
	// How many fragments of the field to answer; absent is the IR's 5.
	NumberOfFragments *uint32 `protobuf:"varint,5,opt,name=number_of_fragments,json=numberOfFragments,proto3,oneof" json:"number_of_fragments,omitempty"`
	unknownFields     protoimpl.UnknownFields
	sizeCache         protoimpl.SizeCache
}

func (x *HighlightField) Reset() {
	*x = HighlightField{}
	mi := &file_loams_collection_v1_query_proto_msgTypes[35]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *HighlightField) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*HighlightField) ProtoMessage() {}

func (x *HighlightField) ProtoReflect() protoreflect.Message {
	mi := &file_loams_collection_v1_query_proto_msgTypes[35]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use HighlightField.ProtoReflect.Descriptor instead.
func (*HighlightField) Descriptor() ([]byte, []int) {
	return file_loams_collection_v1_query_proto_rawDescGZIP(), []int{35}
}

func (x *HighlightField) GetField() string {
	if x != nil {
		return x.Field
	}
	return ""
}

func (x *HighlightField) GetPreTag() string {
	if x != nil && x.PreTag != nil {
		return *x.PreTag
	}
	return ""
}

func (x *HighlightField) GetPostTag() string {
	if x != nil && x.PostTag != nil {
		return *x.PostTag
	}
	return ""
}

func (x *HighlightField) GetFragmentSize() uint32 {
	if x != nil && x.FragmentSize != nil {
		return *x.FragmentSize
	}
	return 0
}

func (x *HighlightField) GetNumberOfFragments() uint32 {
	if x != nil && x.NumberOfFragments != nil {
		return *x.NumberOfFragments
	}
	return 0
}

// Groups hits by a field value (Qdrant's `group_by`).
type GroupBy struct {
	state protoimpl.MessageState `protogen:"open.v1"`
	Field string                 `protobuf:"bytes,1,opt,name=field,proto3" json:"field,omitempty"`
	// How many hits of a group to answer; absent is the IR's 3.
	GroupSize *uint32 `protobuf:"varint,2,opt,name=group_size,json=groupSize,proto3,oneof" json:"group_size,omitempty"`
	// How many groups to answer; absent is the IR's 10.
	Limit         *uint32 `protobuf:"varint,3,opt,name=limit,proto3,oneof" json:"limit,omitempty"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *GroupBy) Reset() {
	*x = GroupBy{}
	mi := &file_loams_collection_v1_query_proto_msgTypes[36]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *GroupBy) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*GroupBy) ProtoMessage() {}

func (x *GroupBy) ProtoReflect() protoreflect.Message {
	mi := &file_loams_collection_v1_query_proto_msgTypes[36]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use GroupBy.ProtoReflect.Descriptor instead.
func (*GroupBy) Descriptor() ([]byte, []int) {
	return file_loams_collection_v1_query_proto_rawDescGZIP(), []int{36}
}

func (x *GroupBy) GetField() string {
	if x != nil {
		return x.Field
	}
	return ""
}

func (x *GroupBy) GetGroupSize() uint32 {
	if x != nil && x.GroupSize != nil {
		return *x.GroupSize
	}
	return 0
}

func (x *GroupBy) GetLimit() uint32 {
	if x != nil && x.Limit != nil {
		return *x.Limit
	}
	return 0
}

type SearchResponse struct {
	state protoimpl.MessageState `protogen:"open.v1"`
	// The hits, in the effective order. Absent when there are none, which is
	// proto3 JSON's spelling of an empty list.
	Hits []*Hit `protobuf:"bytes,1,rep,name=hits,proto3" json:"hits,omitempty"`
	// How many documents matched. Absent unless `track_total_hits` asked for a
	// count, because counting is not free.
	Total *TotalHits `protobuf:"bytes,2,opt,name=total,proto3" json:"total,omitempty"`
	// The aggregations, as Tantivy's own JSON answers them.
	Aggregations *structpb.Value `protobuf:"bytes,3,opt,name=aggregations,proto3" json:"aggregations,omitempty"`
	// The groups, when the request asked to group.
	Groups []*HitGroup `protobuf:"bytes,4,rep,name=groups,proto3" json:"groups,omitempty"`
	// The state the read saw (overview §6.5), and a `consistency.at_least` that
	// reads this state again. The same value is the `loams-consistency-token`
	// response header.
	ReadToken string `protobuf:"bytes,5,opt,name=read_token,json=readToken,proto3" json:"read_token,omitempty"`
	// The hot structures the read used — `hnsw`, `splits` — in the REST route's
	// spelling and in declaration order, and **absent when it used none**. The
	// same list is the `loams-hot-used` response header, which answers `none`
	// where this answers nothing at all: the header is always there, this field
	// is only there when there is something in it.
	HotUsed []string `protobuf:"bytes,6,rep,name=hot_used,json=hotUsed,proto3" json:"hot_used,omitempty"`
	// The design's per-answer `performance` block (§05 §4). Nothing computes it
	// yet, on either surface, so it is declared here and never answered: the
	// field names the contract this answer will meet rather than reporting one
	// this build does not measure. A REST answer has no such key today, and
	// adding it to one surface only is exactly what this package's equality test
	// exists to prevent.
	Performance *structpb.Struct `protobuf:"bytes,7,opt,name=performance,proto3" json:"performance,omitempty"`
	// What the expansion did, when the request expanded (GR1 Task 20).
	Expand        *ExpandStats `protobuf:"bytes,8,opt,name=expand,proto3" json:"expand,omitempty"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *SearchResponse) Reset() {
	*x = SearchResponse{}
	mi := &file_loams_collection_v1_query_proto_msgTypes[37]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *SearchResponse) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*SearchResponse) ProtoMessage() {}

func (x *SearchResponse) ProtoReflect() protoreflect.Message {
	mi := &file_loams_collection_v1_query_proto_msgTypes[37]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use SearchResponse.ProtoReflect.Descriptor instead.
func (*SearchResponse) Descriptor() ([]byte, []int) {
	return file_loams_collection_v1_query_proto_rawDescGZIP(), []int{37}
}

func (x *SearchResponse) GetHits() []*Hit {
	if x != nil {
		return x.Hits
	}
	return nil
}

func (x *SearchResponse) GetTotal() *TotalHits {
	if x != nil {
		return x.Total
	}
	return nil
}

func (x *SearchResponse) GetAggregations() *structpb.Value {
	if x != nil {
		return x.Aggregations
	}
	return nil
}

func (x *SearchResponse) GetGroups() []*HitGroup {
	if x != nil {
		return x.Groups
	}
	return nil
}

func (x *SearchResponse) GetReadToken() string {
	if x != nil {
		return x.ReadToken
	}
	return ""
}

func (x *SearchResponse) GetHotUsed() []string {
	if x != nil {
		return x.HotUsed
	}
	return nil
}

func (x *SearchResponse) GetPerformance() *structpb.Struct {
	if x != nil {
		return x.Performance
	}
	return nil
}

func (x *SearchResponse) GetExpand() *ExpandStats {
	if x != nil {
		return x.Expand
	}
	return nil
}

// One hit.
type Hit struct {
	state protoimpl.MessageState `protogen:"open.v1"`
	// The document's id, as the `DocumentId` oneof `Document.id` already uses,
	// for Task 3's reason: a `u64::MAX` id must not come back through a
	// `double`.
	Pk *DocumentId `protobuf:"bytes,1,opt,name=pk,proto3" json:"pk,omitempty"`
	// Absent at zero, which proto3 JSON spells the same way the REST route
	// writes `0.0`: a filter-only answer scores nothing.
	Score float32 `protobuf:"fixed32,2,opt,name=score,proto3" json:"score,omitempty"`
	// The values this hit sorts by, in `sort`'s order, so a caller can resume
	// from it with `search_after`.
	SortValues []*structpb.Value `protobuf:"bytes,3,rep,name=sort_values,json=sortValues,proto3" json:"sort_values,omitempty"`
	// Absent when the projection leaves the source out, rather than an empty
	// object: "no source" and "an empty source" are different answers.
	Source *structpb.Struct `protobuf:"bytes,4,opt,name=source,proto3" json:"source,omitempty"`
	// Dense vectors by name, as the REST route's JSON (`[1.0, 0.0, 0.0]`).
	Vectors map[string]*structpb.Value `protobuf:"bytes,5,rep,name=vectors,proto3" json:"vectors,omitempty" protobuf_key:"bytes,1,opt,name=key" protobuf_val:"bytes,2,opt,name=value"`
	// Sparse vectors by name, as the REST route's JSON
	// (`{"indices": [1], "values": [1.0]}`), with `indices` ascending.
	SparseVectors map[string]*structpb.Struct `protobuf:"bytes,6,rep,name=sparse_vectors,json=sparseVectors,proto3" json:"sparse_vectors,omitempty" protobuf_key:"bytes,1,opt,name=key" protobuf_val:"bytes,2,opt,name=value"`
	// The fragments per field, when `highlight` asked for them. A `Value`
	// because it is a list of strings, which proto3 JSON spells as a `Value`'s
	// list.
	Highlight map[string]*structpb.Value `protobuf:"bytes,7,rep,name=highlight,proto3" json:"highlight,omitempty" protobuf_key:"bytes,1,opt,name=key" protobuf_val:"bytes,2,opt,name=value"`
	// The typed field values per name, as the REST route's JSON (Ruling 9).
	Fields map[string]*structpb.Value `protobuf:"bytes,8,rep,name=fields,proto3" json:"fields,omitempty" protobuf_key:"bytes,1,opt,name=key" protobuf_val:"bytes,2,opt,name=value"`
	// The vertices an expansion reached from this hit (`EXPAND_OUTPUT_NESTED`).
	Neighbors []*Neighbor `protobuf:"bytes,9,rep,name=neighbors,proto3" json:"neighbors,omitempty"`
	// What the hit is in the graph (`EXPAND_OUTPUT_FLAT`).
	Graph         *GraphHit `protobuf:"bytes,10,opt,name=graph,proto3" json:"graph,omitempty"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *Hit) Reset() {
	*x = Hit{}
	mi := &file_loams_collection_v1_query_proto_msgTypes[38]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *Hit) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*Hit) ProtoMessage() {}

func (x *Hit) ProtoReflect() protoreflect.Message {
	mi := &file_loams_collection_v1_query_proto_msgTypes[38]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use Hit.ProtoReflect.Descriptor instead.
func (*Hit) Descriptor() ([]byte, []int) {
	return file_loams_collection_v1_query_proto_rawDescGZIP(), []int{38}
}

func (x *Hit) GetPk() *DocumentId {
	if x != nil {
		return x.Pk
	}
	return nil
}

func (x *Hit) GetScore() float32 {
	if x != nil {
		return x.Score
	}
	return 0
}

func (x *Hit) GetSortValues() []*structpb.Value {
	if x != nil {
		return x.SortValues
	}
	return nil
}

func (x *Hit) GetSource() *structpb.Struct {
	if x != nil {
		return x.Source
	}
	return nil
}

func (x *Hit) GetVectors() map[string]*structpb.Value {
	if x != nil {
		return x.Vectors
	}
	return nil
}

func (x *Hit) GetSparseVectors() map[string]*structpb.Struct {
	if x != nil {
		return x.SparseVectors
	}
	return nil
}

func (x *Hit) GetHighlight() map[string]*structpb.Value {
	if x != nil {
		return x.Highlight
	}
	return nil
}

func (x *Hit) GetFields() map[string]*structpb.Value {
	if x != nil {
		return x.Fields
	}
	return nil
}

func (x *Hit) GetNeighbors() []*Neighbor {
	if x != nil {
		return x.Neighbors
	}
	return nil
}

func (x *Hit) GetGraph() *GraphHit {
	if x != nil {
		return x.Graph
	}
	return nil
}

// The `expand` stage (design §07 §5.2): the top `seeds` hits seed a walk of
// a graph. Zero takes each number's default.
type ExpandSpec struct {
	state protoimpl.MessageState `protogen:"open.v1"`
	// The graph, in the request's namespace.
	Graph string `protobuf:"bytes,1,opt,name=graph,proto3" json:"graph,omitempty"`
	// The hit's source field holding the seed vertex's key; empty: the hit's id.
	FromField string `protobuf:"bytes,2,opt,name=from_field,json=fromField,proto3" json:"from_field,omitempty"`
	// The seeds' vertex label; empty: any label.
	SeedLabel string `protobuf:"bytes,3,opt,name=seed_label,json=seedLabel,proto3" json:"seed_label,omitempty"`
	// How many top hits seed the walk; 0 is 10.
	Seeds uint32 `protobuf:"varint,4,opt,name=seeds,proto3" json:"seeds,omitempty"`
	// 1 or 2; 0 is 1.
	Hops uint32 `protobuf:"varint,5,opt,name=hops,proto3" json:"hops,omitempty"`
	// Unspecified is out.
	Direction ExpandDirection `protobuf:"varint,6,opt,name=direction,proto3,enum=loams.collection.v1.ExpandDirection" json:"direction,omitempty"`
	// Edge types walked; empty walks every type.
	EdgeTypes []string `protobuf:"bytes,7,rep,name=edge_types,json=edgeTypes,proto3" json:"edge_types,omitempty"`
	// Over the edges' properties (the structured subset of `Query`).
	EdgeFilter *Query `protobuf:"bytes,8,opt,name=edge_filter,json=edgeFilter,proto3" json:"edge_filter,omitempty"`
	// Over the reached vertices' properties.
	VertexFilter *Query `protobuf:"bytes,9,opt,name=vertex_filter,json=vertexFilter,proto3" json:"vertex_filter,omitempty"`
	// Neighbors per seed; 0 is 50.
	LimitPerSeed uint32 `protobuf:"varint,10,opt,name=limit_per_seed,json=limitPerSeed,proto3" json:"limit_per_seed,omitempty"`
	// Neighbors in all; 0 is 100.
	Limit uint32 `protobuf:"varint,11,opt,name=limit,proto3" json:"limit,omitempty"`
	// The neighbors' document fields, as `select` spells a projection; absent
	// fetches none.
	Select *structpb.Struct `protobuf:"bytes,12,opt,name=select,proto3" json:"select,omitempty"`
	// Unspecified is nested.
	Output ExpandOutput `protobuf:"varint,13,opt,name=output,proto3,enum=loams.collection.v1.ExpandOutput" json:"output,omitempty"`
	// A `loams.graph.v1` consistency token: the walk sees at least its writes.
	ConsistencyToken string `protobuf:"bytes,14,opt,name=consistency_token,json=consistencyToken,proto3" json:"consistency_token,omitempty"`
	unknownFields    protoimpl.UnknownFields
	sizeCache        protoimpl.SizeCache
}

func (x *ExpandSpec) Reset() {
	*x = ExpandSpec{}
	mi := &file_loams_collection_v1_query_proto_msgTypes[39]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *ExpandSpec) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*ExpandSpec) ProtoMessage() {}

func (x *ExpandSpec) ProtoReflect() protoreflect.Message {
	mi := &file_loams_collection_v1_query_proto_msgTypes[39]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use ExpandSpec.ProtoReflect.Descriptor instead.
func (*ExpandSpec) Descriptor() ([]byte, []int) {
	return file_loams_collection_v1_query_proto_rawDescGZIP(), []int{39}
}

func (x *ExpandSpec) GetGraph() string {
	if x != nil {
		return x.Graph
	}
	return ""
}

func (x *ExpandSpec) GetFromField() string {
	if x != nil {
		return x.FromField
	}
	return ""
}

func (x *ExpandSpec) GetSeedLabel() string {
	if x != nil {
		return x.SeedLabel
	}
	return ""
}

func (x *ExpandSpec) GetSeeds() uint32 {
	if x != nil {
		return x.Seeds
	}
	return 0
}

func (x *ExpandSpec) GetHops() uint32 {
	if x != nil {
		return x.Hops
	}
	return 0
}

func (x *ExpandSpec) GetDirection() ExpandDirection {
	if x != nil {
		return x.Direction
	}
	return ExpandDirection_EXPAND_DIRECTION_UNSPECIFIED
}

func (x *ExpandSpec) GetEdgeTypes() []string {
	if x != nil {
		return x.EdgeTypes
	}
	return nil
}

func (x *ExpandSpec) GetEdgeFilter() *Query {
	if x != nil {
		return x.EdgeFilter
	}
	return nil
}

func (x *ExpandSpec) GetVertexFilter() *Query {
	if x != nil {
		return x.VertexFilter
	}
	return nil
}

func (x *ExpandSpec) GetLimitPerSeed() uint32 {
	if x != nil {
		return x.LimitPerSeed
	}
	return 0
}

func (x *ExpandSpec) GetLimit() uint32 {
	if x != nil {
		return x.Limit
	}
	return 0
}

func (x *ExpandSpec) GetSelect() *structpb.Struct {
	if x != nil {
		return x.Select
	}
	return nil
}

func (x *ExpandSpec) GetOutput() ExpandOutput {
	if x != nil {
		return x.Output
	}
	return ExpandOutput_EXPAND_OUTPUT_UNSPECIFIED
}

func (x *ExpandSpec) GetConsistencyToken() string {
	if x != nil {
		return x.ConsistencyToken
	}
	return ""
}

// The `rerank` stage (design §07 §5.2).
type RerankSpec struct {
	state protoimpl.MessageState `protogen:"open.v1"`
	// Types that are valid to be assigned to Method:
	//
	//	*RerankSpec_Inherit
	//	*RerankSpec_Vector
	//	*RerankSpec_Model
	Method        isRerankSpec_Method `protobuf_oneof:"method"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *RerankSpec) Reset() {
	*x = RerankSpec{}
	mi := &file_loams_collection_v1_query_proto_msgTypes[40]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *RerankSpec) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*RerankSpec) ProtoMessage() {}

func (x *RerankSpec) ProtoReflect() protoreflect.Message {
	mi := &file_loams_collection_v1_query_proto_msgTypes[40]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use RerankSpec.ProtoReflect.Descriptor instead.
func (*RerankSpec) Descriptor() ([]byte, []int) {
	return file_loams_collection_v1_query_proto_rawDescGZIP(), []int{40}
}

func (x *RerankSpec) GetMethod() isRerankSpec_Method {
	if x != nil {
		return x.Method
	}
	return nil
}

func (x *RerankSpec) GetInherit() *InheritRerank {
	if x != nil {
		if x, ok := x.Method.(*RerankSpec_Inherit); ok {
			return x.Inherit
		}
	}
	return nil
}

func (x *RerankSpec) GetVector() *VectorRerank {
	if x != nil {
		if x, ok := x.Method.(*RerankSpec_Vector); ok {
			return x.Vector
		}
	}
	return nil
}

func (x *RerankSpec) GetModel() *ModelRerank {
	if x != nil {
		if x, ok := x.Method.(*RerankSpec_Model); ok {
			return x.Model
		}
	}
	return nil
}

type isRerankSpec_Method interface {
	isRerankSpec_Method()
}

type RerankSpec_Inherit struct {
	// A neighbor scores the most, over its seeds, of seed score × decay^hop.
	Inherit *InheritRerank `protobuf:"bytes,1,opt,name=inherit,proto3,oneof"`
}

type RerankSpec_Vector struct {
	// The cosine similarity of each candidate's vector to a query vector.
	Vector *VectorRerank `protobuf:"bytes,2,opt,name=vector,proto3,oneof"`
}

type RerankSpec_Model struct {
	// An external reranker: off on every server (§05 §4), always refused.
	Model *ModelRerank `protobuf:"bytes,3,opt,name=model,proto3,oneof"`
}

func (*RerankSpec_Inherit) isRerankSpec_Method() {}

func (*RerankSpec_Vector) isRerankSpec_Method() {}

func (*RerankSpec_Model) isRerankSpec_Method() {}

// See `RerankSpec.inherit`.
type InheritRerank struct {
	state protoimpl.MessageState `protogen:"open.v1"`
	// 0 to 1; absent is 0.5.
	Decay         *float32 `protobuf:"fixed32,1,opt,name=decay,proto3,oneof" json:"decay,omitempty"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *InheritRerank) Reset() {
	*x = InheritRerank{}
	mi := &file_loams_collection_v1_query_proto_msgTypes[41]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *InheritRerank) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*InheritRerank) ProtoMessage() {}

func (x *InheritRerank) ProtoReflect() protoreflect.Message {
	mi := &file_loams_collection_v1_query_proto_msgTypes[41]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use InheritRerank.ProtoReflect.Descriptor instead.
func (*InheritRerank) Descriptor() ([]byte, []int) {
	return file_loams_collection_v1_query_proto_rawDescGZIP(), []int{41}
}

func (x *InheritRerank) GetDecay() float32 {
	if x != nil && x.Decay != nil {
		return *x.Decay
	}
	return 0
}

// See `RerankSpec.vector`.
type VectorRerank struct {
	state protoimpl.MessageState `protogen:"open.v1"`
	// A dense vector field of the candidates' collections.
	Field         string    `protobuf:"bytes,1,opt,name=field,proto3" json:"field,omitempty"`
	Query         []float32 `protobuf:"fixed32,2,rep,packed,name=query,proto3" json:"query,omitempty"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *VectorRerank) Reset() {
	*x = VectorRerank{}
	mi := &file_loams_collection_v1_query_proto_msgTypes[42]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *VectorRerank) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*VectorRerank) ProtoMessage() {}

func (x *VectorRerank) ProtoReflect() protoreflect.Message {
	mi := &file_loams_collection_v1_query_proto_msgTypes[42]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use VectorRerank.ProtoReflect.Descriptor instead.
func (*VectorRerank) Descriptor() ([]byte, []int) {
	return file_loams_collection_v1_query_proto_rawDescGZIP(), []int{42}
}

func (x *VectorRerank) GetField() string {
	if x != nil {
		return x.Field
	}
	return ""
}

func (x *VectorRerank) GetQuery() []float32 {
	if x != nil {
		return x.Query
	}
	return nil
}

// See `RerankSpec.model`.
type ModelRerank struct {
	state         protoimpl.MessageState `protogen:"open.v1"`
	Endpoint      string                 `protobuf:"bytes,1,opt,name=endpoint,proto3" json:"endpoint,omitempty"`
	TextField     string                 `protobuf:"bytes,2,opt,name=text_field,json=textField,proto3" json:"text_field,omitempty"`
	Query         string                 `protobuf:"bytes,3,opt,name=query,proto3" json:"query,omitempty"`
	TopN          uint32                 `protobuf:"varint,4,opt,name=top_n,json=topN,proto3" json:"top_n,omitempty"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *ModelRerank) Reset() {
	*x = ModelRerank{}
	mi := &file_loams_collection_v1_query_proto_msgTypes[43]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *ModelRerank) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*ModelRerank) ProtoMessage() {}

func (x *ModelRerank) ProtoReflect() protoreflect.Message {
	mi := &file_loams_collection_v1_query_proto_msgTypes[43]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use ModelRerank.ProtoReflect.Descriptor instead.
func (*ModelRerank) Descriptor() ([]byte, []int) {
	return file_loams_collection_v1_query_proto_rawDescGZIP(), []int{43}
}

func (x *ModelRerank) GetEndpoint() string {
	if x != nil {
		return x.Endpoint
	}
	return ""
}

func (x *ModelRerank) GetTextField() string {
	if x != nil {
		return x.TextField
	}
	return ""
}

func (x *ModelRerank) GetQuery() string {
	if x != nil {
		return x.Query
	}
	return ""
}

func (x *ModelRerank) GetTopN() uint32 {
	if x != nil {
		return x.TopN
	}
	return 0
}

// A vertex an expansion reached.
type Neighbor struct {
	state protoimpl.MessageState `protogen:"open.v1"`
	Label string                 `protobuf:"bytes,1,opt,name=label,proto3" json:"label,omitempty"`
	// Its key: its document's id when a link wrote it.
	Key *DocumentId `protobuf:"bytes,2,opt,name=key,proto3" json:"key,omitempty"`
	Hop uint32      `protobuf:"varint,3,opt,name=hop,proto3" json:"hop,omitempty"`
	// The vertex it was reached from: the seed for hop 1.
	Via      *DocumentId `protobuf:"bytes,4,opt,name=via,proto3" json:"via,omitempty"`
	EdgeType string      `protobuf:"bytes,5,opt,name=edge_type,json=edgeType,proto3" json:"edge_type,omitempty"`
	// The edge's key (its document's id).
	Edge  *DocumentId `protobuf:"bytes,6,opt,name=edge,proto3" json:"edge,omitempty"`
	Score float32     `protobuf:"fixed32,7,opt,name=score,proto3" json:"score,omitempty"`
	// The vertex's collection, when a collection link wrote it.
	Collection string `protobuf:"bytes,8,opt,name=collection,proto3" json:"collection,omitempty"`
	// Its document, when `ExpandSpec.select` asked for one.
	Source        *structpb.Struct `protobuf:"bytes,9,opt,name=source,proto3" json:"source,omitempty"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *Neighbor) Reset() {
	*x = Neighbor{}
	mi := &file_loams_collection_v1_query_proto_msgTypes[44]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *Neighbor) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*Neighbor) ProtoMessage() {}

func (x *Neighbor) ProtoReflect() protoreflect.Message {
	mi := &file_loams_collection_v1_query_proto_msgTypes[44]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use Neighbor.ProtoReflect.Descriptor instead.
func (*Neighbor) Descriptor() ([]byte, []int) {
	return file_loams_collection_v1_query_proto_rawDescGZIP(), []int{44}
}

func (x *Neighbor) GetLabel() string {
	if x != nil {
		return x.Label
	}
	return ""
}

func (x *Neighbor) GetKey() *DocumentId {
	if x != nil {
		return x.Key
	}
	return nil
}

func (x *Neighbor) GetHop() uint32 {
	if x != nil {
		return x.Hop
	}
	return 0
}

func (x *Neighbor) GetVia() *DocumentId {
	if x != nil {
		return x.Via
	}
	return nil
}

func (x *Neighbor) GetEdgeType() string {
	if x != nil {
		return x.EdgeType
	}
	return ""
}

func (x *Neighbor) GetEdge() *DocumentId {
	if x != nil {
		return x.Edge
	}
	return nil
}

func (x *Neighbor) GetScore() float32 {
	if x != nil {
		return x.Score
	}
	return 0
}

func (x *Neighbor) GetCollection() string {
	if x != nil {
		return x.Collection
	}
	return ""
}

func (x *Neighbor) GetSource() *structpb.Struct {
	if x != nil {
		return x.Source
	}
	return nil
}

// What a flat answer's hit is in the graph.
type GraphHit struct {
	state protoimpl.MessageState `protogen:"open.v1"`
	Label string                 `protobuf:"bytes,1,opt,name=label,proto3" json:"label,omitempty"`
	// 0 for a seed.
	Hop uint32 `protobuf:"varint,2,opt,name=hop,proto3" json:"hop,omitempty"`
	// The seeds it was reached from.
	Seeds         []*DocumentId `protobuf:"bytes,3,rep,name=seeds,proto3" json:"seeds,omitempty"`
	Collection    string        `protobuf:"bytes,4,opt,name=collection,proto3" json:"collection,omitempty"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *GraphHit) Reset() {
	*x = GraphHit{}
	mi := &file_loams_collection_v1_query_proto_msgTypes[45]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *GraphHit) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*GraphHit) ProtoMessage() {}

func (x *GraphHit) ProtoReflect() protoreflect.Message {
	mi := &file_loams_collection_v1_query_proto_msgTypes[45]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use GraphHit.ProtoReflect.Descriptor instead.
func (*GraphHit) Descriptor() ([]byte, []int) {
	return file_loams_collection_v1_query_proto_rawDescGZIP(), []int{45}
}

func (x *GraphHit) GetLabel() string {
	if x != nil {
		return x.Label
	}
	return ""
}

func (x *GraphHit) GetHop() uint32 {
	if x != nil {
		return x.Hop
	}
	return 0
}

func (x *GraphHit) GetSeeds() []*DocumentId {
	if x != nil {
		return x.Seeds
	}
	return nil
}

func (x *GraphHit) GetCollection() string {
	if x != nil {
		return x.Collection
	}
	return ""
}

// What an expansion did.
type ExpandStats struct {
	state protoimpl.MessageState `protogen:"open.v1"`
	Seeds uint64                 `protobuf:"varint,1,opt,name=seeds,proto3" json:"seeds,omitempty"`
	// Seeds with no key or no vertex.
	SeedsSkipped uint64 `protobuf:"varint,2,opt,name=seeds_skipped,json=seedsSkipped,proto3" json:"seeds_skipped,omitempty"`
	Neighbors    uint64 `protobuf:"varint,3,opt,name=neighbors,proto3" json:"neighbors,omitempty"`
	// The graph state the walk read.
	GraphToken    string `protobuf:"bytes,4,opt,name=graph_token,json=graphToken,proto3" json:"graph_token,omitempty"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *ExpandStats) Reset() {
	*x = ExpandStats{}
	mi := &file_loams_collection_v1_query_proto_msgTypes[46]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *ExpandStats) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*ExpandStats) ProtoMessage() {}

func (x *ExpandStats) ProtoReflect() protoreflect.Message {
	mi := &file_loams_collection_v1_query_proto_msgTypes[46]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use ExpandStats.ProtoReflect.Descriptor instead.
func (*ExpandStats) Descriptor() ([]byte, []int) {
	return file_loams_collection_v1_query_proto_rawDescGZIP(), []int{46}
}

func (x *ExpandStats) GetSeeds() uint64 {
	if x != nil {
		return x.Seeds
	}
	return 0
}

func (x *ExpandStats) GetSeedsSkipped() uint64 {
	if x != nil {
		return x.SeedsSkipped
	}
	return 0
}

func (x *ExpandStats) GetNeighbors() uint64 {
	if x != nil {
		return x.Neighbors
	}
	return 0
}

func (x *ExpandStats) GetGraphToken() string {
	if x != nil {
		return x.GraphToken
	}
	return ""
}

// One group of a grouped search.
type HitGroup struct {
	state protoimpl.MessageState `protogen:"open.v1"`
	// The field value the group is, in the IR's own JSON.
	Key           *structpb.Value `protobuf:"bytes,1,opt,name=key,proto3" json:"key,omitempty"`
	Hits          []*Hit          `protobuf:"bytes,2,rep,name=hits,proto3" json:"hits,omitempty"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *HitGroup) Reset() {
	*x = HitGroup{}
	mi := &file_loams_collection_v1_query_proto_msgTypes[47]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *HitGroup) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*HitGroup) ProtoMessage() {}

func (x *HitGroup) ProtoReflect() protoreflect.Message {
	mi := &file_loams_collection_v1_query_proto_msgTypes[47]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use HitGroup.ProtoReflect.Descriptor instead.
func (*HitGroup) Descriptor() ([]byte, []int) {
	return file_loams_collection_v1_query_proto_rawDescGZIP(), []int{47}
}

func (x *HitGroup) GetKey() *structpb.Value {
	if x != nil {
		return x.Key
	}
	return nil
}

func (x *HitGroup) GetHits() []*Hit {
	if x != nil {
		return x.Hits
	}
	return nil
}

// How many documents matched, and whether the count is exact.
type TotalHits struct {
	state protoimpl.MessageState `protogen:"open.v1"`
	// The count. `optional` and always set: a count of zero is a count, and
	// proto3 JSON omitting it would read as "unanswered".
	Value *uint64 `protobuf:"varint,1,opt,name=value,proto3,oneof" json:"value,omitempty"`
	// `TOTAL_RELATION_EQ` when `value` is the whole count and
	// `TOTAL_RELATION_GTE` when it is a floor (`track_total_hits` capped at
	// `up_to`).
	Relation      TotalRelation `protobuf:"varint,2,opt,name=relation,proto3,enum=loams.collection.v1.TotalRelation" json:"relation,omitempty"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *TotalHits) Reset() {
	*x = TotalHits{}
	mi := &file_loams_collection_v1_query_proto_msgTypes[48]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *TotalHits) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*TotalHits) ProtoMessage() {}

func (x *TotalHits) ProtoReflect() protoreflect.Message {
	mi := &file_loams_collection_v1_query_proto_msgTypes[48]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use TotalHits.ProtoReflect.Descriptor instead.
func (*TotalHits) Descriptor() ([]byte, []int) {
	return file_loams_collection_v1_query_proto_rawDescGZIP(), []int{48}
}

func (x *TotalHits) GetValue() uint64 {
	if x != nil && x.Value != nil {
		return *x.Value
	}
	return 0
}

func (x *TotalHits) GetRelation() TotalRelation {
	if x != nil {
		return x.Relation
	}
	return TotalRelation_TOTAL_RELATION_UNSPECIFIED
}

var File_loams_collection_v1_query_proto protoreflect.FileDescriptor

const file_loams_collection_v1_query_proto_rawDesc = "" +
	"\n" +
	"\x1floams/collection/v1/query.proto\x12\x13loams.collection.v1\x1a\x1bgoogle/protobuf/empty.proto\x1a\x1cgoogle/protobuf/struct.proto\x1a\"loams/collection/v1/document.proto\x1a\x1eloams/options/v1/options.proto\"\xb7\t\n" +
	"\rSearchRequest\x12\x1c\n" +
	"\tnamespace\x18\x01 \x01(\tR\tnamespace\x12\x1e\n" +
	"\n" +
	"collection\x18\x02 \x01(\tR\n" +
	"collection\x12B\n" +
	"\vconsistency\x18\x03 \x01(\v2 .loams.collection.v1.ConsistencyR\vconsistency\x12>\n" +
	"\n" +
	"retrievers\x18\x04 \x03(\v2\x1e.loams.collection.v1.RetrieverR\n" +
	"retrievers\x123\n" +
	"\x06fusion\x18\x05 \x01(\v2\x1b.loams.collection.v1.FusionR\x06fusion\x122\n" +
	"\x06filter\x18\x06 \x01(\v2\x1a.loams.collection.v1.QueryR\x06filter\x120\n" +
	"\x04sort\x18\a \x03(\v2\x1c.loams.collection.v1.SortKeyR\x04sort\x12\x16\n" +
	"\x06offset\x18\b \x01(\rR\x06offset\x12\x19\n" +
	"\x05limit\x18\t \x01(\rH\x00R\x05limit\x88\x01\x01\x129\n" +
	"\fsearch_after\x18\n" +
	" \x03(\v2\x16.google.protobuf.ValueR\vsearchAfter\x12,\n" +
	"\x0fscore_threshold\x18\v \x01(\x02H\x01R\x0escoreThreshold\x88\x01\x01\x12/\n" +
	"\x06select\x18\f \x01(\v2\x17.google.protobuf.StructR\x06select\x12:\n" +
	"\faggregations\x18\r \x01(\v2\x16.google.protobuf.ValueR\faggregations\x12<\n" +
	"\thighlight\x18\x0e \x01(\v2\x1e.loams.collection.v1.HighlightR\thighlight\x127\n" +
	"\bgroup_by\x18\x0f \x01(\v2\x1c.loams.collection.v1.GroupByR\agroupBy\x12@\n" +
	"\x10track_total_hits\x18\x10 \x01(\v2\x16.google.protobuf.ValueR\x0etrackTotalHits\x123\n" +
	"\x06rerank\x18\x11 \x01(\v2\x17.google.protobuf.StructB\x02\x18\x01R\x06rerank\x123\n" +
	"\x06expand\x18\x12 \x01(\v2\x17.google.protobuf.StructB\x02\x18\x01R\x06expand\x12\x12\n" +
	"\x04from\x18\x13 \x01(\tR\x04from\x12:\n" +
	"\bretrieve\x18\x14 \x03(\v2\x1e.loams.collection.v1.RetrieverR\bretrieve\x12+\n" +
	"\x04fuse\x18\x15 \x01(\v2\x17.google.protobuf.StructR\x04fuse\x12@\n" +
	"\vexpand_spec\x18\x16 \x01(\v2\x1f.loams.collection.v1.ExpandSpecR\n" +
	"expandSpec\x12@\n" +
	"\vrerank_spec\x18\x17 \x01(\v2\x1f.loams.collection.v1.RerankSpecR\n" +
	"rerankSpecB\b\n" +
	"\x06_limitB\x12\n" +
	"\x10_score_threshold\"\xbe\x02\n" +
	"\tRetriever\x12:\n" +
	"\x06vector\x18\x01 \x01(\v2 .loams.collection.v1.VectorQueryH\x00R\x06vector\x124\n" +
	"\x04text\x18\x02 \x01(\v2\x1e.loams.collection.v1.TextQueryH\x00R\x04text\x127\n" +
	"\x05fused\x18\x03 \x01(\v2\x1f.loams.collection.v1.FusedQueryH\x00R\x05fused\x12=\n" +
	"\arescore\x18\x04 \x01(\v2!.loams.collection.v1.RescoreQueryH\x00R\arescore\x12:\n" +
	"\x06sparse\x18\x05 \x01(\v2 .loams.collection.v1.SparseQueryH\x00R\x06sparseB\v\n" +
	"\tretriever\"\xb3\x01\n" +
	"\vVectorQuery\x12\x14\n" +
	"\x05field\x18\x01 \x01(\tR\x05field\x12\x14\n" +
	"\x05query\x18\x02 \x03(\x02R\x05query\x12\f\n" +
	"\x01k\x18\x03 \x01(\rR\x01k\x126\n" +
	"\x06params\x18\x04 \x01(\v2\x1e.loams.collection.v1.AnnParamsR\x06params\x122\n" +
	"\x06filter\x18\x05 \x01(\v2\x1a.loams.collection.v1.QueryR\x06filter\"K\n" +
	"\tTextQuery\x120\n" +
	"\x05query\x18\x01 \x01(\v2\x1a.loams.collection.v1.QueryR\x05query\x12\f\n" +
	"\x01k\x18\x02 \x01(\rR\x01k\"\x87\x01\n" +
	"\n" +
	"FusedQuery\x126\n" +
	"\x06inputs\x18\x01 \x03(\v2\x1e.loams.collection.v1.RetrieverR\x06inputs\x123\n" +
	"\x06fusion\x18\x02 \x01(\v2\x1b.loams.collection.v1.FusionR\x06fusion\x12\f\n" +
	"\x01k\x18\x03 \x01(\rR\x01k\"~\n" +
	"\fRescoreQuery\x124\n" +
	"\x05input\x18\x01 \x01(\v2\x1e.loams.collection.v1.RetrieverR\x05input\x12\x14\n" +
	"\x05field\x18\x02 \x01(\tR\x05field\x12\x14\n" +
	"\x05query\x18\x03 \x03(\x02R\x05query\x12\f\n" +
	"\x01k\x18\x04 \x01(\rR\x01k\"\xcf\x01\n" +
	"\vSparseQuery\x12\x14\n" +
	"\x05field\x18\x01 \x01(\tR\x05field\x12-\n" +
	"\x05query\x18\x02 \x01(\v2\x17.google.protobuf.StructR\x05query\x12\f\n" +
	"\x01k\x18\x03 \x01(\rR\x01k\x122\n" +
	"\x06filter\x18\x04 \x01(\v2\x1a.loams.collection.v1.QueryR\x06filter\x129\n" +
	"\x06params\x18\x05 \x01(\v2!.loams.collection.v1.SparseParamsR\x06params\"\xfa\x01\n" +
	"\tAnnParams\x12\x14\n" +
	"\x05exact\x18\x01 \x01(\bR\x05exact\x12\x1d\n" +
	"\anprobes\x18\x02 \x01(\rH\x00R\anprobes\x88\x01\x01\x12(\n" +
	"\rrefine_factor\x18\x03 \x01(\rH\x01R\frefineFactor\x88\x01\x01\x12\x13\n" +
	"\x02ef\x18\x04 \x01(\rH\x02R\x02ef\x88\x01\x01\x12'\n" +
	"\foversampling\x18\x05 \x01(\x02H\x03R\foversampling\x88\x01\x01\x12\x1a\n" +
	"\bdistance\x18\x06 \x01(\tR\bdistanceB\n" +
	"\n" +
	"\b_nprobesB\x10\n" +
	"\x0e_refine_factorB\x05\n" +
	"\x03_efB\x0f\n" +
	"\r_oversampling\"I\n" +
	"\fSparseParams\x129\n" +
	"\n" +
	"idf_corpus\x18\x01 \x01(\v2\x1a.loams.collection.v1.QueryR\tidfCorpus\"\xb5\x01\n" +
	"\x06Fusion\x12,\n" +
	"\x03rrf\x18\x01 \x01(\v2\x18.loams.collection.v1.RrfH\x00R\x03rrf\x12,\n" +
	"\x04dbsf\x18\x02 \x01(\v2\x16.google.protobuf.EmptyH\x00R\x04dbsf\x12E\n" +
	"\fweighted_sum\x18\x03 \x01(\v2 .loams.collection.v1.WeightedSumH\x00R\vweightedSumB\b\n" +
	"\x06fusion\"\x1e\n" +
	"\x03Rrf\x12\x11\n" +
	"\x01k\x18\x01 \x01(\rH\x00R\x01k\x88\x01\x01B\x04\n" +
	"\x02_k\"'\n" +
	"\vWeightedSum\x12\x18\n" +
	"\aweights\x18\x01 \x03(\x02R\aweights\"\xe5\t\n" +
	"\x05Query\x125\n" +
	"\tmatch_all\x18\x01 \x01(\v2\x16.google.protobuf.EmptyH\x00R\bmatchAll\x127\n" +
	"\n" +
	"match_none\x18\x02 \x01(\v2\x16.google.protobuf.EmptyH\x00R\tmatchNone\x127\n" +
	"\x05match\x18\x03 \x01(\v2\x1f.loams.collection.v1.MatchQueryH\x00R\x05match\x12J\n" +
	"\fmatch_phrase\x18\x04 \x01(\v2%.loams.collection.v1.MatchPhraseQueryH\x00R\vmatchPhrase\x12G\n" +
	"\vmulti_match\x18\x05 \x01(\v2$.loams.collection.v1.MultiMatchQueryH\x00R\n" +
	"multiMatch\x124\n" +
	"\x04term\x18\x06 \x01(\v2\x1e.loams.collection.v1.TermQueryH\x00R\x04term\x127\n" +
	"\x05terms\x18\a \x01(\v2\x1f.loams.collection.v1.TermsQueryH\x00R\x05terms\x127\n" +
	"\x05range\x18\b \x01(\v2\x1f.loams.collection.v1.RangeQueryH\x00R\x05range\x129\n" +
	"\x06exists\x18\t \x01(\v2\x1f.loams.collection.v1.FieldQueryH\x00R\x06exists\x12:\n" +
	"\ais_null\x18\n" +
	" \x01(\v2\x1f.loams.collection.v1.FieldQueryH\x00R\x06isNull\x12<\n" +
	"\bis_empty\x18\v \x01(\v2\x1f.loams.collection.v1.FieldQueryH\x00R\aisEmpty\x12J\n" +
	"\fvalues_count\x18\f \x01(\v2%.loams.collection.v1.ValuesCountQueryH\x00R\vvaluesCount\x12:\n" +
	"\x06prefix\x18\r \x01(\v2 .loams.collection.v1.PrefixQueryH\x00R\x06prefix\x12@\n" +
	"\bwildcard\x18\x0e \x01(\v2\".loams.collection.v1.WildcardQueryH\x00R\bwildcard\x127\n" +
	"\x05fuzzy\x18\x0f \x01(\v2\x1f.loams.collection.v1.FuzzyQueryH\x00R\x05fuzzy\x12J\n" +
	"\fquery_string\x18\x11 \x01(\v2%.loams.collection.v1.QueryStringQueryH\x00R\vqueryString\x124\n" +
	"\x04bool\x18\x12 \x01(\v2\x1e.loams.collection.v1.BoolQueryH\x00R\x04bool\x127\n" +
	"\x05boost\x18\x13 \x01(\v2\x1f.loams.collection.v1.BoostQueryH\x00R\x05boost\x12P\n" +
	"\x0econstant_score\x18\x14 \x01(\v2'.loams.collection.v1.ConstantScoreQueryH\x00R\rconstantScore\x12(\n" +
	"\x03ids\x18\x10 \x03(\v2\x16.google.protobuf.ValueR\x03idsB\a\n" +
	"\x05query\"\xfc\x01\n" +
	"\n" +
	"MatchQuery\x12\x14\n" +
	"\x05field\x18\x01 \x01(\tR\x05field\x12\x12\n" +
	"\x04text\x18\x02 \x01(\tR\x04text\x12\x1a\n" +
	"\boperator\x18\x03 \x01(\tR\boperator\x125\n" +
	"\x14minimum_should_match\x18\x04 \x01(\tH\x00R\x12minimumShouldMatch\x88\x01\x01\x12<\n" +
	"\tfuzziness\x18\x05 \x01(\v2\x1e.loams.collection.v1.FuzzinessR\tfuzziness\x12\x1a\n" +
	"\banalyzer\x18\x06 \x01(\tR\banalyzerB\x17\n" +
	"\x15_minimum_should_match\"P\n" +
	"\x10MatchPhraseQuery\x12\x14\n" +
	"\x05field\x18\x01 \x01(\tR\x05field\x12\x12\n" +
	"\x04text\x18\x02 \x01(\tR\x04text\x12\x12\n" +
	"\x04slop\x18\x03 \x01(\rR\x04slop\"\x91\x02\n" +
	"\x0fMultiMatchQuery\x12<\n" +
	"\x06fields\x18\x01 \x03(\v2$.loams.collection.v1.MultiMatchFieldR\x06fields\x12\x12\n" +
	"\x04text\x18\x02 \x01(\tR\x04text\x127\n" +
	"\x04kind\x18\x03 \x01(\x0e2#.loams.collection.v1.MultiMatchKindR\x04kind\x12=\n" +
	"\boperator\x18\x04 \x01(\x0e2!.loams.collection.v1.BoolOperatorR\boperator\x12$\n" +
	"\vtie_breaker\x18\x05 \x01(\x02H\x00R\n" +
	"tieBreaker\x88\x01\x01B\x0e\n" +
	"\f_tie_breaker\"=\n" +
	"\x0fMultiMatchField\x12\x14\n" +
	"\x05field\x18\x01 \x01(\tR\x05field\x12\x14\n" +
	"\x05boost\x18\x02 \x01(\x02R\x05boost\"O\n" +
	"\tTermQuery\x12\x14\n" +
	"\x05field\x18\x01 \x01(\tR\x05field\x12,\n" +
	"\x05value\x18\x02 \x01(\v2\x16.google.protobuf.ValueR\x05value\"R\n" +
	"\n" +
	"TermsQuery\x12\x14\n" +
	"\x05field\x18\x01 \x01(\tR\x05field\x12.\n" +
	"\x06values\x18\x02 \x03(\v2\x16.google.protobuf.ValueR\x06values\"\xc6\x01\n" +
	"\n" +
	"RangeQuery\x12\x14\n" +
	"\x05field\x18\x01 \x01(\tR\x05field\x12&\n" +
	"\x02gt\x18\x02 \x01(\v2\x16.google.protobuf.ValueR\x02gt\x12(\n" +
	"\x03gte\x18\x03 \x01(\v2\x16.google.protobuf.ValueR\x03gte\x12&\n" +
	"\x02lt\x18\x04 \x01(\v2\x16.google.protobuf.ValueR\x02lt\x12(\n" +
	"\x03lte\x18\x05 \x01(\v2\x16.google.protobuf.ValueR\x03lte\"\"\n" +
	"\n" +
	"FieldQuery\x12\x14\n" +
	"\x05field\x18\x01 \x01(\tR\x05field\"\x9e\x01\n" +
	"\x10ValuesCountQuery\x12\x14\n" +
	"\x05field\x18\x01 \x01(\tR\x05field\x12\x13\n" +
	"\x02gt\x18\x02 \x01(\x04H\x00R\x02gt\x88\x01\x01\x12\x15\n" +
	"\x03gte\x18\x03 \x01(\x04H\x01R\x03gte\x88\x01\x01\x12\x13\n" +
	"\x02lt\x18\x04 \x01(\x04H\x02R\x02lt\x88\x01\x01\x12\x15\n" +
	"\x03lte\x18\x05 \x01(\x04H\x03R\x03lte\x88\x01\x01B\x05\n" +
	"\x03_gtB\x06\n" +
	"\x04_gteB\x05\n" +
	"\x03_ltB\x06\n" +
	"\x04_lte\"9\n" +
	"\vPrefixQuery\x12\x14\n" +
	"\x05field\x18\x01 \x01(\tR\x05field\x12\x14\n" +
	"\x05value\x18\x02 \x01(\tR\x05value\"?\n" +
	"\rWildcardQuery\x12\x14\n" +
	"\x05field\x18\x01 \x01(\tR\x05field\x12\x18\n" +
	"\apattern\x18\x02 \x01(\tR\apattern\"v\n" +
	"\n" +
	"FuzzyQuery\x12\x14\n" +
	"\x05field\x18\x01 \x01(\tR\x05field\x12\x14\n" +
	"\x05value\x18\x02 \x01(\tR\x05value\x12<\n" +
	"\tfuzziness\x18\x03 \x01(\v2\x1e.loams.collection.v1.FuzzinessR\tfuzziness\"\x9d\x01\n" +
	"\x10QueryStringQuery\x12\x14\n" +
	"\x05query\x18\x01 \x01(\tR\x05query\x12%\n" +
	"\x0edefault_fields\x18\x02 \x03(\tR\rdefaultFields\x12L\n" +
	"\x10default_operator\x18\x03 \x01(\x0e2!.loams.collection.v1.BoolOperatorR\x0fdefaultOperator\"\xaa\x02\n" +
	"\tBoolQuery\x12.\n" +
	"\x04must\x18\x01 \x03(\v2\x1a.loams.collection.v1.QueryR\x04must\x122\n" +
	"\x06should\x18\x02 \x03(\v2\x1a.loams.collection.v1.QueryR\x06should\x125\n" +
	"\bmust_not\x18\x03 \x03(\v2\x1a.loams.collection.v1.QueryR\amustNot\x122\n" +
	"\x06filter\x18\x04 \x03(\v2\x1a.loams.collection.v1.QueryR\x06filter\x125\n" +
	"\x14minimum_should_match\x18\x05 \x01(\tH\x00R\x12minimumShouldMatch\x88\x01\x01B\x17\n" +
	"\x15_minimum_should_match\"T\n" +
	"\n" +
	"BoostQuery\x120\n" +
	"\x05query\x18\x01 \x01(\v2\x1a.loams.collection.v1.QueryR\x05query\x12\x14\n" +
	"\x05boost\x18\x02 \x01(\x02R\x05boost\"\\\n" +
	"\x12ConstantScoreQuery\x120\n" +
	"\x05query\x18\x01 \x01(\v2\x1a.loams.collection.v1.QueryR\x05query\x12\x14\n" +
	"\x05score\x18\x02 \x01(\x02R\x05score\"^\n" +
	"\tFuzziness\x12,\n" +
	"\x04auto\x18\x01 \x01(\v2\x16.google.protobuf.EmptyH\x00R\x04auto\x12\x16\n" +
	"\x05edits\x18\x02 \x01(\rH\x00R\x05editsB\v\n" +
	"\tfuzziness\"\xb7\x01\n" +
	"\aSortKey\x126\n" +
	"\x05score\x18\x01 \x01(\v2\x1e.loams.collection.v1.ScoreSortH\x00R\x05score\x125\n" +
	"\x02pk\x18\x02 \x01(\v2#.loams.collection.v1.PrimaryKeySortH\x00R\x02pk\x126\n" +
	"\x05field\x18\x03 \x01(\v2\x1e.loams.collection.v1.FieldSortH\x00R\x05fieldB\x05\n" +
	"\x03key\"A\n" +
	"\tScoreSort\x124\n" +
	"\x05order\x18\x01 \x01(\x0e2\x1e.loams.collection.v1.SortOrderR\x05order\"F\n" +
	"\x0ePrimaryKeySort\x124\n" +
	"\x05order\x18\x01 \x01(\x0e2\x1e.loams.collection.v1.SortOrderR\x05order\"\x94\x01\n" +
	"\tFieldSort\x12\x14\n" +
	"\x05field\x18\x01 \x01(\tR\x05field\x124\n" +
	"\x05order\x18\x02 \x01(\x0e2\x1e.loams.collection.v1.SortOrderR\x05order\x12;\n" +
	"\amissing\x18\x03 \x01(\x0e2!.loams.collection.v1.MissingOrderR\amissing\"H\n" +
	"\tHighlight\x12;\n" +
	"\x06fields\x18\x01 \x03(\v2#.loams.collection.v1.HighlightFieldR\x06fields\"\x86\x02\n" +
	"\x0eHighlightField\x12\x14\n" +
	"\x05field\x18\x01 \x01(\tR\x05field\x12\x1c\n" +
	"\apre_tag\x18\x02 \x01(\tH\x00R\x06preTag\x88\x01\x01\x12\x1e\n" +
	"\bpost_tag\x18\x03 \x01(\tH\x01R\apostTag\x88\x01\x01\x12(\n" +
	"\rfragment_size\x18\x04 \x01(\rH\x02R\ffragmentSize\x88\x01\x01\x123\n" +
	"\x13number_of_fragments\x18\x05 \x01(\rH\x03R\x11numberOfFragments\x88\x01\x01B\n" +
	"\n" +
	"\b_pre_tagB\v\n" +
	"\t_post_tagB\x10\n" +
	"\x0e_fragment_sizeB\x16\n" +
	"\x14_number_of_fragments\"w\n" +
	"\aGroupBy\x12\x14\n" +
	"\x05field\x18\x01 \x01(\tR\x05field\x12\"\n" +
	"\n" +
	"group_size\x18\x02 \x01(\rH\x00R\tgroupSize\x88\x01\x01\x12\x19\n" +
	"\x05limit\x18\x03 \x01(\rH\x01R\x05limit\x88\x01\x01B\r\n" +
	"\v_group_sizeB\b\n" +
	"\x06_limit\"\x96\x03\n" +
	"\x0eSearchResponse\x12,\n" +
	"\x04hits\x18\x01 \x03(\v2\x18.loams.collection.v1.HitR\x04hits\x124\n" +
	"\x05total\x18\x02 \x01(\v2\x1e.loams.collection.v1.TotalHitsR\x05total\x12:\n" +
	"\faggregations\x18\x03 \x01(\v2\x16.google.protobuf.ValueR\faggregations\x125\n" +
	"\x06groups\x18\x04 \x03(\v2\x1d.loams.collection.v1.HitGroupR\x06groups\x12\x1d\n" +
	"\n" +
	"read_token\x18\x05 \x01(\tR\treadToken\x12\x19\n" +
	"\bhot_used\x18\x06 \x03(\tR\ahotUsed\x129\n" +
	"\vperformance\x18\a \x01(\v2\x17.google.protobuf.StructR\vperformance\x128\n" +
	"\x06expand\x18\b \x01(\v2 .loams.collection.v1.ExpandStatsR\x06expand\"\x9a\a\n" +
	"\x03Hit\x12/\n" +
	"\x02pk\x18\x01 \x01(\v2\x1f.loams.collection.v1.DocumentIdR\x02pk\x12\x14\n" +
	"\x05score\x18\x02 \x01(\x02R\x05score\x127\n" +
	"\vsort_values\x18\x03 \x03(\v2\x16.google.protobuf.ValueR\n" +
	"sortValues\x12/\n" +
	"\x06source\x18\x04 \x01(\v2\x17.google.protobuf.StructR\x06source\x12?\n" +
	"\avectors\x18\x05 \x03(\v2%.loams.collection.v1.Hit.VectorsEntryR\avectors\x12R\n" +
	"\x0esparse_vectors\x18\x06 \x03(\v2+.loams.collection.v1.Hit.SparseVectorsEntryR\rsparseVectors\x12E\n" +
	"\thighlight\x18\a \x03(\v2'.loams.collection.v1.Hit.HighlightEntryR\thighlight\x12<\n" +
	"\x06fields\x18\b \x03(\v2$.loams.collection.v1.Hit.FieldsEntryR\x06fields\x12;\n" +
	"\tneighbors\x18\t \x03(\v2\x1d.loams.collection.v1.NeighborR\tneighbors\x123\n" +
	"\x05graph\x18\n" +
	" \x01(\v2\x1d.loams.collection.v1.GraphHitR\x05graph\x1aR\n" +
	"\fVectorsEntry\x12\x10\n" +
	"\x03key\x18\x01 \x01(\tR\x03key\x12,\n" +
	"\x05value\x18\x02 \x01(\v2\x16.google.protobuf.ValueR\x05value:\x028\x01\x1aY\n" +
	"\x12SparseVectorsEntry\x12\x10\n" +
	"\x03key\x18\x01 \x01(\tR\x03key\x12-\n" +
	"\x05value\x18\x02 \x01(\v2\x17.google.protobuf.StructR\x05value:\x028\x01\x1aT\n" +
	"\x0eHighlightEntry\x12\x10\n" +
	"\x03key\x18\x01 \x01(\tR\x03key\x12,\n" +
	"\x05value\x18\x02 \x01(\v2\x16.google.protobuf.ValueR\x05value:\x028\x01\x1aQ\n" +
	"\vFieldsEntry\x12\x10\n" +
	"\x03key\x18\x01 \x01(\tR\x03key\x12,\n" +
	"\x05value\x18\x02 \x01(\v2\x16.google.protobuf.ValueR\x05value:\x028\x01\"\xc0\x04\n" +
	"\n" +
	"ExpandSpec\x12\x14\n" +
	"\x05graph\x18\x01 \x01(\tR\x05graph\x12\x1d\n" +
	"\n" +
	"from_field\x18\x02 \x01(\tR\tfromField\x12\x1d\n" +
	"\n" +
	"seed_label\x18\x03 \x01(\tR\tseedLabel\x12\x14\n" +
	"\x05seeds\x18\x04 \x01(\rR\x05seeds\x12\x12\n" +
	"\x04hops\x18\x05 \x01(\rR\x04hops\x12B\n" +
	"\tdirection\x18\x06 \x01(\x0e2$.loams.collection.v1.ExpandDirectionR\tdirection\x12\x1d\n" +
	"\n" +
	"edge_types\x18\a \x03(\tR\tedgeTypes\x12;\n" +
	"\vedge_filter\x18\b \x01(\v2\x1a.loams.collection.v1.QueryR\n" +
	"edgeFilter\x12?\n" +
	"\rvertex_filter\x18\t \x01(\v2\x1a.loams.collection.v1.QueryR\fvertexFilter\x12$\n" +
	"\x0elimit_per_seed\x18\n" +
	" \x01(\rR\flimitPerSeed\x12\x14\n" +
	"\x05limit\x18\v \x01(\rR\x05limit\x12/\n" +
	"\x06select\x18\f \x01(\v2\x17.google.protobuf.StructR\x06select\x129\n" +
	"\x06output\x18\r \x01(\x0e2!.loams.collection.v1.ExpandOutputR\x06output\x12+\n" +
	"\x11consistency_token\x18\x0e \x01(\tR\x10consistencyToken\"\xcd\x01\n" +
	"\n" +
	"RerankSpec\x12>\n" +
	"\ainherit\x18\x01 \x01(\v2\".loams.collection.v1.InheritRerankH\x00R\ainherit\x12;\n" +
	"\x06vector\x18\x02 \x01(\v2!.loams.collection.v1.VectorRerankH\x00R\x06vector\x128\n" +
	"\x05model\x18\x03 \x01(\v2 .loams.collection.v1.ModelRerankH\x00R\x05modelB\b\n" +
	"\x06method\"4\n" +
	"\rInheritRerank\x12\x19\n" +
	"\x05decay\x18\x01 \x01(\x02H\x00R\x05decay\x88\x01\x01B\b\n" +
	"\x06_decay\":\n" +
	"\fVectorRerank\x12\x14\n" +
	"\x05field\x18\x01 \x01(\tR\x05field\x12\x14\n" +
	"\x05query\x18\x02 \x03(\x02R\x05query\"s\n" +
	"\vModelRerank\x12\x1a\n" +
	"\bendpoint\x18\x01 \x01(\tR\bendpoint\x12\x1d\n" +
	"\n" +
	"text_field\x18\x02 \x01(\tR\ttextField\x12\x14\n" +
	"\x05query\x18\x03 \x01(\tR\x05query\x12\x13\n" +
	"\x05top_n\x18\x04 \x01(\rR\x04topN\"\xd1\x02\n" +
	"\bNeighbor\x12\x14\n" +
	"\x05label\x18\x01 \x01(\tR\x05label\x121\n" +
	"\x03key\x18\x02 \x01(\v2\x1f.loams.collection.v1.DocumentIdR\x03key\x12\x10\n" +
	"\x03hop\x18\x03 \x01(\rR\x03hop\x121\n" +
	"\x03via\x18\x04 \x01(\v2\x1f.loams.collection.v1.DocumentIdR\x03via\x12\x1b\n" +
	"\tedge_type\x18\x05 \x01(\tR\bedgeType\x123\n" +
	"\x04edge\x18\x06 \x01(\v2\x1f.loams.collection.v1.DocumentIdR\x04edge\x12\x14\n" +
	"\x05score\x18\a \x01(\x02R\x05score\x12\x1e\n" +
	"\n" +
	"collection\x18\b \x01(\tR\n" +
	"collection\x12/\n" +
	"\x06source\x18\t \x01(\v2\x17.google.protobuf.StructR\x06source\"\x89\x01\n" +
	"\bGraphHit\x12\x14\n" +
	"\x05label\x18\x01 \x01(\tR\x05label\x12\x10\n" +
	"\x03hop\x18\x02 \x01(\rR\x03hop\x125\n" +
	"\x05seeds\x18\x03 \x03(\v2\x1f.loams.collection.v1.DocumentIdR\x05seeds\x12\x1e\n" +
	"\n" +
	"collection\x18\x04 \x01(\tR\n" +
	"collection\"\x87\x01\n" +
	"\vExpandStats\x12\x14\n" +
	"\x05seeds\x18\x01 \x01(\x04R\x05seeds\x12#\n" +
	"\rseeds_skipped\x18\x02 \x01(\x04R\fseedsSkipped\x12\x1c\n" +
	"\tneighbors\x18\x03 \x01(\x04R\tneighbors\x12\x1f\n" +
	"\vgraph_token\x18\x04 \x01(\tR\n" +
	"graphToken\"b\n" +
	"\bHitGroup\x12(\n" +
	"\x03key\x18\x01 \x01(\v2\x16.google.protobuf.ValueR\x03key\x12,\n" +
	"\x04hits\x18\x02 \x03(\v2\x18.loams.collection.v1.HitR\x04hits\"p\n" +
	"\tTotalHits\x12\x19\n" +
	"\x05value\x18\x01 \x01(\x04H\x00R\x05value\x88\x01\x01\x12>\n" +
	"\brelation\x18\x02 \x01(\x0e2\".loams.collection.v1.TotalRelationR\brelationB\b\n" +
	"\x06_value*Z\n" +
	"\fBoolOperator\x12\x1d\n" +
	"\x19BOOL_OPERATOR_UNSPECIFIED\x10\x00\x12\x14\n" +
	"\x10BOOL_OPERATOR_OR\x10\x01\x12\x15\n" +
	"\x11BOOL_OPERATOR_AND\x10\x02*\xda\x01\n" +
	"\x0eMultiMatchKind\x12 \n" +
	"\x1cMULTI_MATCH_KIND_UNSPECIFIED\x10\x00\x12 \n" +
	"\x1cMULTI_MATCH_KIND_BEST_FIELDS\x10\x01\x12 \n" +
	"\x1cMULTI_MATCH_KIND_MOST_FIELDS\x10\x02\x12!\n" +
	"\x1dMULTI_MATCH_KIND_CROSS_FIELDS\x10\x03\x12\x1b\n" +
	"\x17MULTI_MATCH_KIND_PHRASE\x10\x04\x12\"\n" +
	"\x1eMULTI_MATCH_KIND_PHRASE_PREFIX\x10\x05*P\n" +
	"\tSortOrder\x12\x1a\n" +
	"\x16SORT_ORDER_UNSPECIFIED\x10\x00\x12\x12\n" +
	"\x0eSORT_ORDER_ASC\x10\x01\x12\x13\n" +
	"\x0fSORT_ORDER_DESC\x10\x02*^\n" +
	"\fMissingOrder\x12\x1d\n" +
	"\x19MISSING_ORDER_UNSPECIFIED\x10\x00\x12\x17\n" +
	"\x13MISSING_ORDER_FIRST\x10\x01\x12\x16\n" +
	"\x12MISSING_ORDER_LAST\x10\x02*\x81\x01\n" +
	"\x0fExpandDirection\x12 \n" +
	"\x1cEXPAND_DIRECTION_UNSPECIFIED\x10\x00\x12\x18\n" +
	"\x14EXPAND_DIRECTION_OUT\x10\x01\x12\x17\n" +
	"\x13EXPAND_DIRECTION_IN\x10\x02\x12\x19\n" +
	"\x15EXPAND_DIRECTION_BOTH\x10\x03*_\n" +
	"\fExpandOutput\x12\x1d\n" +
	"\x19EXPAND_OUTPUT_UNSPECIFIED\x10\x00\x12\x18\n" +
	"\x14EXPAND_OUTPUT_NESTED\x10\x01\x12\x16\n" +
	"\x12EXPAND_OUTPUT_FLAT\x10\x02*^\n" +
	"\rTotalRelation\x12\x1e\n" +
	"\x1aTOTAL_RELATION_UNSPECIFIED\x10\x00\x12\x15\n" +
	"\x11TOTAL_RELATION_EQ\x10\x01\x12\x16\n" +
	"\x12TOTAL_RELATION_GTE\x10\x022\xdc\x01\n" +
	"\fQueryService\x12u\n" +
	"\x06Search\x12\".loams.collection.v1.SearchRequest\x1a#.loams.collection.v1.SearchResponse\"\"\x92\xb5\x18\a\x12\x05query\x92\xb5\x18\x10\n" +
	"\x06vector\x12\x06search\x90\x02\x01\x1aU\x8a\xb5\x18Q\n" +
	"\x06search\x12GQueries: one search over one collection, dense, sparse, text or hybrid.b\x06proto3"

var (
	file_loams_collection_v1_query_proto_rawDescOnce sync.Once
	file_loams_collection_v1_query_proto_rawDescData []byte
)

func file_loams_collection_v1_query_proto_rawDescGZIP() []byte {
	file_loams_collection_v1_query_proto_rawDescOnce.Do(func() {
		file_loams_collection_v1_query_proto_rawDescData = protoimpl.X.CompressGZIP(unsafe.Slice(unsafe.StringData(file_loams_collection_v1_query_proto_rawDesc), len(file_loams_collection_v1_query_proto_rawDesc)))
	})
	return file_loams_collection_v1_query_proto_rawDescData
}

var file_loams_collection_v1_query_proto_enumTypes = make([]protoimpl.EnumInfo, 7)
var file_loams_collection_v1_query_proto_msgTypes = make([]protoimpl.MessageInfo, 53)
var file_loams_collection_v1_query_proto_goTypes = []any{
	(BoolOperator)(0),          // 0: loams.collection.v1.BoolOperator
	(MultiMatchKind)(0),        // 1: loams.collection.v1.MultiMatchKind
	(SortOrder)(0),             // 2: loams.collection.v1.SortOrder
	(MissingOrder)(0),          // 3: loams.collection.v1.MissingOrder
	(ExpandDirection)(0),       // 4: loams.collection.v1.ExpandDirection
	(ExpandOutput)(0),          // 5: loams.collection.v1.ExpandOutput
	(TotalRelation)(0),         // 6: loams.collection.v1.TotalRelation
	(*SearchRequest)(nil),      // 7: loams.collection.v1.SearchRequest
	(*Retriever)(nil),          // 8: loams.collection.v1.Retriever
	(*VectorQuery)(nil),        // 9: loams.collection.v1.VectorQuery
	(*TextQuery)(nil),          // 10: loams.collection.v1.TextQuery
	(*FusedQuery)(nil),         // 11: loams.collection.v1.FusedQuery
	(*RescoreQuery)(nil),       // 12: loams.collection.v1.RescoreQuery
	(*SparseQuery)(nil),        // 13: loams.collection.v1.SparseQuery
	(*AnnParams)(nil),          // 14: loams.collection.v1.AnnParams
	(*SparseParams)(nil),       // 15: loams.collection.v1.SparseParams
	(*Fusion)(nil),             // 16: loams.collection.v1.Fusion
	(*Rrf)(nil),                // 17: loams.collection.v1.Rrf
	(*WeightedSum)(nil),        // 18: loams.collection.v1.WeightedSum
	(*Query)(nil),              // 19: loams.collection.v1.Query
	(*MatchQuery)(nil),         // 20: loams.collection.v1.MatchQuery
	(*MatchPhraseQuery)(nil),   // 21: loams.collection.v1.MatchPhraseQuery
	(*MultiMatchQuery)(nil),    // 22: loams.collection.v1.MultiMatchQuery
	(*MultiMatchField)(nil),    // 23: loams.collection.v1.MultiMatchField
	(*TermQuery)(nil),          // 24: loams.collection.v1.TermQuery
	(*TermsQuery)(nil),         // 25: loams.collection.v1.TermsQuery
	(*RangeQuery)(nil),         // 26: loams.collection.v1.RangeQuery
	(*FieldQuery)(nil),         // 27: loams.collection.v1.FieldQuery
	(*ValuesCountQuery)(nil),   // 28: loams.collection.v1.ValuesCountQuery
	(*PrefixQuery)(nil),        // 29: loams.collection.v1.PrefixQuery
	(*WildcardQuery)(nil),      // 30: loams.collection.v1.WildcardQuery
	(*FuzzyQuery)(nil),         // 31: loams.collection.v1.FuzzyQuery
	(*QueryStringQuery)(nil),   // 32: loams.collection.v1.QueryStringQuery
	(*BoolQuery)(nil),          // 33: loams.collection.v1.BoolQuery
	(*BoostQuery)(nil),         // 34: loams.collection.v1.BoostQuery
	(*ConstantScoreQuery)(nil), // 35: loams.collection.v1.ConstantScoreQuery
	(*Fuzziness)(nil),          // 36: loams.collection.v1.Fuzziness
	(*SortKey)(nil),            // 37: loams.collection.v1.SortKey
	(*ScoreSort)(nil),          // 38: loams.collection.v1.ScoreSort
	(*PrimaryKeySort)(nil),     // 39: loams.collection.v1.PrimaryKeySort
	(*FieldSort)(nil),          // 40: loams.collection.v1.FieldSort
	(*Highlight)(nil),          // 41: loams.collection.v1.Highlight
	(*HighlightField)(nil),     // 42: loams.collection.v1.HighlightField
	(*GroupBy)(nil),            // 43: loams.collection.v1.GroupBy
	(*SearchResponse)(nil),     // 44: loams.collection.v1.SearchResponse
	(*Hit)(nil),                // 45: loams.collection.v1.Hit
	(*ExpandSpec)(nil),         // 46: loams.collection.v1.ExpandSpec
	(*RerankSpec)(nil),         // 47: loams.collection.v1.RerankSpec
	(*InheritRerank)(nil),      // 48: loams.collection.v1.InheritRerank
	(*VectorRerank)(nil),       // 49: loams.collection.v1.VectorRerank
	(*ModelRerank)(nil),        // 50: loams.collection.v1.ModelRerank
	(*Neighbor)(nil),           // 51: loams.collection.v1.Neighbor
	(*GraphHit)(nil),           // 52: loams.collection.v1.GraphHit
	(*ExpandStats)(nil),        // 53: loams.collection.v1.ExpandStats
	(*HitGroup)(nil),           // 54: loams.collection.v1.HitGroup
	(*TotalHits)(nil),          // 55: loams.collection.v1.TotalHits
	nil,                        // 56: loams.collection.v1.Hit.VectorsEntry
	nil,                        // 57: loams.collection.v1.Hit.SparseVectorsEntry
	nil,                        // 58: loams.collection.v1.Hit.HighlightEntry
	nil,                        // 59: loams.collection.v1.Hit.FieldsEntry
	(*Consistency)(nil),        // 60: loams.collection.v1.Consistency
	(*structpb.Value)(nil),     // 61: google.protobuf.Value
	(*structpb.Struct)(nil),    // 62: google.protobuf.Struct
	(*emptypb.Empty)(nil),      // 63: google.protobuf.Empty
	(*DocumentId)(nil),         // 64: loams.collection.v1.DocumentId
}
var file_loams_collection_v1_query_proto_depIdxs = []int32{
	60,  // 0: loams.collection.v1.SearchRequest.consistency:type_name -> loams.collection.v1.Consistency
	8,   // 1: loams.collection.v1.SearchRequest.retrievers:type_name -> loams.collection.v1.Retriever
	16,  // 2: loams.collection.v1.SearchRequest.fusion:type_name -> loams.collection.v1.Fusion
	19,  // 3: loams.collection.v1.SearchRequest.filter:type_name -> loams.collection.v1.Query
	37,  // 4: loams.collection.v1.SearchRequest.sort:type_name -> loams.collection.v1.SortKey
	61,  // 5: loams.collection.v1.SearchRequest.search_after:type_name -> google.protobuf.Value
	62,  // 6: loams.collection.v1.SearchRequest.select:type_name -> google.protobuf.Struct
	61,  // 7: loams.collection.v1.SearchRequest.aggregations:type_name -> google.protobuf.Value
	41,  // 8: loams.collection.v1.SearchRequest.highlight:type_name -> loams.collection.v1.Highlight
	43,  // 9: loams.collection.v1.SearchRequest.group_by:type_name -> loams.collection.v1.GroupBy
	61,  // 10: loams.collection.v1.SearchRequest.track_total_hits:type_name -> google.protobuf.Value
	62,  // 11: loams.collection.v1.SearchRequest.rerank:type_name -> google.protobuf.Struct
	62,  // 12: loams.collection.v1.SearchRequest.expand:type_name -> google.protobuf.Struct
	8,   // 13: loams.collection.v1.SearchRequest.retrieve:type_name -> loams.collection.v1.Retriever
	62,  // 14: loams.collection.v1.SearchRequest.fuse:type_name -> google.protobuf.Struct
	46,  // 15: loams.collection.v1.SearchRequest.expand_spec:type_name -> loams.collection.v1.ExpandSpec
	47,  // 16: loams.collection.v1.SearchRequest.rerank_spec:type_name -> loams.collection.v1.RerankSpec
	9,   // 17: loams.collection.v1.Retriever.vector:type_name -> loams.collection.v1.VectorQuery
	10,  // 18: loams.collection.v1.Retriever.text:type_name -> loams.collection.v1.TextQuery
	11,  // 19: loams.collection.v1.Retriever.fused:type_name -> loams.collection.v1.FusedQuery
	12,  // 20: loams.collection.v1.Retriever.rescore:type_name -> loams.collection.v1.RescoreQuery
	13,  // 21: loams.collection.v1.Retriever.sparse:type_name -> loams.collection.v1.SparseQuery
	14,  // 22: loams.collection.v1.VectorQuery.params:type_name -> loams.collection.v1.AnnParams
	19,  // 23: loams.collection.v1.VectorQuery.filter:type_name -> loams.collection.v1.Query
	19,  // 24: loams.collection.v1.TextQuery.query:type_name -> loams.collection.v1.Query
	8,   // 25: loams.collection.v1.FusedQuery.inputs:type_name -> loams.collection.v1.Retriever
	16,  // 26: loams.collection.v1.FusedQuery.fusion:type_name -> loams.collection.v1.Fusion
	8,   // 27: loams.collection.v1.RescoreQuery.input:type_name -> loams.collection.v1.Retriever
	62,  // 28: loams.collection.v1.SparseQuery.query:type_name -> google.protobuf.Struct
	19,  // 29: loams.collection.v1.SparseQuery.filter:type_name -> loams.collection.v1.Query
	15,  // 30: loams.collection.v1.SparseQuery.params:type_name -> loams.collection.v1.SparseParams
	19,  // 31: loams.collection.v1.SparseParams.idf_corpus:type_name -> loams.collection.v1.Query
	17,  // 32: loams.collection.v1.Fusion.rrf:type_name -> loams.collection.v1.Rrf
	63,  // 33: loams.collection.v1.Fusion.dbsf:type_name -> google.protobuf.Empty
	18,  // 34: loams.collection.v1.Fusion.weighted_sum:type_name -> loams.collection.v1.WeightedSum
	63,  // 35: loams.collection.v1.Query.match_all:type_name -> google.protobuf.Empty
	63,  // 36: loams.collection.v1.Query.match_none:type_name -> google.protobuf.Empty
	20,  // 37: loams.collection.v1.Query.match:type_name -> loams.collection.v1.MatchQuery
	21,  // 38: loams.collection.v1.Query.match_phrase:type_name -> loams.collection.v1.MatchPhraseQuery
	22,  // 39: loams.collection.v1.Query.multi_match:type_name -> loams.collection.v1.MultiMatchQuery
	24,  // 40: loams.collection.v1.Query.term:type_name -> loams.collection.v1.TermQuery
	25,  // 41: loams.collection.v1.Query.terms:type_name -> loams.collection.v1.TermsQuery
	26,  // 42: loams.collection.v1.Query.range:type_name -> loams.collection.v1.RangeQuery
	27,  // 43: loams.collection.v1.Query.exists:type_name -> loams.collection.v1.FieldQuery
	27,  // 44: loams.collection.v1.Query.is_null:type_name -> loams.collection.v1.FieldQuery
	27,  // 45: loams.collection.v1.Query.is_empty:type_name -> loams.collection.v1.FieldQuery
	28,  // 46: loams.collection.v1.Query.values_count:type_name -> loams.collection.v1.ValuesCountQuery
	29,  // 47: loams.collection.v1.Query.prefix:type_name -> loams.collection.v1.PrefixQuery
	30,  // 48: loams.collection.v1.Query.wildcard:type_name -> loams.collection.v1.WildcardQuery
	31,  // 49: loams.collection.v1.Query.fuzzy:type_name -> loams.collection.v1.FuzzyQuery
	32,  // 50: loams.collection.v1.Query.query_string:type_name -> loams.collection.v1.QueryStringQuery
	33,  // 51: loams.collection.v1.Query.bool:type_name -> loams.collection.v1.BoolQuery
	34,  // 52: loams.collection.v1.Query.boost:type_name -> loams.collection.v1.BoostQuery
	35,  // 53: loams.collection.v1.Query.constant_score:type_name -> loams.collection.v1.ConstantScoreQuery
	61,  // 54: loams.collection.v1.Query.ids:type_name -> google.protobuf.Value
	36,  // 55: loams.collection.v1.MatchQuery.fuzziness:type_name -> loams.collection.v1.Fuzziness
	23,  // 56: loams.collection.v1.MultiMatchQuery.fields:type_name -> loams.collection.v1.MultiMatchField
	1,   // 57: loams.collection.v1.MultiMatchQuery.kind:type_name -> loams.collection.v1.MultiMatchKind
	0,   // 58: loams.collection.v1.MultiMatchQuery.operator:type_name -> loams.collection.v1.BoolOperator
	61,  // 59: loams.collection.v1.TermQuery.value:type_name -> google.protobuf.Value
	61,  // 60: loams.collection.v1.TermsQuery.values:type_name -> google.protobuf.Value
	61,  // 61: loams.collection.v1.RangeQuery.gt:type_name -> google.protobuf.Value
	61,  // 62: loams.collection.v1.RangeQuery.gte:type_name -> google.protobuf.Value
	61,  // 63: loams.collection.v1.RangeQuery.lt:type_name -> google.protobuf.Value
	61,  // 64: loams.collection.v1.RangeQuery.lte:type_name -> google.protobuf.Value
	36,  // 65: loams.collection.v1.FuzzyQuery.fuzziness:type_name -> loams.collection.v1.Fuzziness
	0,   // 66: loams.collection.v1.QueryStringQuery.default_operator:type_name -> loams.collection.v1.BoolOperator
	19,  // 67: loams.collection.v1.BoolQuery.must:type_name -> loams.collection.v1.Query
	19,  // 68: loams.collection.v1.BoolQuery.should:type_name -> loams.collection.v1.Query
	19,  // 69: loams.collection.v1.BoolQuery.must_not:type_name -> loams.collection.v1.Query
	19,  // 70: loams.collection.v1.BoolQuery.filter:type_name -> loams.collection.v1.Query
	19,  // 71: loams.collection.v1.BoostQuery.query:type_name -> loams.collection.v1.Query
	19,  // 72: loams.collection.v1.ConstantScoreQuery.query:type_name -> loams.collection.v1.Query
	63,  // 73: loams.collection.v1.Fuzziness.auto:type_name -> google.protobuf.Empty
	38,  // 74: loams.collection.v1.SortKey.score:type_name -> loams.collection.v1.ScoreSort
	39,  // 75: loams.collection.v1.SortKey.pk:type_name -> loams.collection.v1.PrimaryKeySort
	40,  // 76: loams.collection.v1.SortKey.field:type_name -> loams.collection.v1.FieldSort
	2,   // 77: loams.collection.v1.ScoreSort.order:type_name -> loams.collection.v1.SortOrder
	2,   // 78: loams.collection.v1.PrimaryKeySort.order:type_name -> loams.collection.v1.SortOrder
	2,   // 79: loams.collection.v1.FieldSort.order:type_name -> loams.collection.v1.SortOrder
	3,   // 80: loams.collection.v1.FieldSort.missing:type_name -> loams.collection.v1.MissingOrder
	42,  // 81: loams.collection.v1.Highlight.fields:type_name -> loams.collection.v1.HighlightField
	45,  // 82: loams.collection.v1.SearchResponse.hits:type_name -> loams.collection.v1.Hit
	55,  // 83: loams.collection.v1.SearchResponse.total:type_name -> loams.collection.v1.TotalHits
	61,  // 84: loams.collection.v1.SearchResponse.aggregations:type_name -> google.protobuf.Value
	54,  // 85: loams.collection.v1.SearchResponse.groups:type_name -> loams.collection.v1.HitGroup
	62,  // 86: loams.collection.v1.SearchResponse.performance:type_name -> google.protobuf.Struct
	53,  // 87: loams.collection.v1.SearchResponse.expand:type_name -> loams.collection.v1.ExpandStats
	64,  // 88: loams.collection.v1.Hit.pk:type_name -> loams.collection.v1.DocumentId
	61,  // 89: loams.collection.v1.Hit.sort_values:type_name -> google.protobuf.Value
	62,  // 90: loams.collection.v1.Hit.source:type_name -> google.protobuf.Struct
	56,  // 91: loams.collection.v1.Hit.vectors:type_name -> loams.collection.v1.Hit.VectorsEntry
	57,  // 92: loams.collection.v1.Hit.sparse_vectors:type_name -> loams.collection.v1.Hit.SparseVectorsEntry
	58,  // 93: loams.collection.v1.Hit.highlight:type_name -> loams.collection.v1.Hit.HighlightEntry
	59,  // 94: loams.collection.v1.Hit.fields:type_name -> loams.collection.v1.Hit.FieldsEntry
	51,  // 95: loams.collection.v1.Hit.neighbors:type_name -> loams.collection.v1.Neighbor
	52,  // 96: loams.collection.v1.Hit.graph:type_name -> loams.collection.v1.GraphHit
	4,   // 97: loams.collection.v1.ExpandSpec.direction:type_name -> loams.collection.v1.ExpandDirection
	19,  // 98: loams.collection.v1.ExpandSpec.edge_filter:type_name -> loams.collection.v1.Query
	19,  // 99: loams.collection.v1.ExpandSpec.vertex_filter:type_name -> loams.collection.v1.Query
	62,  // 100: loams.collection.v1.ExpandSpec.select:type_name -> google.protobuf.Struct
	5,   // 101: loams.collection.v1.ExpandSpec.output:type_name -> loams.collection.v1.ExpandOutput
	48,  // 102: loams.collection.v1.RerankSpec.inherit:type_name -> loams.collection.v1.InheritRerank
	49,  // 103: loams.collection.v1.RerankSpec.vector:type_name -> loams.collection.v1.VectorRerank
	50,  // 104: loams.collection.v1.RerankSpec.model:type_name -> loams.collection.v1.ModelRerank
	64,  // 105: loams.collection.v1.Neighbor.key:type_name -> loams.collection.v1.DocumentId
	64,  // 106: loams.collection.v1.Neighbor.via:type_name -> loams.collection.v1.DocumentId
	64,  // 107: loams.collection.v1.Neighbor.edge:type_name -> loams.collection.v1.DocumentId
	62,  // 108: loams.collection.v1.Neighbor.source:type_name -> google.protobuf.Struct
	64,  // 109: loams.collection.v1.GraphHit.seeds:type_name -> loams.collection.v1.DocumentId
	61,  // 110: loams.collection.v1.HitGroup.key:type_name -> google.protobuf.Value
	45,  // 111: loams.collection.v1.HitGroup.hits:type_name -> loams.collection.v1.Hit
	6,   // 112: loams.collection.v1.TotalHits.relation:type_name -> loams.collection.v1.TotalRelation
	61,  // 113: loams.collection.v1.Hit.VectorsEntry.value:type_name -> google.protobuf.Value
	62,  // 114: loams.collection.v1.Hit.SparseVectorsEntry.value:type_name -> google.protobuf.Struct
	61,  // 115: loams.collection.v1.Hit.HighlightEntry.value:type_name -> google.protobuf.Value
	61,  // 116: loams.collection.v1.Hit.FieldsEntry.value:type_name -> google.protobuf.Value
	7,   // 117: loams.collection.v1.QueryService.Search:input_type -> loams.collection.v1.SearchRequest
	44,  // 118: loams.collection.v1.QueryService.Search:output_type -> loams.collection.v1.SearchResponse
	118, // [118:119] is the sub-list for method output_type
	117, // [117:118] is the sub-list for method input_type
	117, // [117:117] is the sub-list for extension type_name
	117, // [117:117] is the sub-list for extension extendee
	0,   // [0:117] is the sub-list for field type_name
}

func init() { file_loams_collection_v1_query_proto_init() }
func file_loams_collection_v1_query_proto_init() {
	if File_loams_collection_v1_query_proto != nil {
		return
	}
	file_loams_collection_v1_document_proto_init()
	file_loams_collection_v1_query_proto_msgTypes[0].OneofWrappers = []any{}
	file_loams_collection_v1_query_proto_msgTypes[1].OneofWrappers = []any{
		(*Retriever_Vector)(nil),
		(*Retriever_Text)(nil),
		(*Retriever_Fused)(nil),
		(*Retriever_Rescore)(nil),
		(*Retriever_Sparse)(nil),
	}
	file_loams_collection_v1_query_proto_msgTypes[7].OneofWrappers = []any{}
	file_loams_collection_v1_query_proto_msgTypes[9].OneofWrappers = []any{
		(*Fusion_Rrf)(nil),
		(*Fusion_Dbsf)(nil),
		(*Fusion_WeightedSum)(nil),
	}
	file_loams_collection_v1_query_proto_msgTypes[10].OneofWrappers = []any{}
	file_loams_collection_v1_query_proto_msgTypes[12].OneofWrappers = []any{
		(*Query_MatchAll)(nil),
		(*Query_MatchNone)(nil),
		(*Query_Match)(nil),
		(*Query_MatchPhrase)(nil),
		(*Query_MultiMatch)(nil),
		(*Query_Term)(nil),
		(*Query_Terms)(nil),
		(*Query_Range)(nil),
		(*Query_Exists)(nil),
		(*Query_IsNull)(nil),
		(*Query_IsEmpty)(nil),
		(*Query_ValuesCount)(nil),
		(*Query_Prefix)(nil),
		(*Query_Wildcard)(nil),
		(*Query_Fuzzy)(nil),
		(*Query_QueryString)(nil),
		(*Query_Bool)(nil),
		(*Query_Boost)(nil),
		(*Query_ConstantScore)(nil),
	}
	file_loams_collection_v1_query_proto_msgTypes[13].OneofWrappers = []any{}
	file_loams_collection_v1_query_proto_msgTypes[15].OneofWrappers = []any{}
	file_loams_collection_v1_query_proto_msgTypes[21].OneofWrappers = []any{}
	file_loams_collection_v1_query_proto_msgTypes[26].OneofWrappers = []any{}
	file_loams_collection_v1_query_proto_msgTypes[29].OneofWrappers = []any{
		(*Fuzziness_Auto)(nil),
		(*Fuzziness_Edits)(nil),
	}
	file_loams_collection_v1_query_proto_msgTypes[30].OneofWrappers = []any{
		(*SortKey_Score)(nil),
		(*SortKey_Pk)(nil),
		(*SortKey_Field)(nil),
	}
	file_loams_collection_v1_query_proto_msgTypes[35].OneofWrappers = []any{}
	file_loams_collection_v1_query_proto_msgTypes[36].OneofWrappers = []any{}
	file_loams_collection_v1_query_proto_msgTypes[40].OneofWrappers = []any{
		(*RerankSpec_Inherit)(nil),
		(*RerankSpec_Vector)(nil),
		(*RerankSpec_Model)(nil),
	}
	file_loams_collection_v1_query_proto_msgTypes[41].OneofWrappers = []any{}
	file_loams_collection_v1_query_proto_msgTypes[48].OneofWrappers = []any{}
	type x struct{}
	out := protoimpl.TypeBuilder{
		File: protoimpl.DescBuilder{
			GoPackagePath: reflect.TypeOf(x{}).PkgPath(),
			RawDescriptor: unsafe.Slice(unsafe.StringData(file_loams_collection_v1_query_proto_rawDesc), len(file_loams_collection_v1_query_proto_rawDesc)),
			NumEnums:      7,
			NumMessages:   53,
			NumExtensions: 0,
			NumServices:   1,
		},
		GoTypes:           file_loams_collection_v1_query_proto_goTypes,
		DependencyIndexes: file_loams_collection_v1_query_proto_depIdxs,
		EnumInfos:         file_loams_collection_v1_query_proto_enumTypes,
		MessageInfos:      file_loams_collection_v1_query_proto_msgTypes,
	}.Build()
	File_loams_collection_v1_query_proto = out.File
	file_loams_collection_v1_query_proto_goTypes = nil
	file_loams_collection_v1_query_proto_depIdxs = nil
}
