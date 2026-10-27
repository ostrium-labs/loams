// Consistency tokens (design §44 §7.4, D609; runtime contract R4 as amended by
// SDKP §2.1).
//
// A write answers with a token; a read accepts one, so a caller that just wrote
// can read its own write. The encoding is the server's
// (`crates/loams-collection/src/token.rs`):
//
//	v1:s<stream>/p<partition>@<offset>,s<stream>/p<partition>@<offset>,…
//
// Each item is "the next offset after the write" for one `(stream, partition)`.
// Merging two tokens keeps, per pair, the larger offset, so a read held to the
// merge waits for both writes. Every number is canonical decimal (no sign, no
// leading zeros), so a token has exactly one spelling, and a malformed token is
// an `*InvalidArgumentError` here, before any network call. Streams and offsets
// are `uint64` and partitions `uint32`, exactly as the server holds them.
//
// The session store is **off by default**. With `Options.SessionConsistency`
// every token a response carries — the `consistency_token`, `token` and
// `read_token` fields and the `loams-consistency-token` header — is merged into
// it, and every read whose request has a `consistency` field is sent
// `at_least = session` when the caller left the field unset.
// `WithoutSessionConsistency()` opts one call out.

package loams

import (
	"fmt"
	"reflect"
	"sort"
	"strconv"
	"strings"
	"sync"

	connect "connectrpc.com/connect"
	"google.golang.org/protobuf/proto"
	"google.golang.org/protobuf/reflect/protoreflect"
)

// ConsistencyTokenField is the generated Go field name of `consistency_token`.
const ConsistencyTokenField = "ConsistencyToken"

// TokenPrefix is the prefix every consistency token carries (§44 §7.4).
const TokenPrefix = "v1:"

// ConsistencyHeader is the header a token travels in: the response header a
// write answers with, and the request header a read whose message has no
// `consistency` field carries it in.
const ConsistencyHeader = "Loams-Consistency-Token"

// IsConsistencyToken reports whether a string carries the token prefix. It
// does not parse it; ParseConsistencyToken does.
func IsConsistencyToken(value string) bool {
	return strings.HasPrefix(value, TokenPrefix)
}

// TokenPosition is one item of a token: the next offset after a write, for one
// partition of one stream.
type TokenPosition struct {
	Stream    uint64
	Partition uint32
	Offset    uint64
}

// ConsistencyToken is a parsed token. The zero value is the empty token, `v1:`.
// It is a value: Merge and Normalized return new tokens.
type ConsistencyToken struct {
	positions []TokenPosition
}

// NewConsistencyToken is a token from its items, in the order given.
func NewConsistencyToken(positions ...TokenPosition) ConsistencyToken {
	return ConsistencyToken{positions: append([]TokenPosition(nil), positions...)}
}

// canonical parses a decimal with no sign and no leading zeros (except "0"),
// no larger than bits allow.
func canonical(text string, bits int) (uint64, bool) {
	if text == "" || (len(text) > 1 && text[0] == '0') {
		return 0, false
	}
	for _, c := range text {
		if c < '0' || c > '9' {
			return 0, false
		}
	}
	value, err := strconv.ParseUint(text, 10, bits)
	return value, err == nil
}

// parseItem reads `s<stream>/p<partition>@<offset>`.
func parseItem(item string) (TokenPosition, bool) {
	rest, ok := strings.CutPrefix(item, "s")
	if !ok {
		return TokenPosition{}, false
	}
	stream, rest, ok := strings.Cut(rest, "/p")
	if !ok {
		return TokenPosition{}, false
	}
	partition, offset, ok := strings.Cut(rest, "@")
	if !ok {
		return TokenPosition{}, false
	}
	s, okS := canonical(stream, 64)
	p, okP := canonical(partition, 32)
	o, okO := canonical(offset, 64)
	if !okS || !okP || !okO {
		return TokenPosition{}, false
	}
	return TokenPosition{Stream: s, Partition: uint32(p), Offset: o}, true
}

// ParseConsistencyToken parses the text form. A malformed token is an
// `*InvalidArgumentError`; the items are kept in the order given, as the
// server's parser keeps them.
func ParseConsistencyToken(text string) (ConsistencyToken, error) {
	invalid := func() error {
		return &InvalidArgumentError{LoamsError{
			Code:     connect.CodeInvalidArgument,
			Reason:   ReasonInvalidConsistencyToken,
			Metadata: map[string]string{"field": "consistency_token"},
			Cause:    fmt.Errorf("invalid consistency token %q", text),
		}}
	}
	body, ok := strings.CutPrefix(text, TokenPrefix)
	if !ok {
		return ConsistencyToken{}, invalid()
	}
	if body == "" {
		return ConsistencyToken{}, nil
	}
	items := strings.Split(body, ",")
	positions := make([]TokenPosition, 0, len(items))
	for _, item := range items {
		position, ok := parseItem(item)
		if !ok {
			return ConsistencyToken{}, invalid()
		}
		positions = append(positions, position)
	}
	return ConsistencyToken{positions: positions}, nil
}

// Positions is the token's items, in its order.
func (t ConsistencyToken) Positions() []TokenPosition {
	return append([]TokenPosition(nil), t.positions...)
}

// IsEmpty reports whether the token names nothing.
func (t ConsistencyToken) IsEmpty() bool { return len(t.positions) == 0 }

// Normalized is the token sorted by (stream, partition), one item per pair,
// the max offset kept.
func (t ConsistencyToken) Normalized() ConsistencyToken {
	sorted := append([]TokenPosition(nil), t.positions...)
	sort.Slice(sorted, func(i, j int) bool {
		a, b := sorted[i], sorted[j]
		if a.Stream != b.Stream {
			return a.Stream < b.Stream
		}
		if a.Partition != b.Partition {
			return a.Partition < b.Partition
		}
		return a.Offset < b.Offset
	})
	out := sorted[:0]
	for _, item := range sorted {
		if n := len(out); n > 0 && out[n-1].Stream == item.Stream && out[n-1].Partition == item.Partition {
			out[n-1] = item
			continue
		}
		out = append(out, item)
	}
	return ConsistencyToken{positions: out}
}

// Merge is both tokens' offsets, a pair both name keeping the larger,
// normalised — what the server's `ConsistencyToken::merge` produces.
func (t ConsistencyToken) Merge(other ConsistencyToken) ConsistencyToken {
	joined := make([]TokenPosition, 0, len(t.positions)+len(other.positions))
	joined = append(joined, t.positions...)
	joined = append(joined, other.positions...)
	return ConsistencyToken{positions: joined}.Normalized()
}

// Offset is the offset for (stream, partition), the max if listed twice.
func (t ConsistencyToken) Offset(stream uint64, partition uint32) (uint64, bool) {
	var found uint64
	ok := false
	for _, item := range t.positions {
		if item.Stream == stream && item.Partition == partition && (!ok || item.Offset > found) {
			found, ok = item.Offset, true
		}
	}
	return found, ok
}

// Equal reports whether two tokens name the same positions once normalised.
func (t ConsistencyToken) Equal(other ConsistencyToken) bool {
	return t.Normalized().String() == other.Normalized().String()
}

// String is the text form, `v1:` and the items joined by `,`, in this token's
// order.
func (t ConsistencyToken) String() string {
	var b strings.Builder
	b.WriteString(TokenPrefix)
	for i, item := range t.positions {
		if i > 0 {
			b.WriteByte(',')
		}
		fmt.Fprintf(&b, "s%d/p%d@%d", item.Stream, item.Partition, item.Offset)
	}
	return b.String()
}

// ConsistencyTokenStore is a session's consistency token (D609).
//
// It is an interface so the session can be shared between two clients without
// either owning it, and so a caller can supply a store of their own.
type ConsistencyTokenStore interface {
	// Current is the token to attach to the next read, or "" for none.
	Current() string
	// Record folds a token the server returned into the session's.
	Record(token string) error
	// Conflicts is how many tokens the session could not parse.
	Conflicts() int
}

// ScopedTokenStore is a store that keeps one token per scope, a proto package
// (R4, SDKP-17). The call path reads and records in the call's own package
// when the store offers it, and through ConsistencyTokenStore when it does not.
type ScopedTokenStore interface {
	// CurrentIn is the scope's token, or "".
	CurrentIn(scope string) string
	// RecordIn folds a token into the scope's.
	RecordIn(token, scope string) error
}

// ConsistencySession is the default store: one merged token per scope (a
// proto package). A Graph read refuses a token naming another package's
// stream, so a collection write's token must never become a graph read's
// `at_least`; Current and Record are the default scope, "".
type ConsistencySession struct {
	mu       sync.Mutex
	tokens   map[string]ConsistencyToken
	rejected int
}

// NewConsistencySession is an empty session.
func NewConsistencySession() *ConsistencySession { return &ConsistencySession{} }

// Current is the default scope's merged token, or "" before anything was
// recorded there.
func (s *ConsistencySession) Current() string { return s.CurrentIn("") }

// CurrentIn is the scope's merged token, or "".
func (s *ConsistencySession) CurrentIn(scope string) string {
	s.mu.Lock()
	defer s.mu.Unlock()
	token, ok := s.tokens[scope]
	if !ok {
		return ""
	}
	return token.String()
}

// Token is the default scope's merged token, parsed, and whether there is one.
func (s *ConsistencySession) Token() (ConsistencyToken, bool) {
	s.mu.Lock()
	defer s.mu.Unlock()
	token, ok := s.tokens[""]
	return token, ok
}

// Record merges a token into the default scope. An empty token is not a token;
// a malformed one is counted and returned as an `*InvalidArgumentError`, and
// the session keeps what it had.
func (s *ConsistencySession) Record(token string) error { return s.RecordIn(token, "") }

// RecordIn merges a token into the scope's.
func (s *ConsistencySession) RecordIn(token, scope string) error {
	if token == "" {
		return nil
	}
	parsed, err := ParseConsistencyToken(token)
	s.mu.Lock()
	defer s.mu.Unlock()
	if err != nil {
		s.rejected++
		return err
	}
	if s.tokens == nil {
		s.tokens = map[string]ConsistencyToken{}
	}
	if held, ok := s.tokens[scope]; ok {
		s.tokens[scope] = held.Merge(parsed)
	} else {
		s.tokens[scope] = parsed.Normalized()
	}
	return nil
}

// Rejected is how many tokens the session could not parse.
func (s *ConsistencySession) Rejected() int {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.rejected
}

// Conflicts is Rejected, under the interface's older name.
func (s *ConsistencySession) Conflicts() int { return s.Rejected() }

// Clear forgets every token, so the next read is not held to one.
func (s *ConsistencySession) Clear() {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.tokens, s.rejected = nil, 0
}

// SessionPackages are the packages whose reads take the session's token as
// `at_least` (R4, SDKP-17): collections and SQL. A graph's token names one
// graph, and a Graph read refuses any other.
var SessionPackages = map[string]bool{"loams.collection.v1": true, "loams.sql.v1": true}

// currentIn is a store's token for scope, through ScopedTokenStore when it
// offers one.
func currentIn(session ConsistencyTokenStore, scope string) string {
	if scoped, ok := session.(ScopedTokenStore); ok {
		return scoped.CurrentIn(scope)
	}
	return session.Current()
}

// tokenFields are the response fields a token is read from (R4, SDKP §2.1).
var tokenFields = []protoreflect.Name{"consistency_token", "token", "read_token"}

// recordConsistency merges every token a response carried into a session.
//
// A failure here is **counted, not returned**: the RPC already succeeded, and a
// caller that retries on that error performs the write twice. A string that is
// not a `v1:` token at all — a page token, a bearer — is not the session's
// business and is skipped without counting.
func recordConsistency(session ConsistencyTokenStore, response any, header, scope string) {
	if session == nil {
		return
	}
	tokens := responseTokens(response)
	if header != "" {
		tokens = append(tokens, header)
	}
	for _, token := range tokens {
		if !IsConsistencyToken(token) {
			continue
		}
		if scoped, ok := session.(ScopedTokenStore); ok {
			_ = scoped.RecordIn(token, scope)
			continue
		}
		_ = session.Record(token)
	}
}

// responseTokens reads the token fields off a response. A proto message is
// read through its descriptor; anything else (a test's plain struct) by Go
// field name.
func responseTokens(response any) []string {
	if response == nil {
		return nil
	}
	var out []string
	if message, ok := response.(proto.Message); ok {
		reflected := message.ProtoReflect()
		if !reflected.IsValid() {
			return nil
		}
		fields := reflected.Descriptor().Fields()
		for _, name := range tokenFields {
			field := fields.ByName(name)
			if field == nil || field.Kind() != protoreflect.StringKind || field.IsList() {
				continue
			}
			if value := reflected.Get(field).String(); value != "" {
				out = append(out, value)
			}
		}
		return out
	}
	holder := reflect.ValueOf(response)
	for holder.Kind() == reflect.Pointer {
		if holder.IsNil() {
			return nil
		}
		holder = holder.Elem()
	}
	if !holder.IsValid() || holder.Kind() != reflect.Struct {
		return nil
	}
	for _, name := range []string{ConsistencyTokenField, "Token", "ReadToken"} {
		field := holder.FieldByName(name)
		if field.IsValid() && field.Kind() == reflect.String && field.String() != "" {
			out = append(out, field.String())
		}
	}
	return out
}

// consistencyField is the request's `consistency` message field and how to
// say "at least this token" in it: a plain `at_least` field
// (`loams.collection.v1.Consistency`) or the `at_least` arm of a oneof
// (`loams.graph.v1.Consistency`) — protoreflect sets both the same way.
func consistencyField(message protoreflect.Message) (protoreflect.FieldDescriptor, protoreflect.FieldDescriptor) {
	field := message.Descriptor().Fields().ByName("consistency")
	if field == nil || field.Kind() != protoreflect.MessageKind || field.IsList() || field.IsMap() {
		return nil, nil
	}
	atLeast := field.Message().Fields().ByName("at_least")
	if atLeast == nil || atLeast.Kind() != protoreflect.StringKind || atLeast.IsList() {
		return nil, nil
	}
	return field, atLeast
}
