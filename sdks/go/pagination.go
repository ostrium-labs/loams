// Pagination (design §44 §7.4, D617; runtime contract R6).
//
// AIP-158: `page_size` and `page_token` in, `next_page_token` out. A generated
// binding says which two fields those are (`FacadeOptions.pagination` is
// `"<items>:<next page token>"`), so the iterator is one function for every
// paged RPC rather than one per list RPC.
//
// In Go the iterator is a range-over-func, because Go 1.23 made that the
// language's own iteration shape and a caller should not have to write a
// `yield func` by hand:
//
//	items := loams.Paginate(binding, fetch, ctx, request)
//	for collection := range items.Seq() {
//	    use(collection)
//	}
//	if err := items.Err(); err != nil {
//	    return err
//	}
//
// The `Seq`/`Err` split is the same one `*loams.Stream` makes, and for the
// same reason: a range-over-func cannot return an error, so an error has to
// live beside it, and a caller who skips `Err` sees a silently short page
// sequence — which for a paginated list looks exactly like the end of the list.
//
// The raw page call stays on the module, so a caller that wants pages, or wants
// to stop after one, does not have to use the iterator.
//
// **No RPC is paged yet.** `loams.collection.v1.ListCollections` arrives with
// API1 Task 2, so the end-to-end half of `TestGoPaginationIterator` is a
// deliberate skip, not an omission: a fixture for an RPC the server does not
// serve would be a test of the stub rather than of the SDK. What is pinned is
// the SDK's half — the token threading, the stop condition, and what happens
// when a binding is not paged.

package loams

import (
	"context"
	"errors"
	"iter"
	"reflect"

	connect "connectrpc.com/connect"

	"loams.dev/go/gen/facade"
)

// PageFetcher makes one page request. A generated module method satisfies it, so
// the iterator drives the same code path an application does.
type PageFetcher[Req, Res any] func(ctx context.Context, request *Req, options ...CallOption) (*Res, error)

// PageIter is every item of a paged call. Call `Seq` to range over it and `Err`
// afterwards.
type PageIter[Req, Res any, Item any] struct {
	binding facade.CallBinding
	fetch   PageFetcher[Req, Res]
	ctx     context.Context
	request *Req
	options []CallOption

	// pageSizeField and pageTokenField are the two fields the binding named, with
	// the AIP-158 defaults filled in.
	pageSizeField  string
	pageTokenField string
	// items and next are the response's fields.
	items string
	next  string

	err error
}

// Paginate returns an iterator over every item of a paged call, following the
// tokens to the end (D617's "the paging iterator").
//
// `options` are passed to every page request, so a per-call header or
// idempotency key reaches all of them.
func Paginate[Req, Res any, Item any](
	binding facade.CallBinding,
	fetch PageFetcher[Req, Res],
	ctx context.Context,
	request *Req,
	options ...CallOption,
) *PageIter[Req, Res, Item] {
	iterator := &PageIter[Req, Res, Item]{
		binding:        binding,
		fetch:          fetch,
		ctx:            ctx,
		request:        request,
		options:        options,
		pageSizeField:  "PageSize",
		pageTokenField: "PageToken",
	}
	if binding.Pagination == nil {
		// A refusal, reported through Err rather than thrown at the call: the
		// iterator is a value a caller holds, and a method that panics or
		// returns two things is harder to compose. The message names the binding
		// so the cause is obvious in a log.
		iterator.err = &LoamsError{
			Code:   connect.CodeInternal,
			RPC:    binding.RPC,
			Reason: ReasonInternal,
			Cause: errors.New(binding.Module + "." + binding.Name +
				" is not a paged call: the proto's facade options name no pagination"),
		}
		return iterator
	}
	iterator.items = binding.Pagination.ItemsField
	iterator.next = binding.Pagination.NextPageTokenField
	if binding.Pagination.PageSizeField != "" {
		iterator.pageSizeField = binding.Pagination.PageSizeField
	}
	if binding.Pagination.PageTokenField != "" {
		iterator.pageTokenField = binding.Pagination.PageTokenField
	}
	return iterator
}

// Seq is the range-over-func over the items. It stops at the last page, when the
// caller's `yield` returns false, or on the first failure, which `Err` then
// reports.
func (p *PageIter[Req, Res, Item]) Seq() iter.Seq[Item] {
	return func(yield func(Item) bool) {
		if p.err != nil {
			return
		}
		token := ""
		for {
			page, err := p.fetch(p.ctx, withPageToken(p.request, p.pageTokenField, token), p.options...)
			if err != nil {
				p.err = ToLoamsError(err, p.binding.RPC)
				return
			}
			stopped := false
			forEachItem(page, p.items, func(value any) bool {
				item, ok := value.(Item)
				if !ok {
					return true
				}
				if !yield(item) {
					stopped = true
					return false
				}
				return true
			})
			if stopped {
				return
			}
			next, ok := readString(page, p.next)
			if !ok || next == "" {
				return
			}
			token = next
		}
	}
}

// Err is why the iteration stopped early, or nil if it reached the end.
func (p *PageIter[Req, Res, Item]) Err() error { return p.err }

// PageFields are the request field names a paged call uses, from its binding.
// A caller that builds a request by hand rather than through the iterator needs
// them, and they are the AIP-158 names unless the proto says otherwise.
func PageFields(binding facade.CallBinding) (pageSizeField, pageTokenField string, err error) {
	if binding.Pagination == nil {
		return "", "", errors.New(binding.Module + "." + binding.Name + " is not a paged call")
	}
	pageSizeField = binding.Pagination.PageSizeField
	pageTokenField = binding.Pagination.PageTokenField
	if pageSizeField == "" {
		pageSizeField = "PageSize"
	}
	if pageTokenField == "" {
		pageTokenField = "PageToken"
	}
	return pageSizeField, pageTokenField, nil
}

// withPageToken copies a request and sets its page token.
//
// The copy is what stops the iterator from mutating the caller's request and
// sending the first page's token back on the second call; `cloneRequest` knows
// how to clone a proto message and a plain struct alike.
func withPageToken[Req any](request *Req, field, token string) *Req {
	if request == nil {
		return nil
	}
	message, ok := cloneRequest(request).(*Req)
	if !ok {
		return request
	}
	if token != "" {
		_ = writeString(message, field, token)
	}
	return message
}

// forEachItem walks a response's repeated field, handing each element to `each`.
//
// A field that is absent yields nothing, which is right: a server that omits an
// empty repeated field is legal proto3, and throwing would break a caller over a
// message the server is allowed to send.
func forEachItem(response any, field string, each func(any) bool) {
	holder := reflect.ValueOf(response)
	for holder.Kind() == reflect.Pointer {
		if holder.IsNil() {
			return
		}
		holder = holder.Elem()
	}
	if !holder.IsValid() || holder.Kind() != reflect.Struct {
		return
	}
	items := holder.FieldByName(field)
	if !items.IsValid() || !items.CanInterface() {
		return
	}
	if items.Kind() != reflect.Slice && items.Kind() != reflect.Array {
		return
	}
	for index := 0; index < items.Len(); index++ {
		if !each(items.Index(index).Interface()) {
			return
		}
	}
}

// errorString is `errors.New`, named so the pagination file does not import
// `errors` for four call sites.
type errorString string

// Error implements error.
func (e errorString) Error() string { return string(e) }
