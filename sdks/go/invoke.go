// The two generic invokers — unary and server-streaming — that the generated
// module methods delegate to.
//
// They are in their own file because this is the whole of the glue between a
// `client.Instance()` method and a wire call: the module method hands over a
// `facade.CallBinding` and the generated stub's method value, and everything
// else in the SDK happens here and in `call.go`.
//
// They are **free functions taking the invoker**, not methods on it, because Go
// does not allow a method to have type parameters and `CallInvoker` cannot be
// generic: one client carries one client per service, and the request and
// response types differ per RPC. They are exported so a caller who builds their
// own client — a test, a proxy — can use the same runtime an application does.

package loams

import (
	"context"
	"errors"
	"net/http"

	connect "connectrpc.com/connect"

	"loams.dev/go/gen/facade"
)

// UnaryMethod is the shape of one generated unary RPC: what
// `protoc-gen-connect-go` writes for `rpc R(Req) returns (Res)`.
type UnaryMethod[Req, Res any] func(context.Context, *connect.Request[Req]) (*connect.Response[Res], error)

// ServerMethod is the shape of one generated server-streaming RPC.
type ServerMethod[Req, Res any] func(context.Context, *connect.Request[Req]) (*connect.ServerStreamForClient[Res], error)

// Unary makes one unary call with the whole runtime contract applied.
//
// The request is keyed **once**, before the first attempt, and the same message
// goes to every attempt: that is R3, and it is why a retried mutation is one
// write rather than two.
func Unary[Req, Res any](
	invoker *CallInvoker,
	ctx context.Context,
	binding facade.CallBinding,
	method UnaryMethod[Req, Res],
	request *Req,
	options ...CallOption,
) (*Res, error) {
	settings := &callSettings{}
	settings.resolve(options)

	keyed := ApplyIdempotencyKey(request, settings.idempotencyKey, binding.TakesIdempotencyKey)
	message, ok := keyed.Request.(*Req)
	if !ok {
		// ApplyIdempotencyKey only clones, so this cannot happen; say so rather
		// than send a nil message and fail inside the codec.
		return nil, newInternalError(binding.RPC, "the keyed request is a %T, not %T", keyed.Request, request)
	}

	session := invoker.sessionFor(settings)
	plan := invoker.plan(binding, settings, keyed.Keyed)
	// An explicit token wins; otherwise the session's, which is what makes
	// WithSessionConsistency read-your-writes rather than record a token and
	// never send it (R4).
	token := consistencyToken(settings, session)

	result, err := CallWithRetry(ctx, message, func(ctx context.Context, _ Attempt) (any, error) {
		req := connect.NewRequest(message)
		headers, headerErr := invoker.bearer(ctx, settings.headers)
		if headerErr != nil {
			return nil, headerErr
		}
		req.Header().Del(ConsistencyHeader)
		applyHeaders(req.Header(), headers)
		if token != "" {
			req.Header().Set(ConsistencyHeader, token)
		}
		response, callErr := method(ctx, req)
		if callErr != nil {
			return nil, callErr
		}
		return response.Msg, nil
	}, plan, binding.RPC)
	if err != nil {
		return nil, err
	}
	response, ok := result.(*Res)
	if !ok {
		return nil, newInternalError(binding.RPC, "the call returned %T, not %T", result, response)
	}
	// A store that cannot merge a token counts it and carries on: the RPC
	// succeeded, and turning that into an error would make a caller that
	// retries on it perform the write twice.
	recordConsistency(session, response)
	return response, nil
}

// ServerStream opens one server stream with the errors mapped.
//
// There is no resume here and no cursor tracking; `WithStreamResume` adds both,
// and it is a CallOption so the same generated method serves both. The error
// mapping is not optional, though: a refusal on a stream arrives **inside** the
// Connect envelope rather than as an HTTP status — the corpus's `live_watch`
// case is exactly that, a 200 with an error frame — so a caller iterating the
// raw stream would see a failure with no `reason`, which is the one place in an
// SDK where `reason` could go missing.
func ServerStream[Req, Res any](
	invoker *CallInvoker,
	ctx context.Context,
	binding facade.CallBinding,
	method ServerMethod[Req, Res],
	request *Req,
	options ...CallOption,
) (*Stream[Res], error) {
	settings := &callSettings{}
	settings.resolve(options)

	session := invoker.sessionFor(settings)
	token := consistencyToken(settings, session)

	open := func(ctx context.Context, message *Req) (*connect.ServerStreamForClient[Res], error) {
		req := connect.NewRequest(message)
		headers, err := invoker.bearer(ctx, settings.headers)
		if err != nil {
			return nil, err
		}
		req.Header().Del(ConsistencyHeader)
		applyHeaders(req.Header(), headers)
		if token != "" {
			req.Header().Set(ConsistencyHeader, token)
		}
		return method(ctx, req)
	}

	// R1 on a stream: one refresh and one re-open, and only while nothing has
	// been yielded. Once messages are flowing the caller is holding a position
	// in the stream, and replaying from the start would duplicate everything
	// they have already seen — the resume is the only correct answer, and it is
	// WithStreamResume's job.
	refreshed := false
	for {
		source, err := open(ctx, request)
		if err == nil {
			return newStream[Req, Res](ctx, binding, source, request, settings, invoker.maxRetries, open), nil
		}
		mapped := ToLoamsError(err, binding.RPC)
		var expired *TokenExpiredError
		if refreshed || invoker.source == nil || !errors.As(mapped, &expired) {
			return nil, mapped
		}
		refreshed = true
		if refreshErr := invoker.source.Refresh(ctx); refreshErr != nil {
			return nil, ToLoamsError(refreshErr, binding.RPC)
		}
	}
}

// newStream wraps an opened connect stream, wiring the resume in if the caller
// asked for one.
func newStream[Req, Res any](
	ctx context.Context,
	binding facade.CallBinding,
	source *connect.ServerStreamForClient[Res],
	request *Req,
	settings *callSettings,
	defaultMaxRetries int,
	open func(context.Context, *Req) (*connect.ServerStreamForClient[Res], error),
) *Stream[Res] {
	stream := &Stream[Res]{
		ctx:        ctx,
		binding:    bindingInfo{RPC: binding.RPC, RetrySafe: binding.Retry == facade.RetrySafe},
		source:     source,
		maxRetries: defaultMaxRetries,
	}
	resume, ok := settings.resume.(StreamResume[Req, Res])
	if !ok {
		return stream
	}
	stream.cursorOf = resume.CursorOf
	stream.onCursor = resume.OnCursor
	if resume.MaxRetries >= 0 {
		stream.maxRetries = resume.MaxRetries
	}
	stream.open = func(ctx context.Context, cursor string) (Receiver[Res], error) {
		reopened := resume.Resume(cursor, request)
		if reopened == nil {
			return nil, newInternalError(binding.RPC, "the resume function returned no request")
		}
		next, err := open(ctx, reopened)
		if err != nil {
			return nil, err
		}
		return next, nil
	}
	return stream
}

// consistencyToken resolves which token a read goes out with: an explicit one
// wins, otherwise the session's — which is what makes WithSessionConsistency
// read-your-writes rather than record a token and never send it (R4).
func consistencyToken(settings *callSettings, session ConsistencyTokenStore) string {
	if settings.consistency != nil && settings.consistency.Token != "" {
		return settings.consistency.Token
	}
	if session == nil {
		return ""
	}
	return session.Current()
}

// applyHeaders copies headers onto a request, replacing any with the same name.
// It is a replacement rather than an add, so a caller's header behaves the same
// on every attempt and a refreshed bearer does not stack up beside the stale
// one.
func applyHeaders(into, from http.Header) {
	for name, values := range from {
		if len(values) > 0 {
			into[name] = values
		}
	}
}
