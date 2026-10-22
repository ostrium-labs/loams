// The collection handle (SDKP §2.8).
//
//	notes := client.Collection("notes")
//	_, err := notes.Upsert(ctx, []loams.DocumentInput{{ID: loams.UintID(1), Source: loams.JSONObject{"title": "Refunds"}}})
//	hits, err := notes.Search(ctx, notes.Query().Text("refund", loams.TextOptions{Fields: []string{"body"}}))
//
// Every call delegates to the module call of the same meaning with
// `collection` (and `namespace`, when the handle was given one) filled in. The
// handle holds no state of its own: two handles on the same collection are
// interchangeable.

package loams

import (
	"context"
	"iter"

	collectionv1 "loams.dev/go/gen/loams/collection/v1"
)

// HandleModules are the calls a handle delegates to. A nil call answers
// `*UnimplementedError`, so a handle over a facade that lacks a module fails
// clearly rather than panicking.
type HandleModules struct {
	Documents      DocumentCalls
	GetDocuments   GetDocumentsFunc
	CountDocuments CountDocumentsFunc
	Search         SearchCalls
	GetCollection  GetCollectionFunc
	DropCollection DropCollectionFunc
}

// CollectionHandle is one collection, by name, with its calls bound to it.
type CollectionHandle struct {
	modules   HandleModules
	name      string
	namespace string
}

// NewCollectionHandle is a handle over modules. namespace is optional; empty
// is the client's default.
func NewCollectionHandle(modules HandleModules, name string, namespace ...string) *CollectionHandle {
	return &CollectionHandle{modules: modules, name: name, namespace: first(namespace)}
}

// Collection is a handle on one collection (SDKP §2.8): its calls are the app
// modules' with `collection` (and `namespace`) filled in.
func (c *Client) Collection(name string, namespace ...string) *CollectionHandle {
	return NewCollectionHandle(c.handleModules, name, namespace...)
}

// Name is the collection's name; Namespace the handle's namespace, or "".
func (h *CollectionHandle) Name() string      { return h.name }
func (h *CollectionHandle) Namespace() string { return h.namespace }

func (h *CollectionHandle) target() Target {
	return Target{Namespace: h.namespace, Collection: h.name}
}

// Upsert writes (creates or replaces) documents.
func (h *CollectionHandle) Upsert(ctx context.Context, docs []DocumentInput, options ...CallOption) (*collectionv1.WriteDocumentsResponse, error) {
	return h.modules.Documents.Upsert(ctx, h.target(), docs, options...)
}

// Get reads documents by id.
func (h *CollectionHandle) Get(ctx context.Context, ids []DocID, options ...CallOption) (*collectionv1.GetDocumentsResponse, error) {
	if h.modules.GetDocuments == nil {
		return nil, unimplemented("documents.get")
	}
	request := &collectionv1.GetDocumentsRequest{Namespace: h.namespace, Collection: h.name}
	for _, id := range ids {
		request.Ids = append(request.Ids, id.Proto())
	}
	return h.modules.GetDocuments(ctx, request, options...)
}

// Delete removes documents by id.
func (h *CollectionHandle) Delete(ctx context.Context, ids []DocID, options ...CallOption) (*collectionv1.WriteDocumentsResponse, error) {
	return h.modules.Documents.Delete(ctx, h.target(), ids, options...)
}

// Patch changes part of one document.
func (h *CollectionHandle) Patch(ctx context.Context, id DocID, change PatchChange, options ...CallOption) (*collectionv1.WriteDocumentsResponse, error) {
	return h.modules.Documents.Patch(ctx, h.target(), id, change, options...)
}

// WriteBatch is a chunked bulk upsert (R13).
func (h *CollectionHandle) WriteBatch(ctx context.Context, docs iter.Seq[DocumentInput], batch BatchOptions, options ...CallOption) (BatchResult, error) {
	return h.modules.Documents.WriteBatch(ctx, h.target(), docs, batch, options...)
}

// Count is how many documents match where (all of them when it is nil).
func (h *CollectionHandle) Count(ctx context.Context, where FilterJSON, options ...CallOption) (*collectionv1.CountDocumentsResponse, error) {
	if h.modules.CountDocuments == nil {
		return nil, unimplemented("documents.count")
	}
	request := &collectionv1.CountDocumentsRequest{Namespace: h.namespace, Collection: h.name}
	if err := setWhere(request, where); err != nil {
		return nil, err
	}
	return h.modules.CountDocuments(ctx, request, options...)
}

// ScrollAll is every document, page by page.
func (h *CollectionHandle) ScrollAll(ctx context.Context, scroll ScrollOptions, options ...CallOption) (*DocumentIter, error) {
	return h.modules.Documents.ScrollAll(ctx, h.target(), scroll, options...)
}

// Query is a search builder already pointed at this collection.
func (h *CollectionHandle) Query() Search {
	s := NewSearch(h.name)
	if h.namespace != "" {
		s = s.Namespace(h.namespace)
	}
	return s
}

// Search runs a search; the builder's collection is replaced by this one.
func (h *CollectionHandle) Search(ctx context.Context, s Search, options ...CallOption) (*collectionv1.SearchResponse, error) {
	s = s.with("collection", h.name)
	if h.namespace != "" {
		s = s.Namespace(h.namespace)
	}
	return h.modules.Search.run(ctx, s, options)
}

// VectorSearchOptions are VectorSearch's options.
type VectorSearchOptions struct {
	VectorOptions
	Limit uint32
	// Where is a post-filter (the search's `filter`); VectorOptions.Filter is
	// the retriever's pre-filter.
	Where FilterJSON
}

// VectorSearch is a kNN search on one vector field.
func (h *CollectionHandle) VectorSearch(ctx context.Context, field string, vector []float32, search VectorSearchOptions, options ...CallOption) (*collectionv1.SearchResponse, error) {
	s := h.Query().Vector(field, vector, search.VectorOptions)
	if search.Where != nil {
		s = s.Filter(search.Where)
	}
	if search.Limit > 0 {
		s = s.Limit(search.Limit)
	}
	return h.Search(ctx, s, options...)
}

// Info is the collection's schema and stats.
func (h *CollectionHandle) Info(ctx context.Context, options ...CallOption) (*collectionv1.CollectionInfo, error) {
	if h.modules.GetCollection == nil {
		return nil, unimplemented("collections.get")
	}
	return h.modules.GetCollection(ctx, &collectionv1.GetCollectionRequest{Namespace: h.namespace, Collection: h.name}, options...)
}

// Drop drops the collection.
func (h *CollectionHandle) Drop(ctx context.Context, options ...CallOption) (*collectionv1.DropCollectionResponse, error) {
	if h.modules.DropCollection == nil {
		return nil, unimplemented("collections.drop")
	}
	return h.modules.DropCollection(ctx, &collectionv1.DropCollectionRequest{Namespace: h.namespace, Collection: h.name}, options...)
}
