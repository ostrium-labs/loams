// Server streams (design §44 §7.4, D610; runtime contract R7).
//
// The API has server streams only (D420): no client streaming, no bidi, because
// a browser cannot stream full duplex over `fetch` and half-duplex works through
// every proxy. A server stream in Go is a `*loams.Stream` with `Receive()`,
// which is design §44 §7.1's wording:
//
//	stream, err := client.Live().Watch(ctx, request, loams.WithStreamResume(resume))
//	if err != nil {
//	    return err
//	}
//	defer stream.Close()
//	for stream.Receive() {
//	    apply(stream.Msg())
//	    if ctx.Err() != nil {
//	        break
//	    }
//	}
//	return stream.Err()
//
// # Why a stream is not just a retry
//
// A stream is the one call where "send it again" is not enough. The server hands
// out cursors, and a reconnect has to resume from the last one the client
// applied, or the client silently misses everything that changed in between —
// which is worse than an error, because a sync UI that is quietly stale looks
// exactly like a sync UI that works.
//
// So a `Stream` with a resume tracks the cursor of every message, re-opens from
// it on a retryable failure, and does not re-yield what it already yielded. The
// re-open request is the stream's own business — `loams.live`'s `WatchRequest`
// resume, for instance — so the caller passes a `Resume` function and the
// runtime supplies the cursor.
//
// The cursor reader is **required**, with no default. A default would have to
// guess a field name, and the API's one server stream carries no `cursor` field
// at all: its cursor is a `StateVersion`. Guessing would be a method that
// silently resumes from nothing.

package loams

import (
	"context"
	"sync"
)

// Receiver yields a stream's messages. `*connect.ServerStreamForClient` is one,
// which is why this package's `Stream` wraps either a real stream or a stub.
type Receiver[Res any] interface {
	// Receive advances to the next message, and reports whether there was one.
	Receive() bool
	// Msg is the message `Receive` advanced to.
	Msg() *Res
	// Err is why the stream ended, or nil if it ended cleanly.
	Err() error
	// Close releases the stream.
	Close() error
}

// StreamResume describes how a server stream re-opens from a cursor (R7).
//
// It is generic in the request and response types, so the compiler checks that
// `Resume` returns the message the RPC takes. The CallOption carries it
// type-erased and the invoker — which already knows both types — asserts it
// back.
type StreamResume[Req, Res any] struct {
	// Resume is the request to re-open with, given the last cursor seen and the
	// original request. Returning the original request re-opens from the
	// beginning, which is correct — and loses nothing but time — for a stream
	// whose snapshot is complete.
	Resume func(cursor string, request *Req) *Req
	// CursorOf reads the cursor off a message. Required, and there is no
	// default: a stream's cursor is that stream's business.
	CursorOf func(*Res) string
	// MaxRetries bounds the re-opens in one **run** of disconnects. Zero is not
	// a request for none; a negative value leaves the client's default in
	// place.
	MaxRetries int
	// OnCursor is called after each message, with the cursor it carried.
	OnCursor func(cursor string, message *Res)
}

// Stream is a server stream, driven by the caller's loop.
//
// Receive returns false at the end of the stream **or** on a failure, and Err
// says which: nil means the stream finished, non-nil means it broke and nothing
// more will arrive. That distinction is the whole contract of the iterator —
// `for stream.Receive()` alone cannot tell "done" from "broken", and a caller
// that ignores Err sees a silently truncated stream.
type Stream[Res any] struct {
	ctx     context.Context
	binding bindingInfo

	// source is the stream currently being read. Receive replaces it on a
	// resume.
	source Receiver[Res]
	// open re-opens the stream from a cursor. It is nil unless the caller
	// passed WithStreamResume, and a nil open is what turns a broken stream
	// into a reported error instead of a spin.
	open     func(ctx context.Context, cursor string) (Receiver[Res], error)
	cursorOf func(*Res) string
	onCursor func(string, *Res)

	maxRetries int
	retrySafe  bool

	mu      sync.Mutex
	cursor  string
	current *Res
	attempt int
	err     error
	closed  bool
}

// Receive advances to the next message, re-opening from the cursor if the stream
// broke in a way a retry covers. It reports whether there was a message.
func (s *Stream[Res]) Receive() bool {
	s.mu.Lock()
	defer s.mu.Unlock()
	for {
		if s.source == nil {
			return false
		}
		if s.source.Receive() {
			message := s.source.Msg()
			// Progress earns a fresh budget: `maxRetries` bounds the re-opens in
			// one run of disconnects, not for the life of the stream.
			s.attempt = 0
			if s.cursorOf != nil {
				if cursor := s.cursorOf(message); cursor != "" {
					s.cursor = cursor
				}
			}
			if s.onCursor != nil {
				s.onCursor(s.cursor, message)
			}
			s.current = message
			return true
		}
		err := s.source.Err()
		if err == nil {
			// A clean end. Not an error, and not something to resume from: the
			// server finished.
			s.source = nil
			return false
		}
		mapped := ToLoamsError(err, s.binding.RPC)
		if s.open == nil || !ShouldRetry(s.ctx, mapped, s.retrySafe, s.attempt, s.maxRetries) {
			// A failure the retry class does not cover — notably an
			// `unimplemented` stream, which is what every `loams.live.v1` RPC
			// answers in the standard variant — is reported rather than spun on.
			s.err = mapped
			s.source = nil
			return false
		}
		_ = s.source.Close()
		if waitErr := Sleep(s.ctx, Backoff(s.attempt, 0)); waitErr != nil {
			s.err = mapped
			s.source = nil
			return false
		}
		next, openErr := s.open(s.ctx, s.cursor)
		s.attempt++
		if openErr != nil {
			s.err = ToLoamsError(openErr, s.binding.RPC)
			s.source = nil
			return false
		}
		s.source = next
	}
}

// Msg is the message the last `Receive` advanced to. It is valid until the next
// `Receive`, which is the same rule the generated iterators use.
func (s *Stream[Res]) Msg() *Res {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.current
}

// Err is why the stream ended, or nil if it ended cleanly.
func (s *Stream[Res]) Err() error {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.err
}

// Cursor is the last cursor the stream applied, or "" if it carries none.
func (s *Stream[Res]) Cursor() string {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.cursor
}

// Close releases the stream. It is safe to call more than once, and after the
// stream has ended.
func (s *Stream[Res]) Close() error {
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.source == nil || s.closed {
		return nil
	}
	s.closed = true
	return s.source.Close()
}

// Range returns `for message := range stream.Range()`, the range-over-func
// alternative to `for stream.Receive()`. Go 1.23 made range-over-func the
// language's own answer to "iterate this stream", and a caller who wants
// `break` with no post-loop check should reach for it.
//
// The loop stops on the first error or after the last message, and `Err` says
// which — the same rule as `Receive`, so Range is a loop, not a contract change.
func (s *Stream[Res]) Range() func(func(*Res) bool) {
	return func(yield func(*Res) bool) {
		for s.Receive() {
			if !yield(s.Msg()) {
				return
			}
		}
	}
}

// bindingInfo is the one field of a call binding a stream needs. It is a plain
// struct so `streams.go` does not import the generated facade package.
type bindingInfo struct {
	// RPC is `package.Service/Method`, which is what a failure is reported
	// against.
	RPC string
	// Retry is the call's class from the generated bindings. A server stream is
	// `manual` today, so a stream only resumes when the caller says so through
	// WithStreamResume.
	RetrySafe bool
}
