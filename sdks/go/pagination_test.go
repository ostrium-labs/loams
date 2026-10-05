// SDK2 Task 2's `go_pagination_iterator`.
//
// Design §44 §7.4, D617 and runtime contract R6: the facade exposes both the raw
// page call and an iterator, and the iterator follows `next_page_token` to the
// end. That is one function for every paged RPC, not one per list RPC, because
// the binding says which two fields page — the generated `Pagination` comes from
// `FacadeOptions.pagination`.
//
// **No RPC is paged yet.** `loams.collection.v1.ListCollections` arrives with
// API1 Task 2, so the conformance half of this test waits for the corpus rather
// than inventing a fixture the server cannot answer. What is pinned now is the
// SDK's half — the token threading, the stop condition, and what happens when a
// binding is not paged — against a stub shaped exactly like a paged response.
// When the RPC lands, the `a_paged_call_from_the_corpus_yields_every_item` case
// starts doing the end-to-end half instead of asserting its absence.

package loams

import (
	"context"
	"errors"
	"fmt"
	"testing"

	"loams.dev/go/gen/facade"
)

// collection is a paged item, shaped like `loams.collection.v1.Collection`.
type collection struct {
	ID string
}

// listCollectionsRequest is a paged request, shaped like
// `ListCollectionsRequest`: a page size in, a page token in.
type listCollectionsRequest struct {
	Namespace string
	PageSize  string
	PageToken string
}

// listCollectionsResponse is a paged response, shaped like
// `ListCollectionsResponse`: the items and the next token.
type listCollectionsResponse struct {
	Collections   []collection
	NextPageToken string
}

// pagedBinding is the binding the first paged RPC will use:
// `collections.listCollections`. It is written out here rather than generated
// because the RPC does not exist; when it does, this becomes the generated entry.
var pagedBinding = facade.CallBinding{
	Module:      "collections",
	Name:        "ListCollections",
	ProtoName:   "listCollections",
	Method:      "ListCollections",
	RPC:         "loams.collection.v1.CollectionService/ListCollections",
	Service:     "loams.collection.v1.CollectionService",
	Package:     "loams.collection.v1",
	Idempotency: facade.NoSideEffects,
	Retry:       facade.RetrySafe,
	Streaming:   facade.Unary,
	Pagination: &facade.Pagination{
		ItemsField:         "Collections",
		NextPageTokenField: "NextPageToken",
		PageSizeField:      "PageSize",
		PageTokenField:     "PageToken",
	},
}

// pagedServer is a stub that answers with a fixed script of pages and records the
// request it was given each time.
func pagedServer(pages []listCollectionsResponse) (*[]*listCollectionsRequest, PageFetcher[listCollectionsRequest, listCollectionsResponse]) {
	requests := []*listCollectionsRequest{}
	fetch := func(_ context.Context, request *listCollectionsRequest, _ ...CallOption) (*listCollectionsResponse, error) {
		requests = append(requests, request)
		index := len(requests) - 1
		if index >= len(pages) {
			return nil, fmt.Errorf("the stub has only %d pages", len(pages))
		}
		return &pages[index], nil
	}
	return &requests, fetch
}

// TestGoPaginationIterator is the required test.
func TestGoPaginationIterator(t *testing.T) {
	t.Run(PaginationIterator, func(t *testing.T) {
		testPaginationIterator(t)
	})
}

func testPaginationIterator(t *testing.T) {
	ctx := context.Background()

	t.Run("follows next_page_token to the end and yields items, not pages", func(t *testing.T) {
		requests, fetch := pagedServer([]listCollectionsResponse{
			{Collections: []collection{{ID: "col_1"}, {ID: "col_2"}}, NextPageToken: "p2"},
			{Collections: []collection{{ID: "col_3"}}, NextPageToken: "p3"},
			{Collections: []collection{{ID: "col_4"}}, NextPageToken: ""},
		})
		items := Paginate[listCollectionsRequest, listCollectionsResponse, collection](
			pagedBinding, fetch, ctx, &listCollectionsRequest{Namespace: "acme"})

		var seen []string
		for item := range items.Seq() {
			seen = append(seen, item.ID)
		}
		if err := items.Err(); err != nil {
			t.Fatalf("the iterator reported %v", err)
		}
		want := []string{"col_1", "col_2", "col_3", "col_4"}
		if len(seen) != len(want) {
			t.Fatalf("the iterator yielded %v, want %v", seen, want)
		}
		for index := range want {
			if seen[index] != want[index] {
				t.Fatalf("the iterator yielded %v, want %v", seen, want)
			}
		}
		// Three requests: the first with no token, each later one carrying the
		// previous response's token.
		if len(*requests) != 3 {
			t.Fatalf("the stub saw %d requests, want 3: %+v", len(*requests), *requests)
		}
		if (*requests)[0].PageToken != "" {
			t.Errorf("the first request carried a page token %q", (*requests)[0].PageToken)
		}
		for index, want := range []string{"", "p2", "p3"} {
			if got := (*requests)[index].PageToken; got != want {
				t.Errorf("request %d carried page token %q, want %q", index+1, got, want)
			}
		}
		// The caller's own fields ride along on every page.
		for index, request := range *requests {
			if request.Namespace != "acme" {
				t.Errorf("request %d lost the namespace: %q", index+1, request.Namespace)
			}
		}
	})

	t.Run("never mutates the caller's request", func(t *testing.T) {
		// Go makes reusing one request value easy, and a mutation would make the
		// second call carry the first page's token. This is the kind of bug a
		// range-over-func hides, because the caller cannot see the intermediate
		// requests.
		_, fetch := pagedServer([]listCollectionsResponse{
			{Collections: []collection{{ID: "col_1"}}, NextPageToken: "p2"},
			{Collections: []collection{{ID: "col_2"}}, NextPageToken: ""},
		})
		caller := &listCollectionsRequest{Namespace: "acme", PageSize: "10"}
		items := Paginate[listCollectionsRequest, listCollectionsResponse, collection](
			pagedBinding, fetch, ctx, caller)
		for range items.Seq() {
		}
		if err := items.Err(); err != nil {
			t.Fatalf("the iterator reported %v", err)
		}
		if caller.PageToken != "" {
			t.Errorf("the caller's request now carries a page token %q", caller.PageToken)
		}
		if caller.PageSize != "10" {
			t.Errorf("the caller's page size became %q", caller.PageSize)
		}
	})

	t.Run("stops on one page, and does not send a second request", func(t *testing.T) {
		requests, fetch := pagedServer([]listCollectionsResponse{
			{Collections: []collection{{ID: "col_1"}}, NextPageToken: ""},
		})
		items := Paginate[listCollectionsRequest, listCollectionsResponse, collection](
			pagedBinding, fetch, ctx, &listCollectionsRequest{})
		var seen []string
		for item := range items.Seq() {
			seen = append(seen, item.ID)
		}
		if len(seen) != 1 {
			t.Errorf("the iterator yielded %v, want one item", seen)
		}
		if len(*requests) != 1 {
			t.Errorf("the stub saw %d requests, want 1", len(*requests))
		}
	})

	t.Run("a caller's break stops the paging", func(t *testing.T) {
		requests, fetch := pagedServer([]listCollectionsResponse{
			{Collections: []collection{{ID: "col_1"}, {ID: "col_2"}}, NextPageToken: "p2"},
			{Collections: []collection{{ID: "col_3"}}, NextPageToken: ""},
		})
		items := Paginate[listCollectionsRequest, listCollectionsResponse, collection](
			pagedBinding, fetch, ctx, &listCollectionsRequest{})
		var seen []string
		for item := range items.Seq() {
			seen = append(seen, item.ID)
			if len(seen) == 2 {
				break
			}
		}
		if len(seen) != 2 {
			t.Errorf("breaking out yielded %v, want two items", seen)
		}
		if err := items.Err(); err != nil {
			t.Errorf("breaking out reported %v, want nil: the caller asked to stop", err)
		}
		if len(*requests) != 1 {
			t.Errorf("breaking out still sent %d requests, want 1", len(*requests))
		}
	})

	t.Run("refuses a binding that is not paged, rather than looping once", func(t *testing.T) {
		notPaged, ok := facade.Binding("instance", "GetInstance")
		if !ok {
			t.Fatal("the binding table has no instance.GetInstance")
		}
		if notPaged.Pagination != nil {
			t.Fatal("instance.GetInstance is marked as paged, so this case proves nothing")
		}
		called := 0
		items := Paginate[listCollectionsRequest, listCollectionsResponse, collection](
			notPaged,
			func(context.Context, *listCollectionsRequest, ...CallOption) (*listCollectionsResponse, error) {
				called++
				return &listCollectionsResponse{}, nil
			},
			ctx, &listCollectionsRequest{})
		for range items.Seq() {
			t.Error("a non-paged call yielded an item")
		}
		err := items.Err()
		if err == nil {
			t.Fatal("a non-paged call reported no error")
		}
		if !containsSubstring(err.Error(), "not a paged call") {
			t.Errorf("the error is %q, want it to say the call is not paged", err.Error())
		}
		if called != 0 {
			t.Errorf("a non-paged call sent %d requests, want none", called)
		}
		if !IsLoamsError(err) {
			t.Errorf("the refusal is %T, want a *LoamsError", err)
		}
	})

	t.Run("tolerates a page whose items field is absent", func(t *testing.T) {
		// A server that omits an empty repeated field is legal proto3. Yielding
		// nothing and moving on is right; throwing would break a caller over a
		// message the server is allowed to send.
		page := 0
		items := Paginate[listCollectionsRequest, listCollectionsResponse, collection](
			pagedBinding,
			func(context.Context, *listCollectionsRequest, ...CallOption) (*listCollectionsResponse, error) {
				page++
				if page < 2 {
					return &listCollectionsResponse{NextPageToken: "p2"}, nil
				}
				return &listCollectionsResponse{NextPageToken: ""}, nil
			},
			ctx, &listCollectionsRequest{})
		var seen []collection
		for item := range items.Seq() {
			seen = append(seen, item)
		}
		if err := items.Err(); err != nil {
			t.Fatalf("the iterator reported %v", err)
		}
		if len(seen) != 0 {
			t.Errorf("the iterator yielded %v, want nothing", seen)
		}
		if page != 2 {
			t.Errorf("the stub saw %d requests, want 2", page)
		}
	})

	t.Run("a failure on a page is reported through Err, not thrown", func(t *testing.T) {
		// A range-over-func cannot return an error, which is why the iterator
		// carries one. A caller who skips `Err` sees a silently short list, which
		// looks exactly like the end of the list — the same trap `*Stream` has.
		page := 0
		items := Paginate[listCollectionsRequest, listCollectionsResponse, collection](
			pagedBinding,
			func(context.Context, *listCollectionsRequest, ...CallOption) (*listCollectionsResponse, error) {
				page++
				if page == 1 {
					return &listCollectionsResponse{
						Collections:   []collection{{ID: "col_1"}},
						NextPageToken: "p2",
					}, nil
				}
				return nil, newUnavailableError("the node is restarting")
			},
			ctx, &listCollectionsRequest{})
		var seen []string
		for item := range items.Seq() {
			seen = append(seen, item.ID)
		}
		if len(seen) != 1 {
			t.Errorf("the iterator yielded %v before the failure, want one item", seen)
		}
		var unavailable *UnavailableError
		if !errors.As(items.Err(), &unavailable) {
			t.Errorf("the failure is %T, want an *UnavailableError", items.Err())
		}
		if page != 2 {
			t.Errorf("the stub saw %d requests, want 2: the iterator must stop at the failure", page)
		}
	})

	t.Run("the request field names come from the binding", func(t *testing.T) {
		pageSize, pageToken, err := PageFields(pagedBinding)
		if err != nil {
			t.Fatalf("PageFields: %v", err)
		}
		if pageSize != "PageSize" || pageToken != "PageToken" {
			t.Errorf("PageFields returned %q and %q", pageSize, pageToken)
		}
		notPaged, _ := facade.Binding("instance", "GetInstance")
		if _, _, err := PageFields(notPaged); err == nil {
			t.Error("PageFields accepted a call that is not paged")
		}
	})

	t.Run("PaginateCall resolves the binding from a module and call name", func(t *testing.T) {
		// The generated alias `collections.listAll` arrives with
		// `ListCollections` (API1 Task 2); this is the same iterator with the
		// binding resolved, so it already works and the alias is only a
		// convenience.
		client := MustNew(Options{Endpoint: "http://127.0.0.1:1"})
		if _, err := PaginateCall[listCollectionsRequest, listCollectionsResponse, collection](
			client, "collections", "ListCollections",
			func(context.Context, *listCollectionsRequest, ...CallOption) (*listCollectionsResponse, error) {
				return &listCollectionsResponse{}, nil
			}, ctx, &listCollectionsRequest{}); err == nil {
			t.Error("PaginateCall resolved a call the generator has not seen")
		}
		// And a call that does exist resolves, so the refusal above is about the
		// missing annotation and not about the resolver.
		items, err := PaginateCall[listCollectionsRequest, listCollectionsResponse, collection](
			client, "instance", "GetInstance",
			func(context.Context, *listCollectionsRequest, ...CallOption) (*listCollectionsResponse, error) {
				t.Error("PaginateCall sent a request for a call that is not paged")
				return &listCollectionsResponse{}, nil
			}, ctx, &listCollectionsRequest{})
		if err != nil {
			t.Fatalf("PaginateCall could not resolve instance.GetInstance: %v", err)
		}
		for range items.Seq() {
		}
		if !containsSubstring(items.Err().Error(), "not a paged call") {
			t.Errorf("the refusal is %q", items.Err())
		}
	})

	t.Run("a_paged_call_from_the_corpus_yields_every_item", func(t *testing.T) {
		// This is the end-to-end half, and it is a **deliberate skip** until API1
		// Task 2 lands `ListCollections`: there is nothing to call, and a fixture
		// for an RPC the server does not serve would be a test of the stub rather
		// than of the SDK. The assertion below is the opposite of an omission — it
		// fails as soon as the generator emits a paged binding, so the moment the
		// RPC lands this case has to be written rather than quietly passing.
		for _, module := range facade.Modules {
			for _, call := range module.Calls {
				if call.Pagination != nil {
					t.Fatalf("%s.%s is paged, so the end-to-end half of %s must be written now",
						module.Name, call.Name, PaginationIterator)
				}
			}
		}
		// The corpus has no paged case either, for the same reason.
		for _, entry := range readCorpus(t).Cases {
			if containsSubstring(entry.About, "page") {
				t.Fatalf("the corpus has a paging case %q, so the end-to-end half must be written now", entry.Name)
			}
		}
	})
}
