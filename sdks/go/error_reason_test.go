// SDK2 Task 2's `go_error_reason_mapping`.
//
// Design §44 §7.4, D611 and runtime contract R8: a failed RPC carries a Connect
// code and one `loams.errors.v1.ErrorInfo`, and **`reason` is what callers
// branch on**. The message is for people and may change.
//
// In Go the branch is `errors.As`, so this pins:
//
//   - every reason in the registry reaches the SDK as a **type** a caller can
//     assert on, not a string comparison, and each one is raised under the code
//     the registry says;
//   - the type comes from the code, so a caller can also branch on the coarse
//     category (`*NotFoundError` for `not_found`);
//   - a reason from a *newer* server, which this SDK's registry does not have,
//     is surfaced rather than dropped — that is what `UnknownReason` is for;
//   - a failure from below the API — a socket, a cancelled context, a deadline —
//     carries no `reason` at all, which is a different thing from a Loams
//     service refusing.
//
// The corpus in `sdks/fixtures` covers the two reasons a `loams dev` produces
// (`not_implemented` and `feature_not_in_variant`); this covers all
// twenty-six.

package loams

import (
	"context"
	"errors"
	"net"
	"os"
	"strings"
	"testing"

	connect "connectrpc.com/connect"

	"google.golang.org/protobuf/types/known/wrapperspb"
	"loams.dev/go/gen/facade"
	errorsv1 "loams.dev/go/gen/loams/errors/v1"
)

// TestGoErrorReasonMapping is the required test.
func TestGoErrorReasonMapping(t *testing.T) {
	t.Run(ErrorReasonMapping, func(t *testing.T) {
		testErrorReasonMapping(t)
	})
}

func testErrorReasonMapping(t *testing.T) {
	t.Run("every reason in the registry maps to a code the SDK can branch on", func(t *testing.T) {
		if len(facade.Reasons) == 0 {
			t.Fatal("the generated reason registry is empty")
		}
		for _, reason := range facade.Reasons {
			codeName, ok := facade.ReasonCodes[reason]
			if !ok {
				t.Errorf("the registry has %q with no code", reason)
				continue
			}
			if _, known := facade.KnownReason(string(reason)); !known {
				t.Errorf("KnownReason(%q) says no, but it is in the registry", reason)
			}
			mapped := ToLoamsError(newWireError(t, codeName, string(reason), nil, ""), "test.RPC")
			if got := ReasonOf(mapped); got != reason {
				t.Errorf("a %s error came back as reason %q, want %q", codeName, got, reason)
			}
			if got := CodeOf(mapped); got.String() != codeName {
				t.Errorf("reason %q came back as code %q, want %q", reason, got, codeName)
			}
			if !IsLoamsError(mapped) {
				t.Errorf("reason %q did not produce a *LoamsError", reason)
			}
		}
	})

	t.Run("the type comes from the code, so the coarse branch works too", func(t *testing.T) {
		// Each case is a closure rather than a `*T` value because Go's idiom is
		// `var target *T; errors.As(err, &target)` — a pointer to a pointer — and a
		// `[]struct{ want any }` cannot express that without the assertion panicking.
		cases := []struct {
			code Code
			name string
			as   func(error) bool
		}{
			{CodeInvalidArgument, "*InvalidArgumentError", func(err error) bool {
				var target *InvalidArgumentError
				return errors.As(err, &target)
			}},
			{CodeNotFound, "*NotFoundError", func(err error) bool {
				var target *NotFoundError
				return errors.As(err, &target)
			}},
			{CodeAlreadyExists, "*AlreadyExistsError", func(err error) bool {
				var target *AlreadyExistsError
				return errors.As(err, &target)
			}},
			{CodePermissionDenied, "*PermissionDeniedError", func(err error) bool {
				var target *PermissionDeniedError
				return errors.As(err, &target)
			}},
			{CodeFailedPrecondition, "*FailedPreconditionError", func(err error) bool {
				var target *FailedPreconditionError
				return errors.As(err, &target)
			}},
			{CodeResourceExhausted, "*ResourceExhaustedError", func(err error) bool {
				var target *ResourceExhaustedError
				return errors.As(err, &target)
			}},
			{CodeUnavailable, "*UnavailableError", func(err error) bool {
				var target *UnavailableError
				return errors.As(err, &target)
			}},
			{CodeDeadlineExceeded, "*DeadlineExceededError", func(err error) bool {
				var target *DeadlineExceededError
				return errors.As(err, &target)
			}},
			{CodeAborted, "*AbortedError", func(err error) bool {
				var target *AbortedError
				return errors.As(err, &target)
			}},
			{CodeInternal, "*InternalError", func(err error) bool {
				var target *InternalError
				return errors.As(err, &target)
			}},
			{CodeUnimplemented, "*UnimplementedError", func(err error) bool {
				var target *UnimplementedError
				return errors.As(err, &target)
			}},
			{CodeUnauthenticated, "*UnauthenticatedError", func(err error) bool {
				var target *UnauthenticatedError
				return errors.As(err, &target)
			}},
		}
		for _, entry := range cases {
			// No ErrorInfo at all: this is the "a failure from below the API" case,
			// or a service that refused without naming a reason. The class must
			// still be right.
			mapped := ToLoamsError(connect.NewError(entry.code, errors.New("no reason here")), "test.RPC")
			if got := ReasonOf(mapped); got != "" {
				t.Errorf("%s produced reason %q, want none", entry.code, got)
			}
			if !entry.as(mapped) {
				t.Errorf("%s did not produce a %s", entry.code, entry.name)
			}
		}
		// Codes D611 does not name fall through to the base type rather than to
		// something arbitrary.
		for _, code := range []Code{CodeCanceled, CodeOutOfRange, CodeDataLoss, CodeUnknown} {
			mapped := ToLoamsError(connect.NewError(code, errors.New("unnamed")), "test.RPC")
			if _, ok := mapped.(*LoamsError); !ok {
				t.Errorf("%s produced %T, want a plain *LoamsError", code, mapped)
			}
		}
	})

	t.Run("a reason this SDK does not have is surfaced, not dropped", func(t *testing.T) {
		// A server newer than the SDK may return a reason this registry has
		// never heard of. Dropping it would leave a caller unable to tell "not
		// supported here" from "not supported at all" (R8).
		const future = "quantum_entanglement_required"
		if facade.IsKnownReason(future) {
			t.Fatalf("%q is already in the registry, so this test proves nothing", future)
		}
		mapped := ToLoamsError(newWireError(t, "failed_precondition", future, nil, "try again later"), "test.RPC")
		if ReasonOf(mapped) != "" {
			t.Errorf("an unknown reason became the typed %q", ReasonOf(mapped))
		}
		var base *LoamsError
		if !errors.As(mapped, &base) {
			t.Fatalf("an unknown reason produced %T, want a *LoamsError", mapped)
		}
		if base.UnknownReason != future {
			t.Errorf("UnknownReason is %q, want %q", base.UnknownReason, future)
		}
		if base.Hint != "try again later" {
			t.Errorf("the hint was dropped: %q", base.Hint)
		}
		// And it is still in the message, so a log line says what happened.
		if !containsSubstring(base.Error(), future) {
			t.Errorf("the error text %q does not mention %q", base.Error(), future)
		}
	})

	t.Run("the two special reasons become their own types", func(t *testing.T) {
		expired := ToLoamsError(newWireError(t, "unauthenticated", string(ReasonTokenExpired), nil, ""), "test.RPC")
		var tokenExpired *TokenExpiredError
		if !errors.As(expired, &tokenExpired) {
			t.Errorf("a token_expired error produced %T, want a *TokenExpiredError", expired)
		}
		// It is also an UnauthenticatedError, so a caller that does not care about
		// the reason still has a class.
		var unauthenticated *UnauthenticatedError
		if !errors.As(expired, &unauthenticated) {
			t.Errorf("a token_expired error is not an *UnauthenticatedError")
		}

		absent := ToLoamsError(newWireError(t, "unimplemented", string(ReasonFeatureNotInVariant),
			map[string]string{"variant": "standard", "package": "loams.live.v1"}, ""), "test.RPC")
		var notInVariant *FeatureNotInVariantError
		if !errors.As(absent, &notInVariant) {
			t.Fatalf("a feature_not_in_variant error produced %T, want a *FeatureNotInVariantError", absent)
		}
		if notInVariant.Variant != "standard" {
			t.Errorf("the variant is %q, want %q read out of metadata rather than the message", notInVariant.Variant, "standard")
		}
		if notInVariant.Metadata["package"] != "loams.live.v1" {
			t.Errorf("the metadata was dropped: %v", notInVariant.Metadata)
		}
		var unimplemented *UnimplementedError
		if !errors.As(absent, &unimplemented) {
			t.Error("a feature_not_in_variant error is not an *UnimplementedError")
		}
	})

	t.Run("a detail of the server's own does not move reason out from under the caller", func(t *testing.T) {
		// The detail is looked up by type, never by position: a service that puts a
		// detail of its own in front of the ErrorInfo must not break every caller.
		wire := connect.NewError(connect.CodeNotFound, errors.New("the named resource does not exist"))
		foreign, err := connect.NewErrorDetail(wrapperspb.String("a detail of the server's own"))
		if err != nil {
			t.Fatalf("building a detail: %v", err)
		}
		info, err := connect.NewErrorDetail(&errorsv1.ErrorInfo{
			Reason:   string(ReasonNotFound),
			Metadata: map[string]string{"namespace": "acme"},
		})
		if err != nil {
			t.Fatalf("building a detail: %v", err)
		}
		wire.AddDetail(foreign)
		wire.AddDetail(info)

		found := ErrorInfo(wire)
		if found == nil {
			t.Fatal("no ErrorInfo was found among two details")
		}
		if found.Reason != string(ReasonNotFound) {
			t.Errorf("the reason is %q, want %q: the lookup did not skip the other detail", found.Reason, ReasonNotFound)
		}
		if found.Metadata["namespace"] != "acme" {
			t.Errorf("the metadata was lost: %v", found.Metadata)
		}
		// And the whole thing maps: the foreign detail did not confuse it.
		if got := ReasonOf(ToLoamsError(wire, "test.RPC")); got != ReasonNotFound {
			t.Errorf("the mapped reason is %q, want %q", got, ReasonNotFound)
		}
		// A failure with no ErrorInfo at all yields no reason rather than a guess.
		if got := ErrorInfo(connect.NewError(connect.CodeInternal, errors.New("x"))); got != nil {
			t.Errorf("a failure with no details produced %+v, want nil", got)
		}
	})

	t.Run("a failure from below the API carries no reason", func(t *testing.T) {
		// R8's third case: a socket, an abort, a timeout. These are different
		// from a Loams service refusing, and conflating them would make
		// `ReasonOf(err) == ""` ambiguous.
		cases := []struct {
			name string
			err  error
			code Code
		}{
			{"a socket", &net.OpError{Op: "dial", Err: errors.New("connection refused")}, CodeUnknown},
			{"a cancelled context", context.Canceled, CodeCanceled},
			{"a deadline", context.DeadlineExceeded, CodeDeadlineExceeded},
			{"something else entirely", errors.New("a bug in the caller"), CodeUnknown},
		}
		for _, entry := range cases {
			mapped := ToLoamsError(entry.err, "test.RPC")
			if got := ReasonOf(mapped); got != "" {
				t.Errorf("%s produced reason %q, want none", entry.name, got)
			}
			if got := CodeOf(mapped); got != entry.code {
				t.Errorf("%s produced code %q, want %q", entry.name, got, entry.code)
			}
			if !errors.Is(mapped, entry.err) {
				t.Errorf("%s: the mapped error does not unwrap to the original", entry.name)
			}
		}
		// The deadline is the one a caller can always act on, so it keeps its own
		// code rather than being hidden behind `unknown`.
		mapped := ToLoamsError(context.DeadlineExceeded, "test.RPC")
		var deadline *DeadlineExceededError
		if !errors.As(mapped, &deadline) {
			t.Errorf("a deadline produced %T, want a *DeadlineExceededError", mapped)
		}
	})

	t.Run("mapping an error twice loses nothing", func(t *testing.T) {
		// R8's third bullet: a LoamsError is returned unchanged if it is mapped
		// twice, so wrapping a call in an extra layer of error handling does not
		// strip the reason a caller needs.
		first := ToLoamsError(newWireError(t, "not_found", string(ReasonNotFound), nil, ""), "test.RPC")
		second := ToLoamsError(first, "another.RPC")
		if second != error(first) {
			t.Error("a second mapping built a new error rather than returning the same one")
		}
		if ReasonOf(second) != ReasonNotFound {
			t.Errorf("the reason became %q after a second mapping", ReasonOf(second))
		}
	})

	t.Run("the reason registry matches docs/api/reasons.md", func(t *testing.T) {
		// The generated list is a transcription, so nothing stops it drifting
		// except a test. The registry's own rule is that a reason may be added
		// but never renamed or removed within a major version, so a name that
		// disappears here is a wire break.
		documented := readReasonRegistry(t)
		for _, reason := range facade.Reasons {
			code, ok := documented[string(reason)]
			if !ok {
				t.Errorf("the SDK registry has %q, which docs/api/reasons.md does not", reason)
				continue
			}
			if got := facade.ReasonCodes[reason]; got != code {
				t.Errorf("reason %q: the SDK says it is raised under %q, the registry says %q", reason, got, code)
			}
		}
		for reason := range documented {
			if !facade.IsKnownReason(facade.Reason(reason)) {
				t.Errorf("docs/api/reasons.md has %q, which the SDK registry does not", reason)
			}
		}
	})
}

// newWireError builds the `*connect.Error` the transport would build for a
// refusal carrying an ErrorInfo: a Connect error whose detail is the marshalled
// ErrorInfo.
func newWireError(t *testing.T, code, reason string, metadata map[string]string, hint string) error {
	t.Helper()
	info := &errorsv1.ErrorInfo{Reason: reason, Metadata: metadata, Hint: hint}
	detail, err := connect.NewErrorDetail(info)
	if err != nil {
		t.Fatalf("building the ErrorInfo detail: %v", err)
	}
	wire := connect.NewError(connectCodeFor(t, code), errors.New("the server said no"))
	wire.AddDetail(detail)
	return wire
}

// connectCodeFor turns a registry code name into a `connect.Code`, failing the
// test if the name is not a code — which is itself worth catching, since the
// registry is the SDK's source of truth.
func connectCodeFor(t *testing.T, name string) connect.Code {
	t.Helper()
	var code connect.Code
	if err := code.UnmarshalText([]byte(name)); err != nil {
		t.Fatalf("docs/api/reasons.md names %q, which is not a Connect code: %v", name, err)
	}
	return code
}

// readReasonRegistry parses the `| reason | code | ... |` table out of
// `docs/api/reasons.md`.
//
// The page is Markdown rather than a machine-readable file, so this is a small
// parser rather than a dependency: the registry is one table, the format is
// fixed by the page's own rules, and a JSON sidecar would be a second thing to
// keep in step with the first.
func readReasonRegistry(t *testing.T) map[string]string {
	t.Helper()
	payload, err := os.ReadFile("../../docs/api/reasons.md")
	if err != nil {
		t.Fatalf("reading docs/api/reasons.md: %v", err)
	}
	registry := map[string]string{}
	inTable := false
	for line := range strings.SplitSeq(string(payload), "\n") {
		trimmed := strings.TrimSpace(line)
		if !strings.HasPrefix(trimmed, "|") {
			inTable = false
			continue
		}
		cells := strings.Split(strings.TrimPrefix(trimmed, "|"), "|")
		if len(cells) < 2 {
			continue
		}
		// The page's table cells are backticked (`` `approval_expired` ``), so a
		// cell is trimmed of Markdown before it is compared with a wire value.
		key := strings.Trim(strings.TrimSpace(cells[0]), "`")
		if key == "reason" {
			inTable = true
			continue
		}
		if !inTable || strings.HasPrefix(key, "-") {
			continue
		}
		registry[key] = strings.Trim(strings.TrimSpace(cells[1]), "`")
	}
	if len(registry) == 0 {
		t.Fatal("docs/api/reasons.md has no reason rows, or its table layout changed")
	}
	return registry
}

func containsSubstring(haystack, needle string) bool {
	return strings.Contains(haystack, needle)
}
