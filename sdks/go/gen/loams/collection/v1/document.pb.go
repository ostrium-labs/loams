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

// Code generated by protoc-gen-go. DO NOT EDIT.
// versions:
// 	protoc-gen-go v1.36.11
// 	protoc        (unknown)
// source: loams/collection/v1/document.proto

package collectionv1

import (
	protoreflect "google.golang.org/protobuf/reflect/protoreflect"
	protoimpl "google.golang.org/protobuf/runtime/protoimpl"
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

// The outcome of one op (Ruling 10).
type OpResult int32

const (
	OpResult_OP_RESULT_UNSPECIFIED OpResult = 0
	// The document was not there and now is.
	OpResult_OP_RESULT_CREATED OpResult = 1
	// The document was there and now holds this write.
	OpResult_OP_RESULT_UPDATED OpResult = 2
	// The document was there and now is not.
	OpResult_OP_RESULT_DELETED OpResult = 3
	// There was no such document (only reported with `report_existence`).
	OpResult_OP_RESULT_NOT_FOUND OpResult = 4
	// A patch whose result equals the document that is already there.
	OpResult_OP_RESULT_NOOP OpResult = 5
	// Written, without existence reporting.
	OpResult_OP_RESULT_ACCEPTED OpResult = 6
)

// Enum value maps for OpResult.
var (
	OpResult_name = map[int32]string{
		0: "OP_RESULT_UNSPECIFIED",
		1: "OP_RESULT_CREATED",
		2: "OP_RESULT_UPDATED",
		3: "OP_RESULT_DELETED",
		4: "OP_RESULT_NOT_FOUND",
		5: "OP_RESULT_NOOP",
		6: "OP_RESULT_ACCEPTED",
	}
	OpResult_value = map[string]int32{
		"OP_RESULT_UNSPECIFIED": 0,
		"OP_RESULT_CREATED":     1,
		"OP_RESULT_UPDATED":     2,
		"OP_RESULT_DELETED":     3,
		"OP_RESULT_NOT_FOUND":   4,
		"OP_RESULT_NOOP":        5,
		"OP_RESULT_ACCEPTED":    6,
	}
)

func (x OpResult) Enum() *OpResult {
	p := new(OpResult)
	*p = x
	return p
}

func (x OpResult) String() string {
	return protoimpl.X.EnumStringOf(x.Descriptor(), protoreflect.EnumNumber(x))
}

func (OpResult) Descriptor() protoreflect.EnumDescriptor {
	return file_loams_collection_v1_document_proto_enumTypes[0].Descriptor()
}

func (OpResult) Type() protoreflect.EnumType {
	return &file_loams_collection_v1_document_proto_enumTypes[0]
}

func (x OpResult) Number() protoreflect.EnumNumber {
	return protoreflect.EnumNumber(x)
}

// Deprecated: Use OpResult.Descriptor instead.
func (OpResult) EnumDescriptor() ([]byte, []int) {
	return file_loams_collection_v1_document_proto_rawDescGZIP(), []int{0}
}

type Freshness int32

const (
	Freshness_FRESHNESS_UNSPECIFIED Freshness = 0
	// Every write acknowledged before the read began.
	Freshness_FRESHNESS_STRONG Freshness = 1
	// Whatever the tail holds right now.
	Freshness_FRESHNESS_EVENTUAL Freshness = 2
)

// Enum value maps for Freshness.
var (
	Freshness_name = map[int32]string{
		0: "FRESHNESS_UNSPECIFIED",
		1: "FRESHNESS_STRONG",
		2: "FRESHNESS_EVENTUAL",
	}
	Freshness_value = map[string]int32{
		"FRESHNESS_UNSPECIFIED": 0,
		"FRESHNESS_STRONG":      1,
		"FRESHNESS_EVENTUAL":    2,
	}
)

func (x Freshness) Enum() *Freshness {
	p := new(Freshness)
	*p = x
	return p
}

func (x Freshness) String() string {
	return protoimpl.X.EnumStringOf(x.Descriptor(), protoreflect.EnumNumber(x))
}

func (Freshness) Descriptor() protoreflect.EnumDescriptor {
	return file_loams_collection_v1_document_proto_enumTypes[1].Descriptor()
}

func (Freshness) Type() protoreflect.EnumType {
	return &file_loams_collection_v1_document_proto_enumTypes[1]
}

func (x Freshness) Number() protoreflect.EnumNumber {
	return protoreflect.EnumNumber(x)
}

// Deprecated: Use Freshness.Descriptor instead.
func (Freshness) EnumDescriptor() ([]byte, []int) {
	return file_loams_collection_v1_document_proto_rawDescGZIP(), []int{1}
}

type WriteDocumentsRequest struct {
	state     protoimpl.MessageState `protogen:"open.v1"`
	Namespace string                 `protobuf:"bytes,1,opt,name=namespace,proto3" json:"namespace,omitempty"`
	// A collection name or an alias.
	Collection string `protobuf:"bytes,2,opt,name=collection,proto3" json:"collection,omitempty"`
	// Applied in order, and all of them or none of them.
	Ops []*WriteOp `protobuf:"bytes,3,rep,name=ops,proto3" json:"ops,omitempty"`
	// Whether `results` reports what each op did to an existing document
	// (`created`, `updated`, `deleted`, `not_found`, `noop`) rather than
	// `accepted`. Absent is `false`, which is the cheaper answer.
	ReportExistence bool `protobuf:"varint,4,opt,name=report_existence,json=reportExistence,proto3" json:"report_existence,omitempty"`
	// The key that makes a retry a replay (D610). Absent means the caller is not
	// retrying and every call is a new write.
	IdempotencyKey string `protobuf:"bytes,5,opt,name=idempotency_key,json=idempotencyKey,proto3" json:"idempotency_key,omitempty"`
	unknownFields  protoimpl.UnknownFields
	sizeCache      protoimpl.SizeCache
}

func (x *WriteDocumentsRequest) Reset() {
	*x = WriteDocumentsRequest{}
	mi := &file_loams_collection_v1_document_proto_msgTypes[0]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *WriteDocumentsRequest) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*WriteDocumentsRequest) ProtoMessage() {}

func (x *WriteDocumentsRequest) ProtoReflect() protoreflect.Message {
	mi := &file_loams_collection_v1_document_proto_msgTypes[0]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use WriteDocumentsRequest.ProtoReflect.Descriptor instead.
func (*WriteDocumentsRequest) Descriptor() ([]byte, []int) {
	return file_loams_collection_v1_document_proto_rawDescGZIP(), []int{0}
}

func (x *WriteDocumentsRequest) GetNamespace() string {
	if x != nil {
		return x.Namespace
	}
	return ""
}

func (x *WriteDocumentsRequest) GetCollection() string {
	if x != nil {
		return x.Collection
	}
	return ""
}

func (x *WriteDocumentsRequest) GetOps() []*WriteOp {
	if x != nil {
		return x.Ops
	}
	return nil
}

func (x *WriteDocumentsRequest) GetReportExistence() bool {
	if x != nil {
		return x.ReportExistence
	}
	return false
}

func (x *WriteDocumentsRequest) GetIdempotencyKey() string {
	if x != nil {
		return x.IdempotencyKey
	}
	return ""
}

type WriteDocumentsResponse struct {
	state protoimpl.MessageState `protogen:"open.v1"`
	// The writes' consistency token: at least these writes are acknowledged.
	// The same value is the `loams-consistency-token` response header.
	Token string `protobuf:"bytes,1,opt,name=token,proto3" json:"token,omitempty"`
	// One per op, in request order.
	Results []OpResult `protobuf:"varint,2,rep,packed,name=results,proto3,enum=loams.collection.v1.OpResult" json:"results,omitempty"`
	// One per op: where it was appended. An op that was not placed answers an
	// entry with no fields, which is how a `noop` reports itself.
	Positions []*OpPosition `protobuf:"bytes,3,rep,name=positions,proto3" json:"positions,omitempty"`
	// The unapplied-data backlog measured at admission, before this write's own
	// records. Also the `loams-unapplied-records` / `loams-unapplied-bytes`
	// response headers, which is where a non-Connect reader reads them.
	UnappliedRecords *uint64 `protobuf:"varint,4,opt,name=unapplied_records,json=unappliedRecords,proto3,oneof" json:"unapplied_records,omitempty"`
	UnappliedBytes   *uint64 `protobuf:"varint,5,opt,name=unapplied_bytes,json=unappliedBytes,proto3,oneof" json:"unapplied_bytes,omitempty"`
	unknownFields    protoimpl.UnknownFields
	sizeCache        protoimpl.SizeCache
}

func (x *WriteDocumentsResponse) Reset() {
	*x = WriteDocumentsResponse{}
	mi := &file_loams_collection_v1_document_proto_msgTypes[1]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *WriteDocumentsResponse) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*WriteDocumentsResponse) ProtoMessage() {}

func (x *WriteDocumentsResponse) ProtoReflect() protoreflect.Message {
	mi := &file_loams_collection_v1_document_proto_msgTypes[1]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use WriteDocumentsResponse.ProtoReflect.Descriptor instead.
func (*WriteDocumentsResponse) Descriptor() ([]byte, []int) {
	return file_loams_collection_v1_document_proto_rawDescGZIP(), []int{1}
}

func (x *WriteDocumentsResponse) GetToken() string {
	if x != nil {
		return x.Token
	}
	return ""
}

func (x *WriteDocumentsResponse) GetResults() []OpResult {
	if x != nil {
		return x.Results
	}
	return nil
}

func (x *WriteDocumentsResponse) GetPositions() []*OpPosition {
	if x != nil {
		return x.Positions
	}
	return nil
}

func (x *WriteDocumentsResponse) GetUnappliedRecords() uint64 {
	if x != nil && x.UnappliedRecords != nil {
		return *x.UnappliedRecords
	}
	return 0
}

func (x *WriteDocumentsResponse) GetUnappliedBytes() uint64 {
	if x != nil && x.UnappliedBytes != nil {
		return *x.UnappliedBytes
	}
	return 0
}

// One document change. A proto oneof, so the JSON is the REST route's
// `{"upsert": {…}}` / `{"delete": {…}}` / `{"patch": {…}}` unchanged, and an
// op that is none of the three is `invalid_argument`.
type WriteOp struct {
	state protoimpl.MessageState `protogen:"open.v1"`
	// Types that are valid to be assigned to Op:
	//
	//	*WriteOp_Upsert
	//	*WriteOp_Delete
	//	*WriteOp_Patch
	Op            isWriteOp_Op `protobuf_oneof:"op"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *WriteOp) Reset() {
	*x = WriteOp{}
	mi := &file_loams_collection_v1_document_proto_msgTypes[2]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *WriteOp) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*WriteOp) ProtoMessage() {}

func (x *WriteOp) ProtoReflect() protoreflect.Message {
	mi := &file_loams_collection_v1_document_proto_msgTypes[2]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use WriteOp.ProtoReflect.Descriptor instead.
func (*WriteOp) Descriptor() ([]byte, []int) {
	return file_loams_collection_v1_document_proto_rawDescGZIP(), []int{2}
}

func (x *WriteOp) GetOp() isWriteOp_Op {
	if x != nil {
		return x.Op
	}
	return nil
}

func (x *WriteOp) GetUpsert() *WriteDocument {
	if x != nil {
		if x, ok := x.Op.(*WriteOp_Upsert); ok {
			return x.Upsert
		}
	}
	return nil
}

func (x *WriteOp) GetDelete() *DeleteDocument {
	if x != nil {
		if x, ok := x.Op.(*WriteOp_Delete); ok {
			return x.Delete
		}
	}
	return nil
}

func (x *WriteOp) GetPatch() *Patch {
	if x != nil {
		if x, ok := x.Op.(*WriteOp_Patch); ok {
			return x.Patch
		}
	}
	return nil
}

type isWriteOp_Op interface {
	isWriteOp_Op()
}

type WriteOp_Upsert struct {
	// Writes the document under its id, replacing it if it is there.
	Upsert *WriteDocument `protobuf:"bytes,1,opt,name=upsert,proto3,oneof"`
}

type WriteOp_Delete struct {
	// Removes the document. Removing one that is not there is a `not_found`,
	// which is only reported with `report_existence`.
	Delete *DeleteDocument `protobuf:"bytes,2,opt,name=delete,proto3,oneof"`
}

type WriteOp_Patch struct {
	// Changes part of a document, and may create one (see `Patch.upsert`).
	Patch *Patch `protobuf:"bytes,3,opt,name=patch,proto3,oneof"`
}

func (*WriteOp_Upsert) isWriteOp_Op() {}

func (*WriteOp_Delete) isWriteOp_Op() {}

func (*WriteOp_Patch) isWriteOp_Op() {}

type WriteDocument struct {
	state protoimpl.MessageState `protogen:"open.v1"`
	Id    *DocumentId            `protobuf:"bytes,1,opt,name=id,proto3" json:"id,omitempty"`
	// The document's payload, as the native REST route's `_source` JSON. Absent
	// is an empty document.
	Source *structpb.Struct `protobuf:"bytes,2,opt,name=source,proto3" json:"source,omitempty"`
	// Dense vectors by name, as the REST route's JSON (`[1.0, 0.0, 0.0]`).
	Vectors map[string]*structpb.Value `protobuf:"bytes,3,rep,name=vectors,proto3" json:"vectors,omitempty" protobuf_key:"bytes,1,opt,name=key" protobuf_val:"bytes,2,opt,name=value"`
	// Sparse vectors by name, as the REST route's JSON (`{"indices": [1],
	// "values": [1.0]}`).
	SparseVectors map[string]*structpb.Struct `protobuf:"bytes,4,rep,name=sparse_vectors,json=sparseVectors,proto3" json:"sparse_vectors,omitempty" protobuf_key:"bytes,1,opt,name=key" protobuf_val:"bytes,2,opt,name=value"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *WriteDocument) Reset() {
	*x = WriteDocument{}
	mi := &file_loams_collection_v1_document_proto_msgTypes[3]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *WriteDocument) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*WriteDocument) ProtoMessage() {}

func (x *WriteDocument) ProtoReflect() protoreflect.Message {
	mi := &file_loams_collection_v1_document_proto_msgTypes[3]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use WriteDocument.ProtoReflect.Descriptor instead.
func (*WriteDocument) Descriptor() ([]byte, []int) {
	return file_loams_collection_v1_document_proto_rawDescGZIP(), []int{3}
}

func (x *WriteDocument) GetId() *DocumentId {
	if x != nil {
		return x.Id
	}
	return nil
}

func (x *WriteDocument) GetSource() *structpb.Struct {
	if x != nil {
		return x.Source
	}
	return nil
}

func (x *WriteDocument) GetVectors() map[string]*structpb.Value {
	if x != nil {
		return x.Vectors
	}
	return nil
}

func (x *WriteDocument) GetSparseVectors() map[string]*structpb.Struct {
	if x != nil {
		return x.SparseVectors
	}
	return nil
}

type DeleteDocument struct {
	state         protoimpl.MessageState `protogen:"open.v1"`
	Id            *DocumentId            `protobuf:"bytes,1,opt,name=id,proto3" json:"id,omitempty"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *DeleteDocument) Reset() {
	*x = DeleteDocument{}
	mi := &file_loams_collection_v1_document_proto_msgTypes[4]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *DeleteDocument) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*DeleteDocument) ProtoMessage() {}

func (x *DeleteDocument) ProtoReflect() protoreflect.Message {
	mi := &file_loams_collection_v1_document_proto_msgTypes[4]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use DeleteDocument.ProtoReflect.Descriptor instead.
func (*DeleteDocument) Descriptor() ([]byte, []int) {
	return file_loams_collection_v1_document_proto_rawDescGZIP(), []int{4}
}

func (x *DeleteDocument) GetId() *DocumentId {
	if x != nil {
		return x.Id
	}
	return nil
}

// One document's change. `mode` and every other spelling here are the REST
// route's, because a reader switches on them verbatim (the same call
// `loams.collection.v1` made for `ScanColumn.role`).
type Patch struct {
	state protoimpl.MessageState `protogen:"open.v1"`
	Id    *DocumentId            `protobuf:"bytes,1,opt,name=id,proto3" json:"id,omitempty"`
	// `merge_deep` (the default), `merge_top` or `replace`. A string rather than
	// an enum so the REST spelling survives verbatim; any other value is
	// `invalid_argument`.
	Mode string `protobuf:"bytes,2,opt,name=mode,proto3" json:"mode,omitempty"`
	// The keys to merge. `merge_deep` merges nested objects, `merge_top`
	// replaces them and `replace` replaces the document.
	Source *structpb.Struct `protobuf:"bytes,3,opt,name=source,proto3" json:"source,omitempty"`
	// The top-level source keys to remove.
	DeleteKeys []string `protobuf:"bytes,4,rep,name=delete_keys,json=deleteKeys,proto3" json:"delete_keys,omitempty"`
	// Vectors to set, by name. A `null` value **removes** the vector, which is
	// why the value is a `Value` and not a `ListValue` (see the header).
	Vectors map[string]*structpb.Value `protobuf:"bytes,5,rep,name=vectors,proto3" json:"vectors,omitempty" protobuf_key:"bytes,1,opt,name=key" protobuf_val:"bytes,2,opt,name=value"`
	// Sparse vectors to set, by name; a `null` value removes one.
	SparseVectors map[string]*structpb.Struct `protobuf:"bytes,6,rep,name=sparse_vectors,json=sparseVectors,proto3" json:"sparse_vectors,omitempty" protobuf_key:"bytes,1,opt,name=key" protobuf_val:"bytes,2,opt,name=value"`
	// The document to write when the patched document is not there. Absent
	// leaves a missing document missing, which is what a `patch_by_filter`
	// never does (it patches what the filter matched).
	Upsert        *WriteDocument `protobuf:"bytes,7,opt,name=upsert,proto3" json:"upsert,omitempty"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *Patch) Reset() {
	*x = Patch{}
	mi := &file_loams_collection_v1_document_proto_msgTypes[5]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *Patch) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*Patch) ProtoMessage() {}

func (x *Patch) ProtoReflect() protoreflect.Message {
	mi := &file_loams_collection_v1_document_proto_msgTypes[5]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use Patch.ProtoReflect.Descriptor instead.
func (*Patch) Descriptor() ([]byte, []int) {
	return file_loams_collection_v1_document_proto_rawDescGZIP(), []int{5}
}

func (x *Patch) GetId() *DocumentId {
	if x != nil {
		return x.Id
	}
	return nil
}

func (x *Patch) GetMode() string {
	if x != nil {
		return x.Mode
	}
	return ""
}

func (x *Patch) GetSource() *structpb.Struct {
	if x != nil {
		return x.Source
	}
	return nil
}

func (x *Patch) GetDeleteKeys() []string {
	if x != nil {
		return x.DeleteKeys
	}
	return nil
}

func (x *Patch) GetVectors() map[string]*structpb.Value {
	if x != nil {
		return x.Vectors
	}
	return nil
}

func (x *Patch) GetSparseVectors() map[string]*structpb.Struct {
	if x != nil {
		return x.SparseVectors
	}
	return nil
}

func (x *Patch) GetUpsert() *WriteDocument {
	if x != nil {
		return x.Upsert
	}
	return nil
}

// One document's id, as the three `PrimaryKey` variants (see the header).
type DocumentId struct {
	state protoimpl.MessageState `protogen:"open.v1"`
	// Types that are valid to be assigned to Id:
	//
	//	*DocumentId_Uint
	//	*DocumentId_String_
	//	*DocumentId_Uuid
	Id            isDocumentId_Id `protobuf_oneof:"id"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *DocumentId) Reset() {
	*x = DocumentId{}
	mi := &file_loams_collection_v1_document_proto_msgTypes[6]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *DocumentId) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*DocumentId) ProtoMessage() {}

func (x *DocumentId) ProtoReflect() protoreflect.Message {
	mi := &file_loams_collection_v1_document_proto_msgTypes[6]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use DocumentId.ProtoReflect.Descriptor instead.
func (*DocumentId) Descriptor() ([]byte, []int) {
	return file_loams_collection_v1_document_proto_rawDescGZIP(), []int{6}
}

func (x *DocumentId) GetId() isDocumentId_Id {
	if x != nil {
		return x.Id
	}
	return nil
}

func (x *DocumentId) GetUint() uint64 {
	if x != nil {
		if x, ok := x.Id.(*DocumentId_Uint); ok {
			return x.Uint
		}
	}
	return 0
}

func (x *DocumentId) GetString_() string {
	if x != nil {
		if x, ok := x.Id.(*DocumentId_String_); ok {
			return x.String_
		}
	}
	return ""
}

func (x *DocumentId) GetUuid() string {
	if x != nil {
		if x, ok := x.Id.(*DocumentId_Uuid); ok {
			return x.Uuid
		}
	}
	return ""
}

type isDocumentId_Id interface {
	isDocumentId_Id()
}

type DocumentId_Uint struct {
	// An id in `0..=2^64−1`, as a decimal string in JSON.
	Uint uint64 `protobuf:"varint,1,opt,name=uint,proto3,oneof"`
}

type DocumentId_String_ struct {
	String_ string `protobuf:"bytes,2,opt,name=string,proto3,oneof"`
}

type DocumentId_Uuid struct {
	// The REST spelling, `8-4-4-4-12` in any case.
	Uuid string `protobuf:"bytes,3,opt,name=uuid,proto3,oneof"`
}

func (*DocumentId_Uint) isDocumentId_Id() {}

func (*DocumentId_String_) isDocumentId_Id() {}

func (*DocumentId_Uuid) isDocumentId_Id() {}

// Where one op was appended.
//
// An op that was not placed answers an entry with **no `seqNo` at all**, which
// is why `seq_no` is `optional` and not a plain `uint64`: offset 0 is the
// first record of a partition and a real answer, and proto3 JSON omitting a
// zero would make it indistinguishable from "this op was not placed".
type OpPosition struct {
	state         protoimpl.MessageState `protogen:"open.v1"`
	Partition     uint32                 `protobuf:"varint,1,opt,name=partition,proto3" json:"partition,omitempty"`
	SeqNo         *uint64                `protobuf:"varint,2,opt,name=seq_no,json=seqNo,proto3,oneof" json:"seq_no,omitempty"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *OpPosition) Reset() {
	*x = OpPosition{}
	mi := &file_loams_collection_v1_document_proto_msgTypes[7]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *OpPosition) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*OpPosition) ProtoMessage() {}

func (x *OpPosition) ProtoReflect() protoreflect.Message {
	mi := &file_loams_collection_v1_document_proto_msgTypes[7]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use OpPosition.ProtoReflect.Descriptor instead.
func (*OpPosition) Descriptor() ([]byte, []int) {
	return file_loams_collection_v1_document_proto_rawDescGZIP(), []int{7}
}

func (x *OpPosition) GetPartition() uint32 {
	if x != nil {
		return x.Partition
	}
	return 0
}

func (x *OpPosition) GetSeqNo() uint64 {
	if x != nil && x.SeqNo != nil {
		return *x.SeqNo
	}
	return 0
}

type GetDocumentsRequest struct {
	state     protoimpl.MessageState `protogen:"open.v1"`
	Namespace string                 `protobuf:"bytes,1,opt,name=namespace,proto3" json:"namespace,omitempty"`
	// A collection name or an alias.
	Collection string `protobuf:"bytes,2,opt,name=collection,proto3" json:"collection,omitempty"`
	// The documents to get. The answer has one entry per id, in this order.
	Ids []*DocumentId `protobuf:"bytes,3,rep,name=ids,proto3" json:"ids,omitempty"`
	// What to return of each document, as the REST route's projection JSON
	// (`{"source": "all" | "none" | {"include": [], "exclude": []},
	// "vectors": [], "fields": []}`). Absent is everything.
	Select        *structpb.Struct `protobuf:"bytes,4,opt,name=select,proto3" json:"select,omitempty"`
	Consistency   *Consistency     `protobuf:"bytes,5,opt,name=consistency,proto3" json:"consistency,omitempty"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *GetDocumentsRequest) Reset() {
	*x = GetDocumentsRequest{}
	mi := &file_loams_collection_v1_document_proto_msgTypes[8]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *GetDocumentsRequest) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*GetDocumentsRequest) ProtoMessage() {}

func (x *GetDocumentsRequest) ProtoReflect() protoreflect.Message {
	mi := &file_loams_collection_v1_document_proto_msgTypes[8]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use GetDocumentsRequest.ProtoReflect.Descriptor instead.
func (*GetDocumentsRequest) Descriptor() ([]byte, []int) {
	return file_loams_collection_v1_document_proto_rawDescGZIP(), []int{8}
}

func (x *GetDocumentsRequest) GetNamespace() string {
	if x != nil {
		return x.Namespace
	}
	return ""
}

func (x *GetDocumentsRequest) GetCollection() string {
	if x != nil {
		return x.Collection
	}
	return ""
}

func (x *GetDocumentsRequest) GetIds() []*DocumentId {
	if x != nil {
		return x.Ids
	}
	return nil
}

func (x *GetDocumentsRequest) GetSelect() *structpb.Struct {
	if x != nil {
		return x.Select
	}
	return nil
}

func (x *GetDocumentsRequest) GetConsistency() *Consistency {
	if x != nil {
		return x.Consistency
	}
	return nil
}

type GetDocumentsResponse struct {
	state protoimpl.MessageState `protogen:"open.v1"`
	// One per requested id, in request order. An entry with no fields is a
	// document that is not there.
	Documents []*Document `protobuf:"bytes,1,rep,name=documents,proto3" json:"documents,omitempty"`
	// The state the documents were read at, and a `consistency.at_least` that
	// reads this state again. The same value is the
	// `loams-consistency-token` response header.
	ReadToken     string `protobuf:"bytes,2,opt,name=read_token,json=readToken,proto3" json:"read_token,omitempty"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *GetDocumentsResponse) Reset() {
	*x = GetDocumentsResponse{}
	mi := &file_loams_collection_v1_document_proto_msgTypes[9]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *GetDocumentsResponse) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*GetDocumentsResponse) ProtoMessage() {}

func (x *GetDocumentsResponse) ProtoReflect() protoreflect.Message {
	mi := &file_loams_collection_v1_document_proto_msgTypes[9]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use GetDocumentsResponse.ProtoReflect.Descriptor instead.
func (*GetDocumentsResponse) Descriptor() ([]byte, []int) {
	return file_loams_collection_v1_document_proto_rawDescGZIP(), []int{9}
}

func (x *GetDocumentsResponse) GetDocuments() []*Document {
	if x != nil {
		return x.Documents
	}
	return nil
}

func (x *GetDocumentsResponse) GetReadToken() string {
	if x != nil {
		return x.ReadToken
	}
	return ""
}

type ScrollDocumentsRequest struct {
	state     protoimpl.MessageState `protogen:"open.v1"`
	Namespace string                 `protobuf:"bytes,1,opt,name=namespace,proto3" json:"namespace,omitempty"`
	// A collection name or an alias.
	Collection string `protobuf:"bytes,2,opt,name=collection,proto3" json:"collection,omitempty"`
	// The native query JSON, as the REST route takes it. Absent is every
	// document. This stays a `Value` while `query.proto` types the filter IR for
	// `QueryService/Search`: the tests that shipped this RPC send the bare string
	// `"match_all"`, which a typed oneof cannot spell, and `SearchRequest.filter`
	// is where a typed filter lands (API1 Task 4).
	Filter *structpb.Value `protobuf:"bytes,3,opt,name=filter,proto3" json:"filter,omitempty"`
	// The last document of the previous page; absent starts at the first.
	After *DocumentId `protobuf:"bytes,4,opt,name=after,proto3" json:"after,omitempty"`
	// The page size; absent or `0` is the server's default.
	Limit       *uint32          `protobuf:"varint,5,opt,name=limit,proto3,oneof" json:"limit,omitempty"`
	Select      *structpb.Struct `protobuf:"bytes,6,opt,name=select,proto3" json:"select,omitempty"`
	Consistency *Consistency     `protobuf:"bytes,7,opt,name=consistency,proto3" json:"consistency,omitempty"`
	// The AIP-158 spelling of `after`, and the **same cursor**: a `DocumentId`,
	// encoded exactly as `after` and `ScrollDocumentsResponse.next` encode it, so
	// there is one cursor on the wire and the two fields are interchangeable —
	// a `next` handed straight back as a `page_token` reads the same page. A
	// request that sets both names different cursors is `invalid_argument`.
	//
	// It is an addition rather than a change of shape: plan ruling 2 offers
	// `page_token` as an alternative to server-streaming `ScrollDocuments`, and
	// Q614 — which of the two the method is — is unanswered, so the shipped
	// unary RPC keeps its shape rather than breaking for an open question. True
	// server-streaming, if Q614 says so, is a separate additive RPC.
	PageToken *DocumentId `protobuf:"bytes,8,opt,name=page_token,json=pageToken,proto3" json:"page_token,omitempty"`
	// The filter as a typed `loams.collection.v1.Query`, in its proto3 JSON
	// (`{"term": {"field": "tags", "value": "policy"}}`, `{"matchAll": {}}`):
	// the shape the SDKs' `Filter` builder writes and `SearchRequest.filter`
	// takes. A `Struct` and not a `Query` because `query.proto` imports this
	// file, so this file cannot import it back (SDKP ruling P0a-1). Setting both
	// this and `filter` is `invalid_argument` with reason `filter_conflict`.
	Where         *structpb.Struct `protobuf:"bytes,9,opt,name=where,proto3" json:"where,omitempty"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *ScrollDocumentsRequest) Reset() {
	*x = ScrollDocumentsRequest{}
	mi := &file_loams_collection_v1_document_proto_msgTypes[10]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *ScrollDocumentsRequest) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*ScrollDocumentsRequest) ProtoMessage() {}

func (x *ScrollDocumentsRequest) ProtoReflect() protoreflect.Message {
	mi := &file_loams_collection_v1_document_proto_msgTypes[10]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use ScrollDocumentsRequest.ProtoReflect.Descriptor instead.
func (*ScrollDocumentsRequest) Descriptor() ([]byte, []int) {
	return file_loams_collection_v1_document_proto_rawDescGZIP(), []int{10}
}

func (x *ScrollDocumentsRequest) GetNamespace() string {
	if x != nil {
		return x.Namespace
	}
	return ""
}

func (x *ScrollDocumentsRequest) GetCollection() string {
	if x != nil {
		return x.Collection
	}
	return ""
}

func (x *ScrollDocumentsRequest) GetFilter() *structpb.Value {
	if x != nil {
		return x.Filter
	}
	return nil
}

func (x *ScrollDocumentsRequest) GetAfter() *DocumentId {
	if x != nil {
		return x.After
	}
	return nil
}

func (x *ScrollDocumentsRequest) GetLimit() uint32 {
	if x != nil && x.Limit != nil {
		return *x.Limit
	}
	return 0
}

func (x *ScrollDocumentsRequest) GetSelect() *structpb.Struct {
	if x != nil {
		return x.Select
	}
	return nil
}

func (x *ScrollDocumentsRequest) GetConsistency() *Consistency {
	if x != nil {
		return x.Consistency
	}
	return nil
}

func (x *ScrollDocumentsRequest) GetPageToken() *DocumentId {
	if x != nil {
		return x.PageToken
	}
	return nil
}

func (x *ScrollDocumentsRequest) GetWhere() *structpb.Struct {
	if x != nil {
		return x.Where
	}
	return nil
}

type ScrollDocumentsResponse struct {
	state protoimpl.MessageState `protogen:"open.v1"`
	// In primary-key order.
	Documents []*Document `protobuf:"bytes,1,rep,name=documents,proto3" json:"documents,omitempty"`
	// The id to continue after; absent on the last page.
	Next          *DocumentId `protobuf:"bytes,2,opt,name=next,proto3" json:"next,omitempty"`
	ReadToken     string      `protobuf:"bytes,3,opt,name=read_token,json=readToken,proto3" json:"read_token,omitempty"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *ScrollDocumentsResponse) Reset() {
	*x = ScrollDocumentsResponse{}
	mi := &file_loams_collection_v1_document_proto_msgTypes[11]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *ScrollDocumentsResponse) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*ScrollDocumentsResponse) ProtoMessage() {}

func (x *ScrollDocumentsResponse) ProtoReflect() protoreflect.Message {
	mi := &file_loams_collection_v1_document_proto_msgTypes[11]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use ScrollDocumentsResponse.ProtoReflect.Descriptor instead.
func (*ScrollDocumentsResponse) Descriptor() ([]byte, []int) {
	return file_loams_collection_v1_document_proto_rawDescGZIP(), []int{11}
}

func (x *ScrollDocumentsResponse) GetDocuments() []*Document {
	if x != nil {
		return x.Documents
	}
	return nil
}

func (x *ScrollDocumentsResponse) GetNext() *DocumentId {
	if x != nil {
		return x.Next
	}
	return nil
}

func (x *ScrollDocumentsResponse) GetReadToken() string {
	if x != nil {
		return x.ReadToken
	}
	return ""
}

type CountDocumentsRequest struct {
	state     protoimpl.MessageState `protogen:"open.v1"`
	Namespace string                 `protobuf:"bytes,1,opt,name=namespace,proto3" json:"namespace,omitempty"`
	// A collection name or an alias.
	Collection  string          `protobuf:"bytes,2,opt,name=collection,proto3" json:"collection,omitempty"`
	Filter      *structpb.Value `protobuf:"bytes,3,opt,name=filter,proto3" json:"filter,omitempty"`
	Consistency *Consistency    `protobuf:"bytes,4,opt,name=consistency,proto3" json:"consistency,omitempty"`
	// The filter as a typed `loams.collection.v1.Query`, in its proto3 JSON
	// (`{"term": {"field": "tags", "value": "policy"}}`, `{"matchAll": {}}`):
	// the shape the SDKs' `Filter` builder writes and `SearchRequest.filter`
	// takes. A `Struct` and not a `Query` because `query.proto` imports this
	// file, so this file cannot import it back (SDKP ruling P0a-1). Setting both
	// this and `filter` is `invalid_argument` with reason `filter_conflict`.
	Where         *structpb.Struct `protobuf:"bytes,5,opt,name=where,proto3" json:"where,omitempty"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *CountDocumentsRequest) Reset() {
	*x = CountDocumentsRequest{}
	mi := &file_loams_collection_v1_document_proto_msgTypes[12]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *CountDocumentsRequest) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*CountDocumentsRequest) ProtoMessage() {}

func (x *CountDocumentsRequest) ProtoReflect() protoreflect.Message {
	mi := &file_loams_collection_v1_document_proto_msgTypes[12]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use CountDocumentsRequest.ProtoReflect.Descriptor instead.
func (*CountDocumentsRequest) Descriptor() ([]byte, []int) {
	return file_loams_collection_v1_document_proto_rawDescGZIP(), []int{12}
}

func (x *CountDocumentsRequest) GetNamespace() string {
	if x != nil {
		return x.Namespace
	}
	return ""
}

func (x *CountDocumentsRequest) GetCollection() string {
	if x != nil {
		return x.Collection
	}
	return ""
}

func (x *CountDocumentsRequest) GetFilter() *structpb.Value {
	if x != nil {
		return x.Filter
	}
	return nil
}

func (x *CountDocumentsRequest) GetConsistency() *Consistency {
	if x != nil {
		return x.Consistency
	}
	return nil
}

func (x *CountDocumentsRequest) GetWhere() *structpb.Struct {
	if x != nil {
		return x.Where
	}
	return nil
}

type CountDocumentsResponse struct {
	state protoimpl.MessageState `protogen:"open.v1"`
	// How many documents matched, as a decimal string in JSON. `optional` and
	// always set: "nothing matched" is a count, not an absent field, and proto3
	// JSON omitting a zero would make an empty result read as "unanswered".
	Count         *uint64 `protobuf:"varint,1,opt,name=count,proto3,oneof" json:"count,omitempty"`
	ReadToken     string  `protobuf:"bytes,2,opt,name=read_token,json=readToken,proto3" json:"read_token,omitempty"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *CountDocumentsResponse) Reset() {
	*x = CountDocumentsResponse{}
	mi := &file_loams_collection_v1_document_proto_msgTypes[13]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *CountDocumentsResponse) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*CountDocumentsResponse) ProtoMessage() {}

func (x *CountDocumentsResponse) ProtoReflect() protoreflect.Message {
	mi := &file_loams_collection_v1_document_proto_msgTypes[13]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use CountDocumentsResponse.ProtoReflect.Descriptor instead.
func (*CountDocumentsResponse) Descriptor() ([]byte, []int) {
	return file_loams_collection_v1_document_proto_rawDescGZIP(), []int{13}
}

func (x *CountDocumentsResponse) GetCount() uint64 {
	if x != nil && x.Count != nil {
		return *x.Count
	}
	return 0
}

func (x *CountDocumentsResponse) GetReadToken() string {
	if x != nil {
		return x.ReadToken
	}
	return ""
}

type DeleteByFilterRequest struct {
	state     protoimpl.MessageState `protogen:"open.v1"`
	Namespace string                 `protobuf:"bytes,1,opt,name=namespace,proto3" json:"namespace,omitempty"`
	// A collection name or an alias.
	Collection string `protobuf:"bytes,2,opt,name=collection,proto3" json:"collection,omitempty"`
	// The native query JSON, as the REST route takes it. Required: there is no
	// "delete everything" spelling, and `match_all` is the caller's to write.
	Filter *structpb.Value `protobuf:"bytes,3,opt,name=filter,proto3" json:"filter,omitempty"`
	// The most rows this call may write. Absent is the kind's ceiling; `0` is
	// `invalid_argument`, because a call that may write nothing is a mistake
	// rather than a no-op.
	MaxRows *uint64 `protobuf:"varint,4,opt,name=max_rows,json=maxRows,proto3,oneof" json:"max_rows,omitempty"`
	// Whether a call matching more than `max_rows` writes what fits and answers a
	// cursor instead of failing. Absent is `false`: a call over its limit fails
	// before writing anything.
	AllowPartial bool `protobuf:"varint,5,opt,name=allow_partial,json=allowPartial,proto3" json:"allow_partial,omitempty"`
	// The `cursor` of the previous partial call, which resumes at the same pin.
	Cursor      *FilterWriteCursor `protobuf:"bytes,6,opt,name=cursor,proto3" json:"cursor,omitempty"`
	Consistency *Consistency       `protobuf:"bytes,7,opt,name=consistency,proto3" json:"consistency,omitempty"`
	// The filter as a typed `loams.collection.v1.Query`, in its proto3 JSON
	// (`{"term": {"field": "tags", "value": "policy"}}`, `{"matchAll": {}}`):
	// the shape the SDKs' `Filter` builder writes and `SearchRequest.filter`
	// takes. A `Struct` and not a `Query` because `query.proto` imports this
	// file, so this file cannot import it back (SDKP ruling P0a-1). Setting both
	// this and `filter` is `invalid_argument` with reason `filter_conflict`.
	Where         *structpb.Struct `protobuf:"bytes,8,opt,name=where,proto3" json:"where,omitempty"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *DeleteByFilterRequest) Reset() {
	*x = DeleteByFilterRequest{}
	mi := &file_loams_collection_v1_document_proto_msgTypes[14]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *DeleteByFilterRequest) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*DeleteByFilterRequest) ProtoMessage() {}

func (x *DeleteByFilterRequest) ProtoReflect() protoreflect.Message {
	mi := &file_loams_collection_v1_document_proto_msgTypes[14]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use DeleteByFilterRequest.ProtoReflect.Descriptor instead.
func (*DeleteByFilterRequest) Descriptor() ([]byte, []int) {
	return file_loams_collection_v1_document_proto_rawDescGZIP(), []int{14}
}

func (x *DeleteByFilterRequest) GetNamespace() string {
	if x != nil {
		return x.Namespace
	}
	return ""
}

func (x *DeleteByFilterRequest) GetCollection() string {
	if x != nil {
		return x.Collection
	}
	return ""
}

func (x *DeleteByFilterRequest) GetFilter() *structpb.Value {
	if x != nil {
		return x.Filter
	}
	return nil
}

func (x *DeleteByFilterRequest) GetMaxRows() uint64 {
	if x != nil && x.MaxRows != nil {
		return *x.MaxRows
	}
	return 0
}

func (x *DeleteByFilterRequest) GetAllowPartial() bool {
	if x != nil {
		return x.AllowPartial
	}
	return false
}

func (x *DeleteByFilterRequest) GetCursor() *FilterWriteCursor {
	if x != nil {
		return x.Cursor
	}
	return nil
}

func (x *DeleteByFilterRequest) GetConsistency() *Consistency {
	if x != nil {
		return x.Consistency
	}
	return nil
}

func (x *DeleteByFilterRequest) GetWhere() *structpb.Struct {
	if x != nil {
		return x.Where
	}
	return nil
}

type PatchByFilterRequest struct {
	state     protoimpl.MessageState `protogen:"open.v1"`
	Namespace string                 `protobuf:"bytes,1,opt,name=namespace,proto3" json:"namespace,omitempty"`
	// A collection name or an alias.
	Collection string          `protobuf:"bytes,2,opt,name=collection,proto3" json:"collection,omitempty"`
	Filter     *structpb.Value `protobuf:"bytes,3,opt,name=filter,proto3" json:"filter,omitempty"`
	// What to change on each match, in the REST route's form. `id` and `upsert`
	// are refused: a filter write patches what the filter matched and never
	// creates a document.
	Patch        *Patch             `protobuf:"bytes,4,opt,name=patch,proto3" json:"patch,omitempty"`
	MaxRows      *uint64            `protobuf:"varint,5,opt,name=max_rows,json=maxRows,proto3,oneof" json:"max_rows,omitempty"`
	AllowPartial bool               `protobuf:"varint,6,opt,name=allow_partial,json=allowPartial,proto3" json:"allow_partial,omitempty"`
	Cursor       *FilterWriteCursor `protobuf:"bytes,7,opt,name=cursor,proto3" json:"cursor,omitempty"`
	Consistency  *Consistency       `protobuf:"bytes,8,opt,name=consistency,proto3" json:"consistency,omitempty"`
	// The filter as a typed `loams.collection.v1.Query`, in its proto3 JSON
	// (`{"term": {"field": "tags", "value": "policy"}}`, `{"matchAll": {}}`):
	// the shape the SDKs' `Filter` builder writes and `SearchRequest.filter`
	// takes. A `Struct` and not a `Query` because `query.proto` imports this
	// file, so this file cannot import it back (SDKP ruling P0a-1). Setting both
	// this and `filter` is `invalid_argument` with reason `filter_conflict`.
	Where         *structpb.Struct `protobuf:"bytes,9,opt,name=where,proto3" json:"where,omitempty"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *PatchByFilterRequest) Reset() {
	*x = PatchByFilterRequest{}
	mi := &file_loams_collection_v1_document_proto_msgTypes[15]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *PatchByFilterRequest) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*PatchByFilterRequest) ProtoMessage() {}

func (x *PatchByFilterRequest) ProtoReflect() protoreflect.Message {
	mi := &file_loams_collection_v1_document_proto_msgTypes[15]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use PatchByFilterRequest.ProtoReflect.Descriptor instead.
func (*PatchByFilterRequest) Descriptor() ([]byte, []int) {
	return file_loams_collection_v1_document_proto_rawDescGZIP(), []int{15}
}

func (x *PatchByFilterRequest) GetNamespace() string {
	if x != nil {
		return x.Namespace
	}
	return ""
}

func (x *PatchByFilterRequest) GetCollection() string {
	if x != nil {
		return x.Collection
	}
	return ""
}

func (x *PatchByFilterRequest) GetFilter() *structpb.Value {
	if x != nil {
		return x.Filter
	}
	return nil
}

func (x *PatchByFilterRequest) GetPatch() *Patch {
	if x != nil {
		return x.Patch
	}
	return nil
}

func (x *PatchByFilterRequest) GetMaxRows() uint64 {
	if x != nil && x.MaxRows != nil {
		return *x.MaxRows
	}
	return 0
}

func (x *PatchByFilterRequest) GetAllowPartial() bool {
	if x != nil {
		return x.AllowPartial
	}
	return false
}

func (x *PatchByFilterRequest) GetCursor() *FilterWriteCursor {
	if x != nil {
		return x.Cursor
	}
	return nil
}

func (x *PatchByFilterRequest) GetConsistency() *Consistency {
	if x != nil {
		return x.Consistency
	}
	return nil
}

func (x *PatchByFilterRequest) GetWhere() *structpb.Struct {
	if x != nil {
		return x.Where
	}
	return nil
}

// The answer to one filter write. Both filter writes answer this, because
// every number in it is the same question either way: how much the filter
// matched, how much this call wrote, and the state it wrote at.
type FilterWriteResponse struct {
	state protoimpl.MessageState `protogen:"open.v1"`
	// Documents matching `filter` at the pin. The four counters are `optional`
	// and always set, for the reason `CountDocumentsResponse.count` gives: a
	// count of zero is a count, and proto3 JSON omitting it would read as
	// "unanswered".
	Matched *uint64 `protobuf:"varint,1,opt,name=matched,proto3,oneof" json:"matched,omitempty"`
	// Documents this call deleted or patched.
	Affected *uint64 `protobuf:"varint,2,opt,name=affected,proto3,oneof" json:"affected,omitempty"`
	// Keys this call's batches wrote: `affected`, and the keys found deleted
	// since the pin or (for a patch) already patched. It is what a caller's own
	// row budget counts.
	Written *uint64 `protobuf:"varint,3,opt,name=written,proto3,oneof" json:"written,omitempty"`
	// How many batches this call wrote.
	Batches *uint64 `protobuf:"varint,4,opt,name=batches,proto3,oneof" json:"batches,omitempty"`
	// Whether the filter still matches rows this call did not reach.
	RowsRemaining bool `protobuf:"varint,5,opt,name=rows_remaining,json=rowsRemaining,proto3" json:"rows_remaining,omitempty"`
	// The cursor that finishes the write at the same pin. Present exactly when
	// `rows_remaining`.
	Cursor *FilterWriteCursor `protobuf:"bytes,6,opt,name=cursor,proto3" json:"cursor,omitempty"`
	// Covers the pin and every batch of this call, so a read at it sees this
	// write. The same value is the `loams-consistency-token` response header.
	Token string `protobuf:"bytes,7,opt,name=token,proto3" json:"token,omitempty"`
	// The snapshot the filter was evaluated at, which the `cursor` resumes at.
	Pin *Pin `protobuf:"bytes,8,opt,name=pin,proto3" json:"pin,omitempty"`
	// How long to wait, when the call stopped at its deadline on a batch refused
	// for backpressure. Absent when the call finished. The same number is the
	// `Retry-After` header of the call that carries it.
	RetryAfterMs  *uint64 `protobuf:"varint,9,opt,name=retry_after_ms,json=retryAfterMs,proto3,oneof" json:"retry_after_ms,omitempty"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *FilterWriteResponse) Reset() {
	*x = FilterWriteResponse{}
	mi := &file_loams_collection_v1_document_proto_msgTypes[16]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *FilterWriteResponse) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*FilterWriteResponse) ProtoMessage() {}

func (x *FilterWriteResponse) ProtoReflect() protoreflect.Message {
	mi := &file_loams_collection_v1_document_proto_msgTypes[16]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use FilterWriteResponse.ProtoReflect.Descriptor instead.
func (*FilterWriteResponse) Descriptor() ([]byte, []int) {
	return file_loams_collection_v1_document_proto_rawDescGZIP(), []int{16}
}

func (x *FilterWriteResponse) GetMatched() uint64 {
	if x != nil && x.Matched != nil {
		return *x.Matched
	}
	return 0
}

func (x *FilterWriteResponse) GetAffected() uint64 {
	if x != nil && x.Affected != nil {
		return *x.Affected
	}
	return 0
}

func (x *FilterWriteResponse) GetWritten() uint64 {
	if x != nil && x.Written != nil {
		return *x.Written
	}
	return 0
}

func (x *FilterWriteResponse) GetBatches() uint64 {
	if x != nil && x.Batches != nil {
		return *x.Batches
	}
	return 0
}

func (x *FilterWriteResponse) GetRowsRemaining() bool {
	if x != nil {
		return x.RowsRemaining
	}
	return false
}

func (x *FilterWriteResponse) GetCursor() *FilterWriteCursor {
	if x != nil {
		return x.Cursor
	}
	return nil
}

func (x *FilterWriteResponse) GetToken() string {
	if x != nil {
		return x.Token
	}
	return ""
}

func (x *FilterWriteResponse) GetPin() *Pin {
	if x != nil {
		return x.Pin
	}
	return nil
}

func (x *FilterWriteResponse) GetRetryAfterMs() uint64 {
	if x != nil && x.RetryAfterMs != nil {
		return *x.RetryAfterMs
	}
	return 0
}

type FilterWriteCursor struct {
	state protoimpl.MessageState `protogen:"open.v1"`
	// The last key written; the next call starts after it.
	After *DocumentId `protobuf:"bytes,1,opt,name=after,proto3" json:"after,omitempty"`
	// The snapshot the first call pinned, so the next call evaluates the same
	// filter against the same state.
	ManifestVersion uint64 `protobuf:"varint,2,opt,name=manifest_version,json=manifestVersion,proto3" json:"manifest_version,omitempty"`
	Token           string `protobuf:"bytes,3,opt,name=token,proto3" json:"token,omitempty"`
	unknownFields   protoimpl.UnknownFields
	sizeCache       protoimpl.SizeCache
}

func (x *FilterWriteCursor) Reset() {
	*x = FilterWriteCursor{}
	mi := &file_loams_collection_v1_document_proto_msgTypes[17]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *FilterWriteCursor) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*FilterWriteCursor) ProtoMessage() {}

func (x *FilterWriteCursor) ProtoReflect() protoreflect.Message {
	mi := &file_loams_collection_v1_document_proto_msgTypes[17]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use FilterWriteCursor.ProtoReflect.Descriptor instead.
func (*FilterWriteCursor) Descriptor() ([]byte, []int) {
	return file_loams_collection_v1_document_proto_rawDescGZIP(), []int{17}
}

func (x *FilterWriteCursor) GetAfter() *DocumentId {
	if x != nil {
		return x.After
	}
	return nil
}

func (x *FilterWriteCursor) GetManifestVersion() uint64 {
	if x != nil {
		return x.ManifestVersion
	}
	return 0
}

func (x *FilterWriteCursor) GetToken() string {
	if x != nil {
		return x.Token
	}
	return ""
}

// One document, as a get or a scroll answers it.
type Document struct {
	state protoimpl.MessageState `protogen:"open.v1"`
	// Absent only on the entry of an id that is not there.
	Id *DocumentId `protobuf:"bytes,1,opt,name=id,proto3" json:"id,omitempty"`
	// Absent when the projection leaves the source out, rather than an empty
	// object: "no source" and "an empty source" are different answers.
	Source *structpb.Struct `protobuf:"bytes,2,opt,name=source,proto3" json:"source,omitempty"`
	// Dense vectors by name, `map<string, google.protobuf.Value>` for the same
	// reason a `WriteDocument`'s are.
	Vectors       map[string]*structpb.Value  `protobuf:"bytes,3,rep,name=vectors,proto3" json:"vectors,omitempty" protobuf_key:"bytes,1,opt,name=key" protobuf_val:"bytes,2,opt,name=value"`
	SparseVectors map[string]*structpb.Struct `protobuf:"bytes,4,rep,name=sparse_vectors,json=sparseVectors,proto3" json:"sparse_vectors,omitempty" protobuf_key:"bytes,1,opt,name=key" protobuf_val:"bytes,2,opt,name=value"`
	// The typed field values by name, as the REST route's JSON (see the header).
	Fields map[string]*structpb.Value `protobuf:"bytes,5,rep,name=fields,proto3" json:"fields,omitempty" protobuf_key:"bytes,1,opt,name=key" protobuf_val:"bytes,2,opt,name=value"`
	// Where the document's last write was appended. `optional` and always set
	// for the same reason as `OpPosition.seq_no`: the first write to a partition
	// is at 0, and "0" must not read as "no position".
	SeqNo         *uint64 `protobuf:"varint,6,opt,name=seq_no,json=seqNo,proto3,oneof" json:"seq_no,omitempty"`
	Partition     uint32  `protobuf:"varint,7,opt,name=partition,proto3" json:"partition,omitempty"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *Document) Reset() {
	*x = Document{}
	mi := &file_loams_collection_v1_document_proto_msgTypes[18]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *Document) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*Document) ProtoMessage() {}

func (x *Document) ProtoReflect() protoreflect.Message {
	mi := &file_loams_collection_v1_document_proto_msgTypes[18]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use Document.ProtoReflect.Descriptor instead.
func (*Document) Descriptor() ([]byte, []int) {
	return file_loams_collection_v1_document_proto_rawDescGZIP(), []int{18}
}

func (x *Document) GetId() *DocumentId {
	if x != nil {
		return x.Id
	}
	return nil
}

func (x *Document) GetSource() *structpb.Struct {
	if x != nil {
		return x.Source
	}
	return nil
}

func (x *Document) GetVectors() map[string]*structpb.Value {
	if x != nil {
		return x.Vectors
	}
	return nil
}

func (x *Document) GetSparseVectors() map[string]*structpb.Struct {
	if x != nil {
		return x.SparseVectors
	}
	return nil
}

func (x *Document) GetFields() map[string]*structpb.Value {
	if x != nil {
		return x.Fields
	}
	return nil
}

func (x *Document) GetSeqNo() uint64 {
	if x != nil && x.SeqNo != nil {
		return *x.SeqNo
	}
	return 0
}

func (x *Document) GetPartition() uint32 {
	if x != nil {
		return x.Partition
	}
	return 0
}

// How much of a read's freshness the caller asks for (rule 1).
type Consistency struct {
	state protoimpl.MessageState `protogen:"open.v1"`
	// Absent is `FRESHNESS_STRONG`: every write acknowledged before the read
	// began is visible. `FRESHNESS_EVENTUAL` is whatever the tail holds.
	Freshness Freshness `protobuf:"varint,1,opt,name=freshness,proto3,enum=loams.collection.v1.Freshness" json:"freshness,omitempty"`
	// At least the writes this token names: the read **waits** for them and then
	// reads exactly the state up to them, which is what makes this a wait and
	// not a hint. A token this server cannot read is `invalid_argument`.
	AtLeast string `protobuf:"bytes,2,opt,name=at_least,json=atLeast,proto3" json:"at_least,omitempty"`
	// Exactly one retained manifest plus the token's writes after it, which is
	// what a filter write pins. A pinned read is not a free one.
	Pin           *Pin `protobuf:"bytes,3,opt,name=pin,proto3" json:"pin,omitempty"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *Consistency) Reset() {
	*x = Consistency{}
	mi := &file_loams_collection_v1_document_proto_msgTypes[19]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *Consistency) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*Consistency) ProtoMessage() {}

func (x *Consistency) ProtoReflect() protoreflect.Message {
	mi := &file_loams_collection_v1_document_proto_msgTypes[19]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use Consistency.ProtoReflect.Descriptor instead.
func (*Consistency) Descriptor() ([]byte, []int) {
	return file_loams_collection_v1_document_proto_rawDescGZIP(), []int{19}
}

func (x *Consistency) GetFreshness() Freshness {
	if x != nil {
		return x.Freshness
	}
	return Freshness_FRESHNESS_UNSPECIFIED
}

func (x *Consistency) GetAtLeast() string {
	if x != nil {
		return x.AtLeast
	}
	return ""
}

func (x *Consistency) GetPin() *Pin {
	if x != nil {
		return x.Pin
	}
	return nil
}

// One pinned state of a collection: a manifest version and the token that
// covers it. `Consistency.pin` names the state to read and
// `FilterWriteResponse.pin` reports the one a filter write read.
type Pin struct {
	state           protoimpl.MessageState `protogen:"open.v1"`
	ManifestVersion uint64                 `protobuf:"varint,1,opt,name=manifest_version,json=manifestVersion,proto3" json:"manifest_version,omitempty"`
	// A `consistency.at_least` that reads this state again.
	Token         string `protobuf:"bytes,2,opt,name=token,proto3" json:"token,omitempty"`
	unknownFields protoimpl.UnknownFields
	sizeCache     protoimpl.SizeCache
}

func (x *Pin) Reset() {
	*x = Pin{}
	mi := &file_loams_collection_v1_document_proto_msgTypes[20]
	ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
	ms.StoreMessageInfo(mi)
}

func (x *Pin) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*Pin) ProtoMessage() {}

func (x *Pin) ProtoReflect() protoreflect.Message {
	mi := &file_loams_collection_v1_document_proto_msgTypes[20]
	if x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use Pin.ProtoReflect.Descriptor instead.
func (*Pin) Descriptor() ([]byte, []int) {
	return file_loams_collection_v1_document_proto_rawDescGZIP(), []int{20}
}

func (x *Pin) GetManifestVersion() uint64 {
	if x != nil {
		return x.ManifestVersion
	}
	return 0
}

func (x *Pin) GetToken() string {
	if x != nil {
		return x.Token
	}
	return ""
}

var File_loams_collection_v1_document_proto protoreflect.FileDescriptor

const file_loams_collection_v1_document_proto_rawDesc = "" +
	"\n" +
	"\"loams/collection/v1/document.proto\x12\x13loams.collection.v1\x1a\x1cgoogle/protobuf/struct.proto\x1a\x1eloams/options/v1/options.proto\"\xd9\x01\n" +
	"\x15WriteDocumentsRequest\x12\x1c\n" +
	"\tnamespace\x18\x01 \x01(\tR\tnamespace\x12\x1e\n" +
	"\n" +
	"collection\x18\x02 \x01(\tR\n" +
	"collection\x12.\n" +
	"\x03ops\x18\x03 \x03(\v2\x1c.loams.collection.v1.WriteOpR\x03ops\x12)\n" +
	"\x10report_existence\x18\x04 \x01(\bR\x0freportExistence\x12'\n" +
	"\x0fidempotency_key\x18\x05 \x01(\tR\x0eidempotencyKey\"\xb0\x02\n" +
	"\x16WriteDocumentsResponse\x12\x14\n" +
	"\x05token\x18\x01 \x01(\tR\x05token\x127\n" +
	"\aresults\x18\x02 \x03(\x0e2\x1d.loams.collection.v1.OpResultR\aresults\x12=\n" +
	"\tpositions\x18\x03 \x03(\v2\x1f.loams.collection.v1.OpPositionR\tpositions\x120\n" +
	"\x11unapplied_records\x18\x04 \x01(\x04H\x00R\x10unappliedRecords\x88\x01\x01\x12,\n" +
	"\x0funapplied_bytes\x18\x05 \x01(\x04H\x01R\x0eunappliedBytes\x88\x01\x01B\x14\n" +
	"\x12_unapplied_recordsB\x12\n" +
	"\x10_unapplied_bytes\"\xc0\x01\n" +
	"\aWriteOp\x12<\n" +
	"\x06upsert\x18\x01 \x01(\v2\".loams.collection.v1.WriteDocumentH\x00R\x06upsert\x12=\n" +
	"\x06delete\x18\x02 \x01(\v2#.loams.collection.v1.DeleteDocumentH\x00R\x06delete\x122\n" +
	"\x05patch\x18\x03 \x01(\v2\x1a.loams.collection.v1.PatchH\x00R\x05patchB\x04\n" +
	"\x02op\"\xc9\x03\n" +
	"\rWriteDocument\x12/\n" +
	"\x02id\x18\x01 \x01(\v2\x1f.loams.collection.v1.DocumentIdR\x02id\x12/\n" +
	"\x06source\x18\x02 \x01(\v2\x17.google.protobuf.StructR\x06source\x12I\n" +
	"\avectors\x18\x03 \x03(\v2/.loams.collection.v1.WriteDocument.VectorsEntryR\avectors\x12\\\n" +
	"\x0esparse_vectors\x18\x04 \x03(\v25.loams.collection.v1.WriteDocument.SparseVectorsEntryR\rsparseVectors\x1aR\n" +
	"\fVectorsEntry\x12\x10\n" +
	"\x03key\x18\x01 \x01(\tR\x03key\x12,\n" +
	"\x05value\x18\x02 \x01(\v2\x16.google.protobuf.ValueR\x05value:\x028\x01\x1aY\n" +
	"\x12SparseVectorsEntry\x12\x10\n" +
	"\x03key\x18\x01 \x01(\tR\x03key\x12-\n" +
	"\x05value\x18\x02 \x01(\v2\x17.google.protobuf.StructR\x05value:\x028\x01\"A\n" +
	"\x0eDeleteDocument\x12/\n" +
	"\x02id\x18\x01 \x01(\v2\x1f.loams.collection.v1.DocumentIdR\x02id\"\xa2\x04\n" +
	"\x05Patch\x12/\n" +
	"\x02id\x18\x01 \x01(\v2\x1f.loams.collection.v1.DocumentIdR\x02id\x12\x12\n" +
	"\x04mode\x18\x02 \x01(\tR\x04mode\x12/\n" +
	"\x06source\x18\x03 \x01(\v2\x17.google.protobuf.StructR\x06source\x12\x1f\n" +
	"\vdelete_keys\x18\x04 \x03(\tR\n" +
	"deleteKeys\x12A\n" +
	"\avectors\x18\x05 \x03(\v2'.loams.collection.v1.Patch.VectorsEntryR\avectors\x12T\n" +
	"\x0esparse_vectors\x18\x06 \x03(\v2-.loams.collection.v1.Patch.SparseVectorsEntryR\rsparseVectors\x12:\n" +
	"\x06upsert\x18\a \x01(\v2\".loams.collection.v1.WriteDocumentR\x06upsert\x1aR\n" +
	"\fVectorsEntry\x12\x10\n" +
	"\x03key\x18\x01 \x01(\tR\x03key\x12,\n" +
	"\x05value\x18\x02 \x01(\v2\x16.google.protobuf.ValueR\x05value:\x028\x01\x1aY\n" +
	"\x12SparseVectorsEntry\x12\x10\n" +
	"\x03key\x18\x01 \x01(\tR\x03key\x12-\n" +
	"\x05value\x18\x02 \x01(\v2\x17.google.protobuf.StructR\x05value:\x028\x01\"X\n" +
	"\n" +
	"DocumentId\x12\x14\n" +
	"\x04uint\x18\x01 \x01(\x04H\x00R\x04uint\x12\x18\n" +
	"\x06string\x18\x02 \x01(\tH\x00R\x06string\x12\x14\n" +
	"\x04uuid\x18\x03 \x01(\tH\x00R\x04uuidB\x04\n" +
	"\x02id\"Q\n" +
	"\n" +
	"OpPosition\x12\x1c\n" +
	"\tpartition\x18\x01 \x01(\rR\tpartition\x12\x1a\n" +
	"\x06seq_no\x18\x02 \x01(\x04H\x00R\x05seqNo\x88\x01\x01B\t\n" +
	"\a_seq_no\"\xfb\x01\n" +
	"\x13GetDocumentsRequest\x12\x1c\n" +
	"\tnamespace\x18\x01 \x01(\tR\tnamespace\x12\x1e\n" +
	"\n" +
	"collection\x18\x02 \x01(\tR\n" +
	"collection\x121\n" +
	"\x03ids\x18\x03 \x03(\v2\x1f.loams.collection.v1.DocumentIdR\x03ids\x12/\n" +
	"\x06select\x18\x04 \x01(\v2\x17.google.protobuf.StructR\x06select\x12B\n" +
	"\vconsistency\x18\x05 \x01(\v2 .loams.collection.v1.ConsistencyR\vconsistency\"r\n" +
	"\x14GetDocumentsResponse\x12;\n" +
	"\tdocuments\x18\x01 \x03(\v2\x1d.loams.collection.v1.DocumentR\tdocuments\x12\x1d\n" +
	"\n" +
	"read_token\x18\x02 \x01(\tR\treadToken\"\xc6\x03\n" +
	"\x16ScrollDocumentsRequest\x12\x1c\n" +
	"\tnamespace\x18\x01 \x01(\tR\tnamespace\x12\x1e\n" +
	"\n" +
	"collection\x18\x02 \x01(\tR\n" +
	"collection\x12.\n" +
	"\x06filter\x18\x03 \x01(\v2\x16.google.protobuf.ValueR\x06filter\x125\n" +
	"\x05after\x18\x04 \x01(\v2\x1f.loams.collection.v1.DocumentIdR\x05after\x12\x19\n" +
	"\x05limit\x18\x05 \x01(\rH\x00R\x05limit\x88\x01\x01\x12/\n" +
	"\x06select\x18\x06 \x01(\v2\x17.google.protobuf.StructR\x06select\x12B\n" +
	"\vconsistency\x18\a \x01(\v2 .loams.collection.v1.ConsistencyR\vconsistency\x12>\n" +
	"\n" +
	"page_token\x18\b \x01(\v2\x1f.loams.collection.v1.DocumentIdR\tpageToken\x12-\n" +
	"\x05where\x18\t \x01(\v2\x17.google.protobuf.StructR\x05whereB\b\n" +
	"\x06_limit\"\xaa\x01\n" +
	"\x17ScrollDocumentsResponse\x12;\n" +
	"\tdocuments\x18\x01 \x03(\v2\x1d.loams.collection.v1.DocumentR\tdocuments\x123\n" +
	"\x04next\x18\x02 \x01(\v2\x1f.loams.collection.v1.DocumentIdR\x04next\x12\x1d\n" +
	"\n" +
	"read_token\x18\x03 \x01(\tR\treadToken\"\xf8\x01\n" +
	"\x15CountDocumentsRequest\x12\x1c\n" +
	"\tnamespace\x18\x01 \x01(\tR\tnamespace\x12\x1e\n" +
	"\n" +
	"collection\x18\x02 \x01(\tR\n" +
	"collection\x12.\n" +
	"\x06filter\x18\x03 \x01(\v2\x16.google.protobuf.ValueR\x06filter\x12B\n" +
	"\vconsistency\x18\x04 \x01(\v2 .loams.collection.v1.ConsistencyR\vconsistency\x12-\n" +
	"\x05where\x18\x05 \x01(\v2\x17.google.protobuf.StructR\x05where\"\\\n" +
	"\x16CountDocumentsResponse\x12\x19\n" +
	"\x05count\x18\x01 \x01(\x04H\x00R\x05count\x88\x01\x01\x12\x1d\n" +
	"\n" +
	"read_token\x18\x02 \x01(\tR\treadTokenB\b\n" +
	"\x06_count\"\x8a\x03\n" +
	"\x15DeleteByFilterRequest\x12\x1c\n" +
	"\tnamespace\x18\x01 \x01(\tR\tnamespace\x12\x1e\n" +
	"\n" +
	"collection\x18\x02 \x01(\tR\n" +
	"collection\x12.\n" +
	"\x06filter\x18\x03 \x01(\v2\x16.google.protobuf.ValueR\x06filter\x12\x1e\n" +
	"\bmax_rows\x18\x04 \x01(\x04H\x00R\amaxRows\x88\x01\x01\x12#\n" +
	"\rallow_partial\x18\x05 \x01(\bR\fallowPartial\x12>\n" +
	"\x06cursor\x18\x06 \x01(\v2&.loams.collection.v1.FilterWriteCursorR\x06cursor\x12B\n" +
	"\vconsistency\x18\a \x01(\v2 .loams.collection.v1.ConsistencyR\vconsistency\x12-\n" +
	"\x05where\x18\b \x01(\v2\x17.google.protobuf.StructR\x05whereB\v\n" +
	"\t_max_rows\"\xbb\x03\n" +
	"\x14PatchByFilterRequest\x12\x1c\n" +
	"\tnamespace\x18\x01 \x01(\tR\tnamespace\x12\x1e\n" +
	"\n" +
	"collection\x18\x02 \x01(\tR\n" +
	"collection\x12.\n" +
	"\x06filter\x18\x03 \x01(\v2\x16.google.protobuf.ValueR\x06filter\x120\n" +
	"\x05patch\x18\x04 \x01(\v2\x1a.loams.collection.v1.PatchR\x05patch\x12\x1e\n" +
	"\bmax_rows\x18\x05 \x01(\x04H\x00R\amaxRows\x88\x01\x01\x12#\n" +
	"\rallow_partial\x18\x06 \x01(\bR\fallowPartial\x12>\n" +
	"\x06cursor\x18\a \x01(\v2&.loams.collection.v1.FilterWriteCursorR\x06cursor\x12B\n" +
	"\vconsistency\x18\b \x01(\v2 .loams.collection.v1.ConsistencyR\vconsistency\x12-\n" +
	"\x05where\x18\t \x01(\v2\x17.google.protobuf.StructR\x05whereB\v\n" +
	"\t_max_rows\"\xab\x03\n" +
	"\x13FilterWriteResponse\x12\x1d\n" +
	"\amatched\x18\x01 \x01(\x04H\x00R\amatched\x88\x01\x01\x12\x1f\n" +
	"\baffected\x18\x02 \x01(\x04H\x01R\baffected\x88\x01\x01\x12\x1d\n" +
	"\awritten\x18\x03 \x01(\x04H\x02R\awritten\x88\x01\x01\x12\x1d\n" +
	"\abatches\x18\x04 \x01(\x04H\x03R\abatches\x88\x01\x01\x12%\n" +
	"\x0erows_remaining\x18\x05 \x01(\bR\rrowsRemaining\x12>\n" +
	"\x06cursor\x18\x06 \x01(\v2&.loams.collection.v1.FilterWriteCursorR\x06cursor\x12\x14\n" +
	"\x05token\x18\a \x01(\tR\x05token\x12*\n" +
	"\x03pin\x18\b \x01(\v2\x18.loams.collection.v1.PinR\x03pin\x12)\n" +
	"\x0eretry_after_ms\x18\t \x01(\x04H\x04R\fretryAfterMs\x88\x01\x01B\n" +
	"\n" +
	"\b_matchedB\v\n" +
	"\t_affectedB\n" +
	"\n" +
	"\b_writtenB\n" +
	"\n" +
	"\b_batchesB\x11\n" +
	"\x0f_retry_after_ms\"\x8b\x01\n" +
	"\x11FilterWriteCursor\x125\n" +
	"\x05after\x18\x01 \x01(\v2\x1f.loams.collection.v1.DocumentIdR\x05after\x12)\n" +
	"\x10manifest_version\x18\x02 \x01(\x04R\x0fmanifestVersion\x12\x14\n" +
	"\x05token\x18\x03 \x01(\tR\x05token\"\x95\x05\n" +
	"\bDocument\x12/\n" +
	"\x02id\x18\x01 \x01(\v2\x1f.loams.collection.v1.DocumentIdR\x02id\x12/\n" +
	"\x06source\x18\x02 \x01(\v2\x17.google.protobuf.StructR\x06source\x12D\n" +
	"\avectors\x18\x03 \x03(\v2*.loams.collection.v1.Document.VectorsEntryR\avectors\x12W\n" +
	"\x0esparse_vectors\x18\x04 \x03(\v20.loams.collection.v1.Document.SparseVectorsEntryR\rsparseVectors\x12A\n" +
	"\x06fields\x18\x05 \x03(\v2).loams.collection.v1.Document.FieldsEntryR\x06fields\x12\x1a\n" +
	"\x06seq_no\x18\x06 \x01(\x04H\x00R\x05seqNo\x88\x01\x01\x12\x1c\n" +
	"\tpartition\x18\a \x01(\rR\tpartition\x1aR\n" +
	"\fVectorsEntry\x12\x10\n" +
	"\x03key\x18\x01 \x01(\tR\x03key\x12,\n" +
	"\x05value\x18\x02 \x01(\v2\x16.google.protobuf.ValueR\x05value:\x028\x01\x1aY\n" +
	"\x12SparseVectorsEntry\x12\x10\n" +
	"\x03key\x18\x01 \x01(\tR\x03key\x12-\n" +
	"\x05value\x18\x02 \x01(\v2\x17.google.protobuf.StructR\x05value:\x028\x01\x1aQ\n" +
	"\vFieldsEntry\x12\x10\n" +
	"\x03key\x18\x01 \x01(\tR\x03key\x12,\n" +
	"\x05value\x18\x02 \x01(\v2\x16.google.protobuf.ValueR\x05value:\x028\x01B\t\n" +
	"\a_seq_no\"\x92\x01\n" +
	"\vConsistency\x12<\n" +
	"\tfreshness\x18\x01 \x01(\x0e2\x1e.loams.collection.v1.FreshnessR\tfreshness\x12\x19\n" +
	"\bat_least\x18\x02 \x01(\tR\aatLeast\x12*\n" +
	"\x03pin\x18\x03 \x01(\v2\x18.loams.collection.v1.PinR\x03pin\"F\n" +
	"\x03Pin\x12)\n" +
	"\x10manifest_version\x18\x01 \x01(\x04R\x0fmanifestVersion\x12\x14\n" +
	"\x05token\x18\x02 \x01(\tR\x05token*\xaf\x01\n" +
	"\bOpResult\x12\x19\n" +
	"\x15OP_RESULT_UNSPECIFIED\x10\x00\x12\x15\n" +
	"\x11OP_RESULT_CREATED\x10\x01\x12\x15\n" +
	"\x11OP_RESULT_UPDATED\x10\x02\x12\x15\n" +
	"\x11OP_RESULT_DELETED\x10\x03\x12\x17\n" +
	"\x13OP_RESULT_NOT_FOUND\x10\x04\x12\x12\n" +
	"\x0eOP_RESULT_NOOP\x10\x05\x12\x16\n" +
	"\x12OP_RESULT_ACCEPTED\x10\x06*T\n" +
	"\tFreshness\x12\x19\n" +
	"\x15FRESHNESS_UNSPECIFIED\x10\x00\x12\x14\n" +
	"\x10FRESHNESS_STRONG\x10\x01\x12\x16\n" +
	"\x12FRESHNESS_EVENTUAL\x10\x022\xd1\x06\n" +
	"\x0fDocumentService\x12v\n" +
	"\x0eWriteDocuments\x12*.loams.collection.v1.WriteDocumentsRequest\x1a+.loams.collection.v1.WriteDocumentsResponse\"\v\x92\xb5\x18\a\x12\x05write\x12q\n" +
	"\fGetDocuments\x12(.loams.collection.v1.GetDocumentsRequest\x1a).loams.collection.v1.GetDocumentsResponse\"\f\x92\xb5\x18\x05\x12\x03get\x90\x02\x01\x12}\n" +
	"\x0fScrollDocuments\x12+.loams.collection.v1.ScrollDocumentsRequest\x1a,.loams.collection.v1.ScrollDocumentsResponse\"\x0f\x92\xb5\x18\b\x12\x06scroll\x90\x02\x01\x12y\n" +
	"\x0eCountDocuments\x12*.loams.collection.v1.CountDocumentsRequest\x1a+.loams.collection.v1.CountDocumentsResponse\"\x0e\x92\xb5\x18\a\x12\x05count\x90\x02\x01\x12~\n" +
	"\x0eDeleteByFilter\x12*.loams.collection.v1.DeleteByFilterRequest\x1a(.loams.collection.v1.FilterWriteResponse\"\x16\x92\xb5\x18\x12\x12\x10delete_by_filter\x12{\n" +
	"\rPatchByFilter\x12).loams.collection.v1.PatchByFilterRequest\x1a(.loams.collection.v1.FilterWriteResponse\"\x15\x92\xb5\x18\x11\x12\x0fpatch_by_filter\x1a\\\x8a\xb5\x18X\n" +
	"\tdocuments\x12KDocuments: write, get, scroll, count, delete by filter and patch by filter.b\x06proto3"

var (
	file_loams_collection_v1_document_proto_rawDescOnce sync.Once
	file_loams_collection_v1_document_proto_rawDescData []byte
)

func file_loams_collection_v1_document_proto_rawDescGZIP() []byte {
	file_loams_collection_v1_document_proto_rawDescOnce.Do(func() {
		file_loams_collection_v1_document_proto_rawDescData = protoimpl.X.CompressGZIP(unsafe.Slice(unsafe.StringData(file_loams_collection_v1_document_proto_rawDesc), len(file_loams_collection_v1_document_proto_rawDesc)))
	})
	return file_loams_collection_v1_document_proto_rawDescData
}

var file_loams_collection_v1_document_proto_enumTypes = make([]protoimpl.EnumInfo, 2)
var file_loams_collection_v1_document_proto_msgTypes = make([]protoimpl.MessageInfo, 28)
var file_loams_collection_v1_document_proto_goTypes = []any{
	(OpResult)(0),                   // 0: loams.collection.v1.OpResult
	(Freshness)(0),                  // 1: loams.collection.v1.Freshness
	(*WriteDocumentsRequest)(nil),   // 2: loams.collection.v1.WriteDocumentsRequest
	(*WriteDocumentsResponse)(nil),  // 3: loams.collection.v1.WriteDocumentsResponse
	(*WriteOp)(nil),                 // 4: loams.collection.v1.WriteOp
	(*WriteDocument)(nil),           // 5: loams.collection.v1.WriteDocument
	(*DeleteDocument)(nil),          // 6: loams.collection.v1.DeleteDocument
	(*Patch)(nil),                   // 7: loams.collection.v1.Patch
	(*DocumentId)(nil),              // 8: loams.collection.v1.DocumentId
	(*OpPosition)(nil),              // 9: loams.collection.v1.OpPosition
	(*GetDocumentsRequest)(nil),     // 10: loams.collection.v1.GetDocumentsRequest
	(*GetDocumentsResponse)(nil),    // 11: loams.collection.v1.GetDocumentsResponse
	(*ScrollDocumentsRequest)(nil),  // 12: loams.collection.v1.ScrollDocumentsRequest
	(*ScrollDocumentsResponse)(nil), // 13: loams.collection.v1.ScrollDocumentsResponse
	(*CountDocumentsRequest)(nil),   // 14: loams.collection.v1.CountDocumentsRequest
	(*CountDocumentsResponse)(nil),  // 15: loams.collection.v1.CountDocumentsResponse
	(*DeleteByFilterRequest)(nil),   // 16: loams.collection.v1.DeleteByFilterRequest
	(*PatchByFilterRequest)(nil),    // 17: loams.collection.v1.PatchByFilterRequest
	(*FilterWriteResponse)(nil),     // 18: loams.collection.v1.FilterWriteResponse
	(*FilterWriteCursor)(nil),       // 19: loams.collection.v1.FilterWriteCursor
	(*Document)(nil),                // 20: loams.collection.v1.Document
	(*Consistency)(nil),             // 21: loams.collection.v1.Consistency
	(*Pin)(nil),                     // 22: loams.collection.v1.Pin
	nil,                             // 23: loams.collection.v1.WriteDocument.VectorsEntry
	nil,                             // 24: loams.collection.v1.WriteDocument.SparseVectorsEntry
	nil,                             // 25: loams.collection.v1.Patch.VectorsEntry
	nil,                             // 26: loams.collection.v1.Patch.SparseVectorsEntry
	nil,                             // 27: loams.collection.v1.Document.VectorsEntry
	nil,                             // 28: loams.collection.v1.Document.SparseVectorsEntry
	nil,                             // 29: loams.collection.v1.Document.FieldsEntry
	(*structpb.Struct)(nil),         // 30: google.protobuf.Struct
	(*structpb.Value)(nil),          // 31: google.protobuf.Value
}
var file_loams_collection_v1_document_proto_depIdxs = []int32{
	4,  // 0: loams.collection.v1.WriteDocumentsRequest.ops:type_name -> loams.collection.v1.WriteOp
	0,  // 1: loams.collection.v1.WriteDocumentsResponse.results:type_name -> loams.collection.v1.OpResult
	9,  // 2: loams.collection.v1.WriteDocumentsResponse.positions:type_name -> loams.collection.v1.OpPosition
	5,  // 3: loams.collection.v1.WriteOp.upsert:type_name -> loams.collection.v1.WriteDocument
	6,  // 4: loams.collection.v1.WriteOp.delete:type_name -> loams.collection.v1.DeleteDocument
	7,  // 5: loams.collection.v1.WriteOp.patch:type_name -> loams.collection.v1.Patch
	8,  // 6: loams.collection.v1.WriteDocument.id:type_name -> loams.collection.v1.DocumentId
	30, // 7: loams.collection.v1.WriteDocument.source:type_name -> google.protobuf.Struct
	23, // 8: loams.collection.v1.WriteDocument.vectors:type_name -> loams.collection.v1.WriteDocument.VectorsEntry
	24, // 9: loams.collection.v1.WriteDocument.sparse_vectors:type_name -> loams.collection.v1.WriteDocument.SparseVectorsEntry
	8,  // 10: loams.collection.v1.DeleteDocument.id:type_name -> loams.collection.v1.DocumentId
	8,  // 11: loams.collection.v1.Patch.id:type_name -> loams.collection.v1.DocumentId
	30, // 12: loams.collection.v1.Patch.source:type_name -> google.protobuf.Struct
	25, // 13: loams.collection.v1.Patch.vectors:type_name -> loams.collection.v1.Patch.VectorsEntry
	26, // 14: loams.collection.v1.Patch.sparse_vectors:type_name -> loams.collection.v1.Patch.SparseVectorsEntry
	5,  // 15: loams.collection.v1.Patch.upsert:type_name -> loams.collection.v1.WriteDocument
	8,  // 16: loams.collection.v1.GetDocumentsRequest.ids:type_name -> loams.collection.v1.DocumentId
	30, // 17: loams.collection.v1.GetDocumentsRequest.select:type_name -> google.protobuf.Struct
	21, // 18: loams.collection.v1.GetDocumentsRequest.consistency:type_name -> loams.collection.v1.Consistency
	20, // 19: loams.collection.v1.GetDocumentsResponse.documents:type_name -> loams.collection.v1.Document
	31, // 20: loams.collection.v1.ScrollDocumentsRequest.filter:type_name -> google.protobuf.Value
	8,  // 21: loams.collection.v1.ScrollDocumentsRequest.after:type_name -> loams.collection.v1.DocumentId
	30, // 22: loams.collection.v1.ScrollDocumentsRequest.select:type_name -> google.protobuf.Struct
	21, // 23: loams.collection.v1.ScrollDocumentsRequest.consistency:type_name -> loams.collection.v1.Consistency
	8,  // 24: loams.collection.v1.ScrollDocumentsRequest.page_token:type_name -> loams.collection.v1.DocumentId
	30, // 25: loams.collection.v1.ScrollDocumentsRequest.where:type_name -> google.protobuf.Struct
	20, // 26: loams.collection.v1.ScrollDocumentsResponse.documents:type_name -> loams.collection.v1.Document
	8,  // 27: loams.collection.v1.ScrollDocumentsResponse.next:type_name -> loams.collection.v1.DocumentId
	31, // 28: loams.collection.v1.CountDocumentsRequest.filter:type_name -> google.protobuf.Value
	21, // 29: loams.collection.v1.CountDocumentsRequest.consistency:type_name -> loams.collection.v1.Consistency
	30, // 30: loams.collection.v1.CountDocumentsRequest.where:type_name -> google.protobuf.Struct
	31, // 31: loams.collection.v1.DeleteByFilterRequest.filter:type_name -> google.protobuf.Value
	19, // 32: loams.collection.v1.DeleteByFilterRequest.cursor:type_name -> loams.collection.v1.FilterWriteCursor
	21, // 33: loams.collection.v1.DeleteByFilterRequest.consistency:type_name -> loams.collection.v1.Consistency
	30, // 34: loams.collection.v1.DeleteByFilterRequest.where:type_name -> google.protobuf.Struct
	31, // 35: loams.collection.v1.PatchByFilterRequest.filter:type_name -> google.protobuf.Value
	7,  // 36: loams.collection.v1.PatchByFilterRequest.patch:type_name -> loams.collection.v1.Patch
	19, // 37: loams.collection.v1.PatchByFilterRequest.cursor:type_name -> loams.collection.v1.FilterWriteCursor
	21, // 38: loams.collection.v1.PatchByFilterRequest.consistency:type_name -> loams.collection.v1.Consistency
	30, // 39: loams.collection.v1.PatchByFilterRequest.where:type_name -> google.protobuf.Struct
	19, // 40: loams.collection.v1.FilterWriteResponse.cursor:type_name -> loams.collection.v1.FilterWriteCursor
	22, // 41: loams.collection.v1.FilterWriteResponse.pin:type_name -> loams.collection.v1.Pin
	8,  // 42: loams.collection.v1.FilterWriteCursor.after:type_name -> loams.collection.v1.DocumentId
	8,  // 43: loams.collection.v1.Document.id:type_name -> loams.collection.v1.DocumentId
	30, // 44: loams.collection.v1.Document.source:type_name -> google.protobuf.Struct
	27, // 45: loams.collection.v1.Document.vectors:type_name -> loams.collection.v1.Document.VectorsEntry
	28, // 46: loams.collection.v1.Document.sparse_vectors:type_name -> loams.collection.v1.Document.SparseVectorsEntry
	29, // 47: loams.collection.v1.Document.fields:type_name -> loams.collection.v1.Document.FieldsEntry
	1,  // 48: loams.collection.v1.Consistency.freshness:type_name -> loams.collection.v1.Freshness
	22, // 49: loams.collection.v1.Consistency.pin:type_name -> loams.collection.v1.Pin
	31, // 50: loams.collection.v1.WriteDocument.VectorsEntry.value:type_name -> google.protobuf.Value
	30, // 51: loams.collection.v1.WriteDocument.SparseVectorsEntry.value:type_name -> google.protobuf.Struct
	31, // 52: loams.collection.v1.Patch.VectorsEntry.value:type_name -> google.protobuf.Value
	30, // 53: loams.collection.v1.Patch.SparseVectorsEntry.value:type_name -> google.protobuf.Struct
	31, // 54: loams.collection.v1.Document.VectorsEntry.value:type_name -> google.protobuf.Value
	30, // 55: loams.collection.v1.Document.SparseVectorsEntry.value:type_name -> google.protobuf.Struct
	31, // 56: loams.collection.v1.Document.FieldsEntry.value:type_name -> google.protobuf.Value
	2,  // 57: loams.collection.v1.DocumentService.WriteDocuments:input_type -> loams.collection.v1.WriteDocumentsRequest
	10, // 58: loams.collection.v1.DocumentService.GetDocuments:input_type -> loams.collection.v1.GetDocumentsRequest
	12, // 59: loams.collection.v1.DocumentService.ScrollDocuments:input_type -> loams.collection.v1.ScrollDocumentsRequest
	14, // 60: loams.collection.v1.DocumentService.CountDocuments:input_type -> loams.collection.v1.CountDocumentsRequest
	16, // 61: loams.collection.v1.DocumentService.DeleteByFilter:input_type -> loams.collection.v1.DeleteByFilterRequest
	17, // 62: loams.collection.v1.DocumentService.PatchByFilter:input_type -> loams.collection.v1.PatchByFilterRequest
	3,  // 63: loams.collection.v1.DocumentService.WriteDocuments:output_type -> loams.collection.v1.WriteDocumentsResponse
	11, // 64: loams.collection.v1.DocumentService.GetDocuments:output_type -> loams.collection.v1.GetDocumentsResponse
	13, // 65: loams.collection.v1.DocumentService.ScrollDocuments:output_type -> loams.collection.v1.ScrollDocumentsResponse
	15, // 66: loams.collection.v1.DocumentService.CountDocuments:output_type -> loams.collection.v1.CountDocumentsResponse
	18, // 67: loams.collection.v1.DocumentService.DeleteByFilter:output_type -> loams.collection.v1.FilterWriteResponse
	18, // 68: loams.collection.v1.DocumentService.PatchByFilter:output_type -> loams.collection.v1.FilterWriteResponse
	63, // [63:69] is the sub-list for method output_type
	57, // [57:63] is the sub-list for method input_type
	57, // [57:57] is the sub-list for extension type_name
	57, // [57:57] is the sub-list for extension extendee
	0,  // [0:57] is the sub-list for field type_name
}

func init() { file_loams_collection_v1_document_proto_init() }
func file_loams_collection_v1_document_proto_init() {
	if File_loams_collection_v1_document_proto != nil {
		return
	}
	file_loams_collection_v1_document_proto_msgTypes[1].OneofWrappers = []any{}
	file_loams_collection_v1_document_proto_msgTypes[2].OneofWrappers = []any{
		(*WriteOp_Upsert)(nil),
		(*WriteOp_Delete)(nil),
		(*WriteOp_Patch)(nil),
	}
	file_loams_collection_v1_document_proto_msgTypes[6].OneofWrappers = []any{
		(*DocumentId_Uint)(nil),
		(*DocumentId_String_)(nil),
		(*DocumentId_Uuid)(nil),
	}
	file_loams_collection_v1_document_proto_msgTypes[7].OneofWrappers = []any{}
	file_loams_collection_v1_document_proto_msgTypes[10].OneofWrappers = []any{}
	file_loams_collection_v1_document_proto_msgTypes[13].OneofWrappers = []any{}
	file_loams_collection_v1_document_proto_msgTypes[14].OneofWrappers = []any{}
	file_loams_collection_v1_document_proto_msgTypes[15].OneofWrappers = []any{}
	file_loams_collection_v1_document_proto_msgTypes[16].OneofWrappers = []any{}
	file_loams_collection_v1_document_proto_msgTypes[18].OneofWrappers = []any{}
	type x struct{}
	out := protoimpl.TypeBuilder{
		File: protoimpl.DescBuilder{
			GoPackagePath: reflect.TypeOf(x{}).PkgPath(),
			RawDescriptor: unsafe.Slice(unsafe.StringData(file_loams_collection_v1_document_proto_rawDesc), len(file_loams_collection_v1_document_proto_rawDesc)),
			NumEnums:      2,
			NumMessages:   28,
			NumExtensions: 0,
			NumServices:   1,
		},
		GoTypes:           file_loams_collection_v1_document_proto_goTypes,
		DependencyIndexes: file_loams_collection_v1_document_proto_depIdxs,
		EnumInfos:         file_loams_collection_v1_document_proto_enumTypes,
		MessageInfos:      file_loams_collection_v1_document_proto_msgTypes,
	}.Build()
	File_loams_collection_v1_document_proto = out.File
	file_loams_collection_v1_document_proto_goTypes = nil
	file_loams_collection_v1_document_proto_depIdxs = nil
}
