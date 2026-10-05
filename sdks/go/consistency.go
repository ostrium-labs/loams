// Consistency tokens (design §44 §7.4, D609; runtime contract R4).
//
// A write answers with a `consistency_token`; a read accepts one, so a caller
// that just wrote can read its own write. Threading those by hand is the
// caller's job today. The session store is the alternative §44 §7.4 asks for:
// **off by default**, and when a call opts in, every response's token is folded
// into the session and attached to later reads.
//
// **The token's encoding is not in the protos yet.** §05 §5 defines the
// semantics (offsets per stream and partition, `STRONG` as the default,
// `EVENTUAL`, `AT_LEAST{token}`) and API1's write paths carry it as an opaque
// `v1:` string; §44 §7.4 says it merges by "max offset per stream and
// partition", which needs the encoding to be parsed. Until that lands this store
// keeps the token it was given, refuses to merge two different tokens into a
// wrong one, and says so — a silently-wrong consistency token reads stale data,
// which is worse than an error.

package loams

import (
	"errors"
	"fmt"
	"reflect"
	"strings"
	"sync"

	connect "connectrpc.com/connect"
)

// ConsistencyTokenField is the generated Go field name of `consistency_token`.
const ConsistencyTokenField = "ConsistencyToken"

// TokenPrefix is the prefix every consistency token carries (§44 §7.4).
const TokenPrefix = "v1:"

// ConsistencyHeader is the request header a read's token travels in. §44 §7.4
// names the response header `loams-consistency-token`; the request side is the
// `consistency` field, and until a proto carries it the header is how the token
// gets there.
const ConsistencyHeader = "Loams-Consistency-Token"

// IsConsistencyToken whether a string looks like a consistency token.
func IsConsistencyToken(value string) bool {
	return strings.HasPrefix(value, TokenPrefix) && len(value) > len(TokenPrefix)
}

// ConsistencyTokenStore is a session's consistency token (D609).
//
// It is an interface so a caller who has real offset arithmetic to do can supply
// it, and so the session can be shared between two clients without either
// owning it.
type ConsistencyTokenStore interface {
	// Current is the token to attach to the next read, or "" for none.
	Current() string
	// Record folds a token the server returned into the session's.
	Record(token string) error
	// Conflicts is how many unmergeable pairs the session has seen. Surfaced so
	// the limitation in this file's header is visible rather than silent.
	Conflicts() int
}

// ConsistencySession is the default store: it keeps the token it was given and
// counts the ones it could not merge.
type ConsistencySession struct {
	mu        sync.Mutex
	token     string
	conflicts int
}

// NewConsistencySession is an empty session.
func NewConsistencySession() *ConsistencySession { return &ConsistencySession{} }

// Current is the token to attach to the next read.
func (s *ConsistencySession) Current() string {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.token
}

// Record folds a token in. An empty token is not a token.
func (s *ConsistencySession) Record(token string) error {
	if token == "" {
		return nil
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	if !IsConsistencyToken(token) {
		s.conflicts++
		return &LoamsError{
			Code:   connect.CodeInternal,
			Reason: ReasonInternal,
			Cause:  fmt.Errorf("not a consistency token: %s", token),
		}
	}
	if s.token == "" || s.token == token {
		s.token = token
		return nil
	}
	s.conflicts++
	return &LoamsError{
		Code:   connect.CodeFailedPrecondition,
		Reason: ReasonFailedPrecondition,
		Cause: errors.New(
			"two different consistency tokens met and the encoding cannot merge them yet; " +
				"the session keeps the first. Merging by stream and partition offset arrives with " +
				"the write paths that carry offsets (design §44 §7.4, D609)"),
	}
}

// Conflicts is how many unmergeable pairs the session has seen.
func (s *ConsistencySession) Conflicts() int {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.conflicts
}

// Clear forgets the token, so the next read is not held to it.
func (s *ConsistencySession) Clear() {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.token = ""
	s.conflicts = 0
}

// recordConsistency folds the token a response carried into a session.
//
// A failure here is **counted, not thrown**: the RPC already succeeded, and a
// caller that retries on that error performs the write twice. The count is the
// only honest report available until the encoding can be merged.
func recordConsistency(session ConsistencyTokenStore, response any) {
	if session == nil || response == nil {
		return
	}
	_ = session.Record(consistencyTokenOf(response))
}

// consistencyTokenOf reads the `consistency_token` off a response, or "".
//
// **No RPC carries one yet** (`docs/sdk/runtime-contract.md` R4: "Not pinned by
// a test yet, because no RPC carries a token"), so this reads a field that no
// generated message has. It is here because R4's session store is worth having
// the moment a write path lands, and reading the field by name rather than
// regenerating the invoker per message is what lets both be true at once.
func consistencyTokenOf(response any) string {
	holder := reflect.ValueOf(response)
	for holder.Kind() == reflect.Pointer {
		if holder.IsNil() {
			return ""
		}
		holder = holder.Elem()
	}
	if !holder.IsValid() || holder.Kind() != reflect.Struct {
		return ""
	}
	field := holder.FieldByName(ConsistencyTokenField)
	if !field.IsValid() || !field.CanInterface() {
		return ""
	}
	if getter, ok := response.(interface{ GetConsistencyToken() string }); ok {
		return getter.GetConsistencyToken()
	}
	if field.Kind() == reflect.String {
		return field.String()
	}
	return ""
}
