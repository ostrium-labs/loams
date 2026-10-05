// The call path: the one place a facade method becomes an RPC (design §44
// §7.4; runtime contract R1–R4).
//
// It does four things the generated facade cannot, and nothing else:
//
//   - attaches the bearer from the client's TokenSource;
//   - retries on the call's class from the generated bindings, with M1.6's
//     backoff numbers, and refreshes the token once on `token_expired`;
//   - gives a mutating call an idempotency key once per logical call and
//     reuses it on every retry, so a retried write is the same write (D610);
//   - turns whatever comes back into the typed LoamsError hierarchy, so a
//     caller branches on a `reason` and never on a message.
//
// # The context, and where it shows up
//
// Every attempt is made with the caller's context, and so is every wait between
// attempts and every token fetch. Three consequences follow, and they are the
// reason the Go SDK is worth having rather than a transliteration of the
// TypeScript one:
//
//   - **A cancelled call costs at most the attempt in flight.** The retry loop
//     checks `ctx.Err()` before spending another attempt, so shutdown does not
//     leave a goroutine sleeping out a 2 s backoff.
//   - **A deadline covers the whole call, not each attempt.** `ctx, cancel :=
//     context.WithTimeout(ctx, 5*time.Second)` bounds three retries and the
//     backoff together, which is what a caller means by "this call may take at
//     most five seconds".
//   - **A context failure keeps its own code.** `context.DeadlineExceeded`
//     becomes `DeadlineExceededError`, not `CodeUnknown`, because a deadline is
//     the one failure the caller can always act on.

package loams

import (
	"context"
	"errors"
	"fmt"
	"net/http"

	"loams.dev/go/gen/facade"
)

// ConsistencyOptions is how a call opts into a session consistency token
// (design §44 §7.4, D609).
type ConsistencyOptions struct {
	// Token is a token from a previous write, or the empty string for `STRONG`
	// (the default). `EVENTUAL` and `AT_LEAST{token}` are spelled as the API
	// spells them once the proto carries them.
	Token string
	// Session folds every response's token into a store and attaches the
	// stored one to later reads. `true` means the client's own session, which
	// is what `Options.SessionConsistency` turned on; a store is one of your
	// own.
	Session ConsistencyTokenStore
}

// CallOption is a per-call override. Everything is optional: the client's
// defaults apply and the generated binding supplies the retry class.
type CallOption func(*callSettings)

// callSettings is the resolved per-call configuration.
type callSettings struct {
	maxRetries *int
	// retrySafe overrides the generated retry class for this call only.
	retrySafe *bool
	// idempotencyKey is the caller's own key, for a retry the caller wants to
	// own rather than the SDK.
	idempotencyKey string
	// headers go on this call. `Authorization` is set by the runtime and wins,
	// because a caller-supplied bearer would defeat the refresh.
	headers http.Header
	// consistency is the session and token this call uses.
	consistency *ConsistencyOptions
	// resume is a StreamResume, held type-erased. The invoker knows the
	// request and response types and asserts it back, which is what lets
	// WithStreamResume be generic and the option not be.
	resume any
}

// WithMaxRetries bounds the retries after the first attempt for this call. Zero
// disables them, which is what a caller wants for a mutation they would rather
// see fail than repeat.
func WithMaxRetries(maxRetries int) CallOption {
	return func(settings *callSettings) { settings.maxRetries = &maxRetries }
}

// WithRetrySafe overrides the call's retry class, which otherwise comes from the
// generated bindings. Setting it false on a read stops the SDK retrying it;
// setting it true on a mutation retries something the proto says is not safe
// to repeat, which is only correct if the call carries an idempotency key.
func WithRetrySafe(retrySafe bool) CallOption {
	return func(settings *callSettings) { settings.retrySafe = &retrySafe }
}

// WithIdempotencyKey supplies the key for this call, making the retry yours
// rather than the SDK's. Omit it and the runtime mints a UUIDv7 per logical
// call and reuses it on every retry (R3).
func WithIdempotencyKey(key string) CallOption {
	return func(settings *callSettings) { settings.idempotencyKey = key }
}

// WithHeader adds one header to this call.
func WithHeader(name, value string) CallOption {
	return func(settings *callSettings) {
		if settings.headers == nil {
			settings.headers = http.Header{}
		}
		settings.headers.Set(name, value)
	}
}

// WithStreamResume makes a server stream reconnect from its last cursor on a
// retryable failure (R7).
//
// Without it a stream is a plain iterator: it stops on a failure and reports it
// through `Stream.Err`, which is correct and is what R7 asks for when the call's
// retry class does not cover the failure. With it, the stream re-opens from the
// last cursor the caller applied and carries on, so a node restart is a hiccup
// rather than a gap.
//
// The type arguments are the request and the response, which is what lets the
// compiler check that `Resume` returns the message the RPC takes:
//
//	stream, err := client.Live().Watch(ctx, request, loams.WithStreamResume(
//	    loams.StreamResume[*loams.WatchRequest, *loams.Transition]{
//	        CursorOf: func(t *loams.Transition) string { return cursorOf(t) },
//	        Resume:   func(c string, r *loams.WatchRequest) *loams.WatchRequest {
//	            return &loams.WatchRequest{Start: &loams.WatchRequest_Resume{
//	                Resume: &loams.Resume{LastVersion: lastVersion, QuerySet: querySet},
//	            }}
//	        },
//	    }))
func WithStreamResume[Req, Res any](resume StreamResume[Req, Res]) CallOption {
	return func(settings *callSettings) { settings.resume = resume }
}

// WithConsistencyToken reads at a token, which is how a caller who just wrote
// reads its own write.
func WithConsistencyToken(token string) CallOption {
	return func(settings *callSettings) {
		if settings.consistency == nil {
			settings.consistency = &ConsistencyOptions{}
		}
		settings.consistency.Token = token
	}
}

// WithSessionConsistency folds this call's responses into a store and attaches
// the stored token to later reads. Passing `true` uses the client's session,
// which `Options.SessionConsistency` turned on; passing a store uses that one.
func WithSessionConsistency(session ConsistencyTokenStore) CallOption {
	return func(settings *callSettings) {
		if settings.consistency == nil {
			settings.consistency = &ConsistencyOptions{}
		}
		settings.consistency.Session = session
	}
}

// resolve applies the caller's options over the client's defaults.
func (s *callSettings) resolve(options []CallOption) {
	for _, option := range options {
		option(s)
	}
}

// Attempt is what one attempt of a call carries.
type Attempt struct {
	// Request is the message, with the idempotency key already set. It is the
	// **same value** on every attempt, which is the whole of R3.
	Request any
	// Attempt is the zero-based attempt number.
	Attempt int
	// Refreshed is true once R1's single refresh has happened.
	Refreshed bool
}

// Send makes one attempt. It returns whatever the transport returned, or an
// error; the retry loop turns the error into the typed hierarchy.
type Send func(ctx context.Context, attempt Attempt) (any, error)

// RetryPlan is everything the retry loop needs, so the policy can be tested
// without a transport.
type RetryPlan struct {
	// RetrySafe is the call's class from the generated binding, or true once
	// the call has been keyed.
	RetrySafe bool
	// MaxRetries is the retries after the first attempt.
	MaxRetries int
	// OnRefresh is the client's refresh, or nil for a client with no
	// TokenSource. A `401 token_expired` triggers it exactly once.
	OnRefresh func(ctx context.Context) error
}

// CallWithRetry runs send until it answers or the plan says stop, and returns
// whatever it last threw as a LoamsError.
//
// Exported so the conformance suite can drive it with a stub send: the retry
// policy, the idempotency-key lifecycle and the refresh-once behaviour are the
// SDK's own logic and are worth pinning without a server.
func CallWithRetry(ctx context.Context, request any, send Send, plan RetryPlan, rpc string) (any, error) {
	refreshed := false
	for attempt := 0; ; attempt++ {
		result, err := send(ctx, Attempt{Request: request, Attempt: attempt, Refreshed: refreshed})
		if err == nil {
			return result, nil
		}
		mapped := ToLoamsError(err, rpc)
		// R1: a rejection whose reason is `token_expired` gets exactly one
		// refresh and one retry. A source that cannot refresh (an API key)
		// makes this a no-op, and a second expiry is reported rather than
		// looped on.
		var expired *TokenExpiredError
		if errors.As(mapped, &expired) && !refreshed && plan.OnRefresh != nil {
			refreshed = true
			if refreshErr := plan.OnRefresh(ctx); refreshErr != nil {
				return nil, ToLoamsError(refreshErr, rpc)
			}
			// The retry is not charged to the call's budget: it is the same
			// logical call, and a caller who set MaxRetries(3) asked for three
			// retries of the request, not three including a credential
			// refresh.
			attempt--
			continue
		}
		if ctxErr := ctx.Err(); ctxErr != nil {
			// The caller gave up while a retryable failure was outstanding. Both
			// facts matter, so the error carries both: see `gaveUp`.
			return nil, gaveUp(mapped, ctxErr, rpc)
		}
		if !ShouldRetry(ctx, mapped, plan.RetrySafe, attempt, plan.MaxRetries) {
			return nil, mapped
		}
		if waitErr := Sleep(ctx, Backoff(attempt, 0)); waitErr != nil {
			// The caller gave up during the backoff rather than before it. Same
			// treatment: the retry still happened, the deadline still ended the call.
			return nil, gaveUp(mapped, waitErr, rpc)
		}
	}
}

// gaveUp records that the caller's context ended a call while a retryable failure
// was still outstanding.
//
// It keeps the type, code and reason the server sent and joins the context error
// into the chain, because a caller needs **both** answers and only one of them is
// obvious: "the node was unavailable" is the diagnosis, "you did not get an
// answer" is what they must act on, and returning either alone gets one of those
// two cases wrong. Joining them means one `errors.As(err, &unavailable)` and one
// `errors.Is(err, context.DeadlineExceeded)` both hold.
//
// The error is mutated in place rather than rebuilt, which is safe because
// `ToLoamsError` built it for this call and nothing else holds it.
func gaveUp(err, ctxErr error, rpc string) error {
	var loamsErr *LoamsError
	if !errors.As(err, &loamsErr) {
		return &LoamsError{
			Code:  CodeCanceled,
			RPC:   rpc,
			Cause: fmt.Errorf("%w (the caller gave up: %w)", err, ctxErr),
		}
	}
	loamsErr.Cause = fmt.Errorf("%w (the caller gave up before the retry: %w)", loamsErr.Cause, ctxErr)
	return err
}

// CallInvoker is the client's call path: a generated CallBinding plus a request
// becomes a response message, with everything the SDK owns applied on the way.
type CallInvoker struct {
	clients map[string]any
	source  TokenSource
	// maxRetries is the client's default; a call overrides it.
	maxRetries int
	// session is nil unless the client turned the session store on.
	session ConsistencyTokenStore
}

// NewCallInvoker builds an invoker over already-constructed Connect clients.
func NewCallInvoker(clients map[string]any, source TokenSource, maxRetries int, session ConsistencyTokenStore) *CallInvoker {
	return &CallInvoker{
		clients:    clients,
		source:     source,
		maxRetries: maxRetries,
		session:    session,
	}
}

// client returns the Connect client a binding names, or a clear internal error.
func (invoker *CallInvoker) client(binding facade.CallBinding) (any, error) {
	found, ok := invoker.clients[binding.Service]
	if !ok {
		return nil, newInternalError(binding.RPC, "no generated client for %s", binding.Service)
	}
	return found, nil
}

// bearer returns the `Authorization` header value for one attempt.
//
// The token is fetched **per attempt**, not per call, because R1's refresh has
// to change it between two attempts of the same logical call.
func (invoker *CallInvoker) bearer(ctx context.Context, extra http.Header) (http.Header, error) {
	headers := http.Header{}
	for name, values := range extra {
		for _, value := range values {
			headers.Add(name, value)
		}
	}
	if invoker.source == nil {
		return headers, nil
	}
	token, err := invoker.source.Token(ctx)
	if err != nil {
		return nil, err
	}
	if token != "" {
		// The header is set here and overwrites anything the caller put in it:
		// a caller-supplied bearer would be one the runtime cannot refresh, and
		// R1 would be unreachable.
		headers.Set("Authorization", "Bearer "+token)
	}
	return headers, nil
}

// sessionFor resolves the store a call asked for: its own if it brought one,
// the client's if it asked for the session's, none otherwise.
func (invoker *CallInvoker) sessionFor(settings *callSettings) ConsistencyTokenStore {
	if settings.consistency == nil || settings.consistency.Session == nil {
		return nil
	}
	return settings.consistency.Session
}

// plan builds the retry plan for one call, given whether the call ended up
// keyed.
func (invoker *CallInvoker) plan(binding facade.CallBinding, settings *callSettings, keyed bool) RetryPlan {
	maxRetries := invoker.maxRetries
	if settings.maxRetries != nil {
		maxRetries = *settings.maxRetries
	}
	if maxRetries < 0 {
		maxRetries = 0
	}
	// D610: a `safe` call always retries; a mutation retries once it carries an
	// idempotency key, which WithIdempotencyKey has just decided.
	retrySafe := binding.Retry == facade.RetrySafe || keyed
	if settings.retrySafe != nil {
		retrySafe = *settings.retrySafe
	}
	plan := RetryPlan{RetrySafe: retrySafe, MaxRetries: maxRetries}
	if invoker.source != nil {
		plan.OnRefresh = invoker.source.Refresh
	}
	return plan
}
