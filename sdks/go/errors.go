// The typed error hierarchy of design §44 §7.4, decision D611.
//
// A failed RPC carries a Connect code and one `loams.errors.v1.ErrorInfo` in
// its details. The `reason` is what callers branch on: a stable `snake_case`
// string, registered in `docs/api/reasons.md` and generated into
// `loams.dev/go/gen/facade`, so `errors.As(err, &notFound)` narrows the type and
// a reason the registry has lost stops compiling. The message is for people and
// may change; nothing in an SDK branches on it.
//
// In Go the branch is `errors.As`, not a `switch`. That is the whole reason the
// hierarchy exists:
//
//	var notFound *loams.NotFoundError
//	if errors.As(err, &notFound) {
//	    // the named resource does not exist
//	}
//
// and for a reason rather than a class:
//
//	if loams.ReasonOf(err) == loams.ReasonNotFound {
//	    // ...
//	}
//
// Three cases are distinct and are not conflated:
//
//   - a reason from a *newer* server, which this SDK's registry does not have:
//     it is surfaced as text in `UnknownReason` and flagged, not dropped;
//   - a failure from *below the API* — a socket, a refused connection, a
//     cancelled context — which carries no `reason` at all;
//   - a `*LoamsError`, which is returned unchanged if it is mapped twice, so
//     wrapping an SDK's own error never loses its reason.

package loams

import (
	"context"
	"errors"
	"fmt"

	connect "connectrpc.com/connect"

	"loams.dev/go/gen/facade"
)

// Code is the Connect code, the canonical classification of a failure. It is an
// alias rather than a new type so a caller can compare it with `connect.Code`
// values and with `loams.CodeNotFound` and friends without a conversion.
type Code = connect.Code

// The Connect codes an SDK branches on. Re-exported so a caller who does not
// otherwise import connect-go can name one.
const (
	CodeCanceled           = connect.CodeCanceled
	CodeUnknown            = connect.CodeUnknown
	CodeInvalidArgument    = connect.CodeInvalidArgument
	CodeDeadlineExceeded   = connect.CodeDeadlineExceeded
	CodeNotFound           = connect.CodeNotFound
	CodeAlreadyExists      = connect.CodeAlreadyExists
	CodePermissionDenied   = connect.CodePermissionDenied
	CodeResourceExhausted  = connect.CodeResourceExhausted
	CodeFailedPrecondition = connect.CodeFailedPrecondition
	CodeAborted            = connect.CodeAborted
	CodeOutOfRange         = connect.CodeOutOfRange
	CodeUnimplemented      = connect.CodeUnimplemented
	CodeInternal           = connect.CodeInternal
	CodeUnavailable        = connect.CodeUnavailable
	CodeDataLoss           = connect.CodeDataLoss
	CodeUnauthenticated    = connect.CodeUnauthenticated
)

// Reason is the stable cause of a failed RPC. See `loams.dev/go/gen/facade`.
type Reason = facade.Reason

// The reason constants, re-exported so a caller needs one import.
const (
	ReasonNotImplemented      = facade.ReasonNotImplemented
	ReasonFeatureNotInVariant = facade.ReasonFeatureNotInVariant
	ReasonTokenExpired        = facade.ReasonTokenExpired
	ReasonInvalidArgument     = facade.ReasonInvalidArgument
	ReasonNotFound            = facade.ReasonNotFound
	ReasonAlreadyExists       = facade.ReasonAlreadyExists
	ReasonPermissionDenied    = facade.ReasonPermissionDenied
	ReasonUnauthenticated     = facade.ReasonUnauthenticated
	ReasonFailedPrecondition  = facade.ReasonFailedPrecondition
	ReasonResourceExhausted   = facade.ReasonResourceExhausted
	ReasonUnavailable         = facade.ReasonUnavailable
	ReasonDeadlineExceeded    = facade.ReasonDeadlineExceeded
	ReasonAborted             = facade.ReasonAborted
	ReasonInternal            = facade.ReasonInternal
)

// Variants of the reasons above are in `loams.dev/go/gen/facade`; the two the
// SDK's own logic branches on are named here.

// ErrorInfoShape is the fields of `loams.errors.v1.ErrorInfo` this package
// reads. It is a shape rather than the generated type so a caller who wants the
// detail does not have to import the stub package.
type ErrorInfoShape struct {
	// Reason is the stable, machine-readable cause.
	Reason string
	// Metadata is structured context, for example {"variant": "standard"}. It
	// never carries secrets.
	Metadata map[string]string
	// Hint is a short next step in the caller's locale.
	Hint string
}

// LoamsError is what every Loams failure carries. The concrete types below are
// its subclasses; a failure whose code D611 does not name (`canceled`,
// `out_of_range`, `data_loss`) is a `*LoamsError` itself.
type LoamsError struct {
	// Code is the Connect code, the canonical classification. It does not
	// change within a major version.
	Code Code
	// Reason is the stable cause, when the server sent an `ErrorInfo`. Empty
	// means the failure came from below the API — a socket, a timeout, a
	// cancelled context — not from a Loams service.
	Reason Reason
	// UnknownReason is a reason off the wire that this SDK's registry does not
	// have, meaning the server is newer than the SDK. It is surfaced rather
	// than dropped: losing it would leave a caller unable to tell "not
	// supported here" from "not supported at all" (R8).
	UnknownReason string
	// Metadata is the structured context the server sent. Never secrets.
	Metadata map[string]string
	// Hint is a short next step in the caller's locale, when the server sent
	// one.
	Hint string
	// RPC is the RPC that failed, as `package.Service/Method`.
	RPC string
	// Cause is the failure below this one: the `*connect.Error`, a
	// `context.DeadlineExceeded`, or a socket error. It is named `Cause` rather
	// than `Err` because `Unwrap` is the method `errors.Is` and `errors.As` look
	// for, and a struct cannot have a field and a method with the same name.
	Cause error
}

// Error implements error. Every method on LoamsError takes a **value** receiver,
// which is what makes the embedding work: with a pointer receiver, only
// `*NotFoundError` would satisfy `error` and `errors.As` would need a pointer to
// a pointer for every subclass.
func (e LoamsError) Error() string {
	rpc := e.RPC
	if rpc != "" {
		rpc += ": "
	}
	reason := string(e.Reason)
	if reason == "" {
		reason = e.UnknownReason
	}
	if reason == "" {
		return rpc + e.Code.String() + ": " + e.message()
	}
	return rpc + e.Code.String() + " (" + reason + "): " + e.message()
}

func (e LoamsError) message() string {
	if e.Cause != nil {
		return e.Cause.Error()
	}
	return e.Code.String()
}

// Unwrap returns the failure below this one, so `errors.Is(err,
// context.DeadlineExceeded)` and `errors.As(err, &connectErr)` both work through
// a Loams error.
func (e LoamsError) Unwrap() error { return e.Cause }

// As lets a subclass answer for the base type.
//
// This is the one non-obvious thing about the hierarchy. `*NotFoundError`
// *embeds* `LoamsError`; it is not one, and `errors.As` does not look inside an
// embedded field. Without this method `errors.As(err, &base)` where base is a
// `*LoamsError` would skip every subclass, and `ReasonOf` and `CodeOf` — which
// both go through it — would return "" and `unknown` for every typed failure.
//
// The pointer receiver is load-bearing and is the opposite of the one on `Error`.
// `errors.As` hands the slot a pointer, and a *value* receiver would hand over a
// copy: every read would work, and every **write** through it — `gaveUp` joining
// a context error into the chain — would land on the copy and be lost. A pointer
// receiver is promoted onto `*NotFoundError` with the receiver pointing into the
// real error, which is what makes the slot a real reference.
//
// Both target shapes are handled because `var base LoamsError` and
// `var base *LoamsError` are both idiomatic Go.
func (e *LoamsError) As(target any) bool {
	switch slot := target.(type) {
	case *LoamsError:
		*slot = *e
		return true
	case **LoamsError:
		*slot = e
		return true
	default:
		return false
	}
}

// As lets a `TokenExpiredError` answer for its parent `UnauthenticatedError`,
// which the promoted `LoamsError.As` cannot: the two are separate concrete types
// and errors.As does not look inside an embedded field. Without this, a caller
// that branches on the class — `var unauthenticated *loams.UnauthenticatedError`
// — would miss every `token_expired` failure.
//
// Only the parent is offered. Filling an unrelated subclass from this error would
// let `errors.As` fabricate a type the failure is not, which is worse than not
// matching at all.
func (e *TokenExpiredError) As(target any) bool {
	if slot, ok := target.(**UnauthenticatedError); ok {
		*slot = &UnauthenticatedError{LoamsError: e.LoamsError}
		return true
	}
	return e.LoamsError.As(target)
}

// Is reports whether a target is a `*LoamsError` with the same reason and code.
// Two Loams errors that describe the same failure compare equal, which is what
// makes `errors.Is(err, loams.ErrNotFound)`-shaped checks work; the subclass
// types still need `errors.As` because that is what carries the extra fields.
func (e LoamsError) Is(target error) bool {
	var other *LoamsError
	if !errors.As(target, &other) {
		return false
	}
	return e.Code == other.Code && e.Reason == other.Reason && e.RPC == other.RPC
}

// The subclasses of LoamsError, one per Connect code D611 names. Each is a
// distinct type so `errors.As` is the branch; none adds a field of its own,
// because the branch is what a caller wants and the reason is on the base.
type (
	// InvalidArgumentError is `invalid_argument`.
	InvalidArgumentError struct{ LoamsError }
	// NotFoundError is `not_found`.
	NotFoundError struct{ LoamsError }
	// AlreadyExistsError is `already_exists`.
	AlreadyExistsError struct{ LoamsError }
	// PermissionDeniedError is `permission_denied`.
	PermissionDeniedError struct{ LoamsError }
	// UnauthenticatedError is `unauthenticated`.
	UnauthenticatedError struct{ LoamsError }
	// FailedPreconditionError is `failed_precondition`.
	FailedPreconditionError struct{ LoamsError }
	// ResourceExhaustedError is `resource_exhausted`.
	ResourceExhaustedError struct{ LoamsError }
	// UnavailableError is `unavailable`.
	UnavailableError struct{ LoamsError }
	// DeadlineExceededError is `deadline_exceeded`.
	DeadlineExceededError struct{ LoamsError }
	// AbortedError is `aborted`.
	AbortedError struct{ LoamsError }
	// InternalError is `internal`.
	InternalError struct{ LoamsError }
	// UnimplementedError is `unimplemented`.
	UnimplementedError struct{ LoamsError }
)

// FeatureNotInVariantError is a package this build variant does not carry
// (design §44 §4, D600).
//
// The server answers `unimplemented` with `reason = feature_not_in_variant` and
// names the variant in `metadata.variant`, which is what Variant reads. A caller
// usually never gets here: `loams.System().Guard` feature-detects from
// `GetInstance.Services[]` before calling, so an unavailable module raises this
// same type from the guard with no request spent. This type is the path for a
// caller who skipped the guard, or whose instance changed variant.
//
// One `errors.As` therefore covers "the guard said no" and "the server
// refused", which is the point of the guard raising the same type.
type FeatureNotInVariantError struct {
	UnimplementedError
	// Variant is the build variant that was asked for, from
	// `metadata.variant`. It is empty if the server did not send one.
	Variant string
}

// As lets a `FeatureNotInVariantError` answer for its parent `UnimplementedError`,
// for the same reason `TokenExpiredError.As` exists.
func (e *FeatureNotInVariantError) As(target any) bool {
	if slot, ok := target.(**UnimplementedError); ok {
		*slot = &UnimplementedError{LoamsError: e.UnimplementedError.LoamsError}
		return true
	}
	return e.UnimplementedError.LoamsError.As(target)
}

// TokenExpiredError is a token the server rejected as expired:
// `unauthenticated` with reason `token_expired`. The runtime refreshes once and
// retries once (D608, R1); a second expiry reaches the caller as this type.
type TokenExpiredError struct{ UnauthenticatedError }

// byCode is the code-to-class mapping of D611. Codes D611 does not name
// (`canceled`, `out_of_range`, `data_loss`) fall through to `*LoamsError`.
func byCode(code Code, rpc string, base LoamsError) error {
	switch code {
	case connect.CodeInvalidArgument:
		return &InvalidArgumentError{base}
	case connect.CodeNotFound:
		return &NotFoundError{base}
	case connect.CodeAlreadyExists:
		return &AlreadyExistsError{base}
	case connect.CodePermissionDenied:
		return &PermissionDeniedError{base}
	case connect.CodeUnauthenticated:
		return &UnauthenticatedError{base}
	case connect.CodeFailedPrecondition:
		return &FailedPreconditionError{base}
	case connect.CodeResourceExhausted:
		return &ResourceExhaustedError{base}
	case connect.CodeUnavailable:
		return &UnavailableError{base}
	case connect.CodeDeadlineExceeded:
		return &DeadlineExceededError{base}
	case connect.CodeAborted:
		return &AbortedError{base}
	case connect.CodeInternal:
		return &InternalError{base}
	case connect.CodeUnimplemented:
		return &UnimplementedError{base}
	default:
		base.RPC = rpc
		return &base
	}
}

// ErrorInfo returns the `ErrorInfo` detail a Connect error carries, if any.
//
// The detail is looked up **by its type**, not by position, so a service that
// adds a detail of its own does not move `reason` out from under a caller.
func ErrorInfo(err *connect.Error) *ErrorInfoShape {
	if err == nil {
		return nil
	}
	for _, detail := range err.Details() {
		// By type, never by position: a service that adds a detail of its own
		// must not move `reason` out from under a caller.
		if detail.Type() != facade.ErrorInfoTypeURL {
			continue
		}
		message, decodeErr := detail.Value()
		if decodeErr != nil {
			// The detail's bytes did not parse. Reported as "no ErrorInfo"
			// rather than dropped silently, because the alternative is a
			// Loams failure with no reason and no hint — the one thing R8 says
			// must not happen.
			continue
		}
		info, ok := message.(*facade.ErrorInfoType)
		if !ok {
			continue
		}
		return &ErrorInfoShape{
			Reason:   info.GetReason(),
			Metadata: info.GetMetadata(),
			Hint:     info.GetHint(),
		}
	}
	return nil
}

// ToLoamsError turns any error into the typed hierarchy.
//
// A `*connect.Error` becomes the class its code names, with `reason` and
// `metadata` lifted out of the `ErrorInfo` detail. Anything else — a socket
// error, a cancelled context, a bug in the SDK — becomes a `*LoamsError` with
// `CodeUnknown`, except that a context failure keeps its own code, because a
// deadline that expires is a deadline and hiding it behind `unknown` would lose
// the one thing the caller can act on.
//
// An error that is already a `*LoamsError` is returned unchanged, so mapping
// twice loses nothing.
func ToLoamsError(err error, rpc string) error {
	if err == nil {
		return nil
	}
	var already *LoamsError
	if errors.As(err, &already) {
		return err
	}
	var connectErr *connect.Error
	if !errors.As(err, &connectErr) {
		// A context failure keeps its own code, and goes through the same mapping
		// as a wire one, so a deadline is a `*DeadlineExceededError` whether it
		// came from the server or from the caller's own context. Hiding the second
		// behind `unknown` would lose the one failure a caller can always act on.
		code := connect.CodeUnknown
		switch {
		case errors.Is(err, context.DeadlineExceeded):
			code = connect.CodeDeadlineExceeded
		case errors.Is(err, context.Canceled):
			code = connect.CodeCanceled
		}
		return byCode(code, rpc, LoamsError{Code: code, RPC: rpc, Cause: err})
	}
	base := LoamsError{
		Code:  connectErr.Code(),
		RPC:   rpc,
		Cause: err,
	}
	if info := ErrorInfo(connectErr); info != nil {
		if reason, known := facade.KnownReason(info.Reason); known {
			base.Reason = reason
		} else if info.Reason != "" {
			base.UnknownReason = info.Reason
		}
		base.Metadata = info.Metadata
		if info.Hint != "" {
			base.Hint = info.Hint
		}
	}
	if base.Reason == ReasonFeatureNotInVariant {
		return &FeatureNotInVariantError{
			UnimplementedError: UnimplementedError{base},
			Variant:            base.Metadata["variant"],
		}
	}
	if base.Code == connect.CodeUnauthenticated && base.Reason == ReasonTokenExpired {
		return &TokenExpiredError{UnauthenticatedError{base}}
	}
	return byCode(base.Code, rpc, base)
}

// IsLoamsError reports whether an error came from Loams rather than from below
// the API. It is `errors.As` spelled out, for a caller who wants a boolean.
func IsLoamsError(err error) bool {
	var loamsErr *LoamsError
	return errors.As(err, &loamsErr)
}

// ReasonOf is the reason an error carries, or "" when it carries none: a
// failure from below the API has no reason, and that is a different thing from
// a Loams service refusing.
func ReasonOf(err error) Reason {
	var loamsErr *LoamsError
	if !errors.As(err, &loamsErr) {
		return ""
	}
	return loamsErr.Reason
}

// CodeOf is the Connect code an error carries, or `CodeUnknown`.
//
// A `*connect.Error` that has not been through `ToLoamsError` yet still has a
// code, and the retry policy asks for the code before the mapping happens — so
// falling back to `connect.CodeOf` is what lets the policy be a function rather
// than a method that only works after a call has failed.
func CodeOf(err error) Code {
	var loamsErr *LoamsError
	if errors.As(err, &loamsErr) {
		return loamsErr.Code
	}
	return connect.CodeOf(err)
}

// newInternalError is the SDK's own failures — a binding that does not resolve,
// a request whose shape it cannot read. They are `internal` because they are
// bugs in this package, not in the caller or the server.
func newInternalError(rpc string, format string, args ...any) error {
	return &LoamsError{
		Code:   connect.CodeInternal,
		RPC:    rpc,
		Reason: ReasonInternal,
		Cause:  fmt.Errorf(format, args...),
	}
}
