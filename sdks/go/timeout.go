// Deadlines and the default namespace (runtime contract R11 and R12, SDKP §2.1).
//
// **R12.** Every unary call runs under one budget, retries and backoff
// included: the client's `Options.Timeout` (30 s when unset), or the call's
// `WithTimeout`. The budget is a context deadline, so connect-go sends it as
// `connect-timeout-ms` (or `grpc-timeout`) and the server can stop early too.
// When it runs out the call fails with a `*DeadlineExceededError` whose reason
// is `client_deadline_exceeded`, and it is never retried, because the budget is
// spent. A deadline the caller's own context set is the caller's and keeps the
// existing mapping; a server's `deadline_exceeded` is the server's and may be
// retried. Streams get no default deadline.
//
// **R11.** The client's `Options.Namespace` fills the `namespace` field of a
// request that has one and leaves it empty. An explicit namespace always wins;
// with neither, the call fails with `*InvalidArgumentError` before any network
// call. This applies to the app data-plane packages (`NamespacedPackages`)
// only: elsewhere a `namespace` field means something else —
// `loams.live.v1.CreateAppRequest.namespace` is an optional free-text org and
// environment — and is left exactly as the caller wrote it.

package loams

import (
	"context"
	"errors"
	"strconv"
	"time"

	connect "connectrpc.com/connect"
	"google.golang.org/protobuf/proto"
	"google.golang.org/protobuf/reflect/protoreflect"

	"loams.dev/go/gen/facade"
)

// DefaultTimeout is the unary deadline when neither the client nor the call
// sets one (R12).
const DefaultTimeout = 30 * time.Second

// The reasons the SDK raises itself, without a server answer (R4, R11–R13,
// SDKP §2.4), registered in `docs/api/reasons.md` as client-side (SDKP P0e).
const (
	ReasonClientDeadlineExceeded = facade.ReasonClientDeadlineExceeded
	ReasonBatchPartiallyApplied  = facade.ReasonBatchPartiallyApplied
)

// ClientReasons is every reason the SDK raises itself.
var ClientReasons = []Reason{
	ReasonClientDeadlineExceeded,
	ReasonBatchPartiallyApplied,
	ReasonNamespaceRequired,
	ReasonInvalidConsistencyToken,
	ReasonSearchAfterNeedsTiebreak,
}

// NamespacedPackages are the packages whose `namespace` field is a Loams
// namespace, and so the packages R11 fills and checks.
var NamespacedPackages = map[string]bool{
	"loams.collection.v1": true,
	"loams.graph.v1":      true,
	"loams.sql.v1":        true,
	"loams.stream.v1":     true,
}

// errClientDeadline is the cause of a context the SDK's own timer ended.
var errClientDeadline = errors.New("loams: the client's deadline was exceeded")

// InvokerDefaults are what the invoker applies to every call (R11, R12).
type InvokerDefaults struct {
	// Namespace fills a request's empty `namespace` field.
	Namespace string
	// Timeout is the unary budget. Zero or negative means none.
	Timeout time.Duration
}

// SetDefaults sets the invoker's per-call defaults. `New` sets them from
// `Options`; a caller who builds an invoker by hand sets them here.
func (invoker *CallInvoker) SetDefaults(defaults InvokerDefaults) {
	invoker.defaults = defaults
}

// WithTimeout is this call's budget, retries included (R12). Zero or negative
// means no deadline for this call.
func WithTimeout(timeout time.Duration) CallOption {
	return func(settings *callSettings) { settings.timeout = &timeout }
}

// timeoutFor is a unary call's budget, or zero for none.
func (invoker *CallInvoker) timeoutFor(settings *callSettings) time.Duration {
	if settings.timeout != nil {
		return *settings.timeout
	}
	return invoker.defaults.Timeout
}

// withBudget derives the call's context. The cancel func is always non-nil.
func withBudget(ctx context.Context, timeout time.Duration) (context.Context, context.CancelFunc) {
	if timeout <= 0 {
		return ctx, func() {}
	}
	return context.WithTimeoutCause(ctx, timeout, errClientDeadline)
}

// clientDeadline reports whether a failure is the SDK's own timer running out,
// rather than the caller's context or the server, and if so builds R12's error.
func clientDeadline(parent, call context.Context, rpc string, timeout time.Duration, err error) (error, bool) {
	if timeout <= 0 || parent.Err() != nil || !errors.Is(context.Cause(call), errClientDeadline) {
		return nil, false
	}
	return &DeadlineExceededError{LoamsError{
		Code:     connect.CodeDeadlineExceeded,
		Reason:   ReasonClientDeadlineExceeded,
		RPC:      rpc,
		Metadata: map[string]string{"timeout_ms": strconv.FormatInt(timeout.Milliseconds(), 10)},
		Cause:    errors.Join(errClientDeadline, err),
	}}, true
}

// prepareRequest is the request as it goes out: the default namespace filled
// in (R11) and `token` set as `consistency.at_least` when the request has that
// field and the caller left `consistency` unset (R4). The caller's message is
// never mutated: a change goes into a clone. `inField` says the token travels
// in the message, so it is not sent as a header as well.
func prepareRequest[Req any](binding facade.CallBinding, request *Req, namespace, token string) (prepared *Req, inField bool, err error) {
	message, ok := any(request).(proto.Message)
	if !ok || request == nil {
		return request, false, nil
	}
	reflected := message.ProtoReflect()
	var clone proto.Message
	mutable := func() protoreflect.Message {
		if clone == nil {
			clone = proto.Clone(message)
		}
		return clone.ProtoReflect()
	}

	if field := reflected.Descriptor().Fields().ByName("namespace"); NamespacedPackages[binding.Package] && field != nil &&
		field.Kind() == protoreflect.StringKind && !field.IsList() && field.ContainingOneof() == nil {
		if reflected.Get(field).String() == "" {
			if namespace == "" {
				return nil, false, &InvalidArgumentError{LoamsError{
					Code:     connect.CodeInvalidArgument,
					Reason:   ReasonNamespaceRequired,
					RPC:      binding.RPC,
					Metadata: map[string]string{"field": "namespace"},
					Cause: errors.New(binding.RPC +
						" needs a namespace: pass one, or give the client a default (Options.Namespace)"),
				}}
			}
			mutable().Set(field, protoreflect.ValueOfString(namespace))
		}
	}

	if field, atLeast := consistencyField(reflected); field != nil {
		inField = true
		if token != "" && !reflected.Has(field) {
			mutable().Mutable(field).Message().Set(atLeast, protoreflect.ValueOfString(token))
		}
	}

	if clone == nil {
		return request, inField, nil
	}
	typed, ok := any(clone).(*Req)
	if !ok {
		return nil, false, newInternalError(binding.RPC, "a cloned %T is a %T", request, clone)
	}
	return typed, inField, nil
}
