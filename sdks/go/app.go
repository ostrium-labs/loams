// The hand-written half of the app modules (SDKP §2.3–§2.5, §4).
//
// The facade gives one method per proto RPC. An app wants a few more, each a
// thin loop or shorthand over those:
//
//   - `DocumentCalls`: Upsert, Delete, Patch and WriteBatch over
//     `WriteDocuments`, and ScrollAll over `ScrollDocuments` (its cursor is a
//     `DocumentId`, not an AIP-158 string, so `Paginate` does not fit it);
//   - `SearchCalls`: Text, Hybrid and QueryAll over `QueryService/Search`.
//
// They take the generated calls as function values, so they work over the
// facade's module methods once P0a's annotations put `documents` and `search`
// on the client, and over a stub in a test today. Nothing here names an RPC
// path or a retry class; those stay the bindings'.

package loams

import (
	"context"
	"errors"
	"iter"

	connect "connectrpc.com/connect"
	"google.golang.org/protobuf/proto"
	"google.golang.org/protobuf/reflect/protoreflect"
	"google.golang.org/protobuf/types/known/structpb"

	collectionv1 "loams.dev/go/gen/loams/collection/v1"
)

// The generated calls the conveniences sit on.
type (
	WriteDocumentsFunc  func(context.Context, *collectionv1.WriteDocumentsRequest, ...CallOption) (*collectionv1.WriteDocumentsResponse, error)
	GetDocumentsFunc    func(context.Context, *collectionv1.GetDocumentsRequest, ...CallOption) (*collectionv1.GetDocumentsResponse, error)
	ScrollDocumentsFunc func(context.Context, *collectionv1.ScrollDocumentsRequest, ...CallOption) (*collectionv1.ScrollDocumentsResponse, error)
	CountDocumentsFunc  func(context.Context, *collectionv1.CountDocumentsRequest, ...CallOption) (*collectionv1.CountDocumentsResponse, error)
	SearchFunc          func(context.Context, *collectionv1.SearchRequest, ...CallOption) (*collectionv1.SearchResponse, error)
	GetCollectionFunc   func(context.Context, *collectionv1.GetCollectionRequest, ...CallOption) (*collectionv1.CollectionInfo, error)
	DropCollectionFunc  func(context.Context, *collectionv1.DropCollectionRequest, ...CallOption) (*collectionv1.DropCollectionResponse, error)
)

// Target is where a document call goes: the collection, and the namespace when
// it is not the client's default.
type Target struct {
	Namespace  string
	Collection string
}

// SparseVector is a sparse vector's indices and values.
type SparseVector struct {
	Indices []uint32  `json:"indices"`
	Values  []float32 `json:"values"`
}

// DocumentInput is one document to write.
type DocumentInput struct {
	ID            DocID
	Source        JSONObject
	Vectors       map[string][]float32
	SparseVectors map[string]SparseVector
}

// PatchChange is what a patch changes.
type PatchChange struct {
	// Set is the keys to merge into `_source`.
	Set JSONObject
	// Unset is the top-level keys to remove.
	Unset []string
	// Mode is merge_deep (the default), merge_top or replace.
	Mode string
	// Vectors to set; a nil slice removes one.
	Vectors map[string][]float32
	// Upsert is the document to write when there is none to patch.
	Upsert *DocumentInput
}

func documentJSON(doc DocumentInput) JSONObject {
	out := JSONObject{"id": doc.ID.documentIDJSON()}
	if doc.Source != nil {
		out["source"] = doc.Source
	}
	if doc.Vectors != nil {
		out["vectors"] = doc.Vectors
	}
	if doc.SparseVectors != nil {
		out["sparseVectors"] = doc.SparseVectors
	}
	return out
}

// UpsertOp is the `WriteOp` that writes doc.
func UpsertOp(doc DocumentInput) (*collectionv1.WriteOp, error) {
	return writeOp(JSONObject{"upsert": documentJSON(doc)})
}

// DeleteOp is the `WriteOp` that removes id.
func DeleteOp(id DocID) *collectionv1.WriteOp {
	return &collectionv1.WriteOp{Op: &collectionv1.WriteOp_Delete{Delete: &collectionv1.DeleteDocument{Id: id.Proto()}}}
}

// PatchOp is the `WriteOp` that patches id.
func PatchOp(id DocID, change PatchChange) (*collectionv1.WriteOp, error) {
	patch := JSONObject{"id": id.documentIDJSON()}
	if change.Mode != "" {
		patch["mode"] = change.Mode
	}
	if change.Set != nil {
		patch["source"] = change.Set
	}
	if len(change.Unset) > 0 {
		patch["deleteKeys"] = change.Unset
	}
	if change.Vectors != nil {
		patch["vectors"] = change.Vectors
	}
	if change.Upsert != nil {
		patch["upsert"] = documentJSON(*change.Upsert)
	}
	return writeOp(JSONObject{"patch": patch})
}

func writeOp(json JSONObject) (*collectionv1.WriteOp, error) {
	op := &collectionv1.WriteOp{}
	if err := decodeJSON(json, op); err != nil {
		return nil, invalidBuild("ops", err)
	}
	return op, nil
}

func writeRequest(target Target, ops []*collectionv1.WriteOp) *collectionv1.WriteDocumentsRequest {
	return &collectionv1.WriteDocumentsRequest{Namespace: target.Namespace, Collection: target.Collection, Ops: ops}
}

func unimplemented(name string) error {
	return &UnimplementedError{LoamsError{
		Code:  connect.CodeUnimplemented,
		Cause: errors.New("this SDK's facade has no " + name + " yet"),
	}}
}

// setWhere sets a request's typed `where` filter (SDKP P0a.3), or refuses
// when the protos this SDK was built from do not have the field.
func setWhere(message proto.Message, where FilterJSON) error {
	if where == nil {
		return nil
	}
	reflected := message.ProtoReflect()
	field := reflected.Descriptor().Fields().ByName("where")
	if field == nil || field.Kind() != protoreflect.MessageKind {
		return invalidBuild("where", errors.New(string(reflected.Descriptor().FullName())+
			" has no typed `where` filter in this SDK's protos"))
	}
	query, err := ToQuery(where)
	if err != nil {
		return err
	}
	reflected.Set(field, protoreflect.ValueOfMessage(query.ProtoReflect()))
	return nil
}

// DocumentCalls are the document conveniences over the generated calls.
type DocumentCalls struct {
	Write  WriteDocumentsFunc
	Scroll ScrollDocumentsFunc
}

// Upsert writes (creates or replaces) documents in one call.
func (d DocumentCalls) Upsert(ctx context.Context, target Target, docs []DocumentInput, options ...CallOption) (*collectionv1.WriteDocumentsResponse, error) {
	if d.Write == nil {
		return nil, unimplemented("documents.write")
	}
	ops := make([]*collectionv1.WriteOp, 0, len(docs))
	for _, doc := range docs {
		op, err := UpsertOp(doc)
		if err != nil {
			return nil, err
		}
		ops = append(ops, op)
	}
	return d.Write(ctx, writeRequest(target, ops), options...)
}

// Delete removes documents by id in one call.
func (d DocumentCalls) Delete(ctx context.Context, target Target, ids []DocID, options ...CallOption) (*collectionv1.WriteDocumentsResponse, error) {
	if d.Write == nil {
		return nil, unimplemented("documents.write")
	}
	ops := make([]*collectionv1.WriteOp, len(ids))
	for i, id := range ids {
		ops[i] = DeleteOp(id)
	}
	return d.Write(ctx, writeRequest(target, ops), options...)
}

// Patch changes part of one document.
func (d DocumentCalls) Patch(ctx context.Context, target Target, id DocID, change PatchChange, options ...CallOption) (*collectionv1.WriteDocumentsResponse, error) {
	if d.Write == nil {
		return nil, unimplemented("documents.write")
	}
	op, err := PatchOp(id, change)
	if err != nil {
		return nil, err
	}
	return d.Write(ctx, writeRequest(target, []*collectionv1.WriteOp{op}), options...)
}

// WriteBatch is a chunked bulk upsert, one chunk in flight (R13).
func (d DocumentCalls) WriteBatch(ctx context.Context, target Target, docs iter.Seq[DocumentInput], batch BatchOptions, options ...CallOption) (BatchResult, error) {
	if d.Write == nil {
		return BatchResult{}, unimplemented("documents.write")
	}
	var buildErr error
	ops := func(yield func(*collectionv1.WriteOp) bool) {
		for doc := range docs {
			op, err := UpsertOp(doc)
			if err != nil {
				buildErr = err
				return
			}
			if !yield(op) {
				return
			}
		}
	}
	result, err := WriteBatch(ctx, func(ctx context.Context, chunk []*collectionv1.WriteOp, key string) (string, error) {
		response, err := d.Write(ctx, writeRequest(target, chunk), append(append([]CallOption{}, options...), WithIdempotencyKey(key))...)
		if err != nil {
			return "", err
		}
		return response.GetToken(), nil
	}, ops, batch)
	if err == nil && buildErr != nil {
		return result, buildErr
	}
	return result, err
}

// ScrollOptions are ScrollAll's options.
type ScrollOptions struct {
	// Where is the typed filter (`ScrollDocumentsRequest.where`, SDKP P0a.3).
	Where FilterJSON
	// Limit is documents per page.
	Limit uint32
	// Select is the projection.
	Select JSONObject
}

// DocumentIter is every document of a scroll. Range over Seq, then check Err.
type DocumentIter struct {
	seq iter.Seq[*collectionv1.Document]
	err error
}

// Seq is the range-over-func over the documents.
func (it *DocumentIter) Seq() iter.Seq[*collectionv1.Document] { return it.seq }

// Err is why the iteration stopped early, or nil.
func (it *DocumentIter) Err() error { return it.err }

// ScrollAll is every document, page by page, following `next` until it is
// unset.
func (d DocumentCalls) ScrollAll(ctx context.Context, target Target, scroll ScrollOptions, options ...CallOption) (*DocumentIter, error) {
	if d.Scroll == nil {
		return nil, unimplemented("documents.scroll")
	}
	base := &collectionv1.ScrollDocumentsRequest{Namespace: target.Namespace, Collection: target.Collection}
	if scroll.Limit > 0 {
		limit := scroll.Limit
		base.Limit = &limit
	}
	if scroll.Select != nil {
		selection, err := structpb.NewStruct(scroll.Select)
		if err != nil {
			return nil, invalidBuild("select", err)
		}
		base.Select = selection
	}
	if err := setWhere(base, scroll.Where); err != nil {
		return nil, err
	}
	it := &DocumentIter{}
	it.seq = func(yield func(*collectionv1.Document) bool) {
		var after *collectionv1.DocumentId
		for {
			request := proto.Clone(base).(*collectionv1.ScrollDocumentsRequest)
			request.After = after
			page, err := d.Scroll(ctx, request, options...)
			if err != nil {
				it.err = err
				return
			}
			for _, doc := range page.GetDocuments() {
				if !yield(doc) {
					return
				}
			}
			if page.GetNext() == nil || page.GetNext().GetId() == nil {
				return
			}
			after = page.GetNext()
		}
	}
	return it, nil
}

// SearchCalls are the search conveniences over `search.query`.
type SearchCalls struct {
	Query SearchFunc
}

// SearchShorthand are the shorthands' common options.
type SearchShorthand struct {
	Namespace string
	Filter    FilterJSON
	Limit     uint32
	// Fields are the text fields; see Search.Text.
	Fields []string
}

func (o SearchShorthand) apply(s Search) Search {
	if o.Namespace != "" {
		s = s.Namespace(o.Namespace)
	}
	if o.Filter != nil {
		s = s.Filter(o.Filter)
	}
	if o.Limit > 0 {
		s = s.Limit(o.Limit)
	}
	return s
}

func (c SearchCalls) run(ctx context.Context, s Search, options []CallOption) (*collectionv1.SearchResponse, error) {
	if c.Query == nil {
		return nil, unimplemented("search.query")
	}
	request, err := s.Build()
	if err != nil {
		return nil, err
	}
	return c.Query(ctx, request, options...)
}

// Text is a text search: one field is a match, several a multi_match, none a
// query_string.
func (c SearchCalls) Text(ctx context.Context, collection, text string, shorthand SearchShorthand, options ...CallOption) (*collectionv1.SearchResponse, error) {
	s := NewSearch(collection).Text(text, TextOptions{Fields: shorthand.Fields})
	return c.run(ctx, shorthand.apply(s), options)
}

// HybridOptions are Hybrid's options.
type HybridOptions struct {
	SearchShorthand
	// Field is the vector field.
	Field string
	// Fusion is RRF() unless set.
	Fusion JSONObject
	// K is each retriever's candidate count.
	K uint32
}

// Hybrid is text plus vector, fused (RRF by default).
func (c SearchCalls) Hybrid(ctx context.Context, collection, text string, vector []float32, hybrid HybridOptions, options ...CallOption) (*collectionv1.SearchResponse, error) {
	fusion := hybrid.Fusion
	if fusion == nil {
		fusion = RRF()
	}
	s := NewSearch(collection).
		Text(text, TextOptions{Fields: hybrid.Fields, K: hybrid.K}).
		Vector(hybrid.Field, vector, VectorOptions{K: hybrid.K}).
		Fuse(fusion)
	return c.run(ctx, hybrid.apply(s), options)
}

// HitIter is every hit of a QueryAll. Range over Seq, then check Err.
type HitIter struct {
	seq iter.Seq[*collectionv1.Hit]
	err error
}

// Seq is the range-over-func over the hits.
func (it *HitIter) Seq() iter.Seq[*collectionv1.Hit] { return it.seq }

// Err is why the iteration stopped early, or nil.
func (it *HitIter) Err() error { return it.err }

// QueryAll is every hit, page after page, following `search_after` from the
// last hit's sort values until a page comes back short. The sort must end in a
// primary-key tiebreaker (`Sort.PK()`), or two hits with equal sort values
// could straddle a page and be skipped or repeated; without one this returns
// `*InvalidArgumentError` before any call.
func (c SearchCalls) QueryAll(ctx context.Context, s Search, options ...CallOption) (*HitIter, error) {
	if !s.HasTiebreak() {
		refused := invalidBuild("sort", errors.New(
			"search.QueryAll needs a sort that ends in the primary key, such as Sort(loams.Sort.Score(), loams.Sort.PK())"))
		refused.(*InvalidArgumentError).Reason = ReasonSearchAfterNeedsTiebreak
		return nil, refused
	}
	if c.Query == nil {
		return nil, unimplemented("search.query")
	}
	if _, err := s.Build(); err != nil {
		return nil, err
	}
	limit := uint32(10)
	switch value := s.json["limit"].(type) {
	case uint32:
		limit = value
	case float64:
		limit = uint32(value)
	}
	it := &HitIter{}
	it.seq = func(yield func(*collectionv1.Hit) bool) {
		page := s
		for {
			answer, err := c.run(ctx, page, options)
			if err != nil {
				it.err = err
				return
			}
			hits := answer.GetHits()
			for _, hit := range hits {
				if !yield(hit) {
					return
				}
			}
			if uint32(len(hits)) < limit || len(hits) == 0 {
				return
			}
			last := hits[len(hits)-1].GetSortValues()
			after := make([]any, len(last))
			for i, value := range last {
				after[i] = value
			}
			page = s.After(after...)
		}
	}
	return it, nil
}
