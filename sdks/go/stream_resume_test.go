// SDK2 Task 2's `go_stream_resume_with_cursor`.
//
// Design §44 §7.4, D610 and runtime contract R7: a server stream is the one
// call where "retry it" is not enough. The server hands out cursors; a reconnect
// has to resume from the last one the client applied, or the client silently
// misses everything that changed in between — which is worse than an error,
// because a sync UI that is quietly stale looks exactly like a sync UI that
// works.
//
// So the assertions are about the *cursor*: that it is read off every message,
// that it is the one carried into the re-open, that the messages already yielded
// are not re-yielded, and that a failure the retry class does not cover is
// reported rather than swallowed.
//
// **The resume half is pinned against a stub**, because the only server stream
// the API has (`LiveService/Watch`) is never served in any variant: it answers
// `feature_not_in_variant` in an end-stream envelope. `TestGoStreamReportsEnvelopeRefusal`
// runs the other half against the corpus — the refusal, read out of the envelope
// rather than off the HTTP status — which is the half a recording can prove.

package loams

import (
	"context"
	"errors"
	"testing"
	"time"

	connect "connectrpc.com/connect"

	"loams.dev/go/gen/facade"
)

// stubMessage is shaped like a live `Transition`: it carries a cursor.
type stubMessage struct {
	SessionID string
	Cursor    string
}

// stubReceiver is a `Receiver` over a fixed script: some messages, then either a
// clean end or a failure. It stands in for `*connect.ServerStreamForClient`,
// whose shape (`Receive() bool`, `Msg()`, `Err()`, `Close()`) it is the reason
// for.
type stubReceiver struct {
	messages []stubMessage
	failWith error
	received int
	closed   bool
}

func (r *stubReceiver) Receive() bool {
	if r.received >= len(r.messages) {
		return false
	}
	r.received++
	return true
}

func (r *stubReceiver) Msg() *stubMessage { return &r.messages[r.received-1] }

func (r *stubReceiver) Err() error {
	if r.received < len(r.messages) {
		return nil
	}
	return r.failWith
}

func (r *stubReceiver) Close() error {
	r.closed = true
	return nil
}

// stubStream is a scripted `*Stream`. It holds the two things the assertions
// about R7 need to look at: the cursor every open was given, and the cursor every
// message carried.
type stubStream struct {
	*Stream[stubMessage]

	batches  [][]stubMessage
	failWith func(open int) error
	// opened is the cursor each open was asked to resume from, in order.
	opened []string
	// cursors is the cursor every message carried, in order.
	cursors []string
	// next is the batch the next open will use.
	next int
}

// newStubStream builds a stream over scripted batches with a resume wired in.
// failWith(open) is the error that ends the open with that index; nil is a clean
// end.
func newStubStream(t *testing.T, batches [][]stubMessage, failWith func(open int) error) *stubStream {
	t.Helper()
	if len(batches) == 0 {
		t.Fatal("a stub stream needs at least one batch")
	}
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	t.Cleanup(cancel)

	stub := &stubStream{batches: batches, failWith: failWith}
	source := stub.sourceFor(0, "")
	// The first open consumed batch 0, so the next one starts at 1.
	stub.next = 1
	stub.Stream = &Stream[stubMessage]{
		ctx:        ctx,
		binding:    bindingInfo{RPC: "loams.live.v1.LiveService/Watch", RetrySafe: true},
		source:     source,
		maxRetries: DefaultMaxRetries,
		retrySafe:  true,
		cursorOf:   func(message *stubMessage) string { return message.Cursor },
		onCursor:   func(cursor string, _ *stubMessage) { stub.cursors = append(stub.cursors, cursor) },
	}
	stub.Stream.open = func(_ context.Context, cursor string) (Receiver[stubMessage], error) {
		return stub.reopen(cursor), nil
	}
	return stub
}

// sourceFor builds the receiver for the batch at `index`, recording the cursor it
// was opened with.
func (s *stubStream) sourceFor(index int, cursor string) *stubReceiver {
	if index < len(s.opened) {
		s.opened[index] = cursor
	} else {
		s.opened = append(s.opened, cursor)
	}
	if index >= len(s.batches) {
		// Past the script: a clean end, which is how a real server says "there is
		// nothing more".
		return &stubReceiver{}
	}
	return &stubReceiver{messages: s.batches[index], failWith: s.failWith(index)}
}

// reopen is the `Stream.open` callback.
func (s *stubStream) reopen(cursor string) Receiver[stubMessage] {
	source := s.sourceFor(s.next, cursor)
	s.next++
	s.source = source
	return source
}

// TestGoStreamResumeWithCursor is the required test.
func TestGoStreamResumeWithCursor(t *testing.T) {
	t.Run(StreamResumeWithCursor, func(t *testing.T) {
		testStreamResumeWithCursor(t)
	})
}

func testStreamResumeWithCursor(t *testing.T) {
	t.Run("the generated binding says watch is a server stream", func(t *testing.T) {
		watch, ok := facade.Binding("live", "Watch")
		if !ok {
			t.Fatal("the binding table has no live.Watch")
		}
		if watch.Streaming != facade.Server {
			t.Errorf("live.Watch is %q, want a server stream", watch.Streaming)
		}
		if watch.Module != "live" {
			t.Errorf("live.Watch is on module %q", watch.Module)
		}
		if watch.Method != "Watch" {
			t.Errorf("live.Watch backs method %q", watch.Method)
		}
		// And the facade hands back a `*Stream`, which is what design §44 §7.1
		// asks for in Go. The conformance test calls it as one.
		var stream *Stream[Transition]
		_ = stream
	})

	t.Run("re-opens from the last cursor and does not repeat a message", func(t *testing.T) {
		// Two sessions: the first dies after two messages with a retryable code,
		// the second runs to completion. The second must be opened from the second
		// message's cursor.
		stub := newStubStream(t,
			[][]stubMessage{
				{{SessionID: "s1", Cursor: "c1"}, {SessionID: "s1", Cursor: "c2"}},
				{{SessionID: "s1", Cursor: "c3"}, {SessionID: "s1", Cursor: "c4"}},
			},
			func(open int) error {
				if open == 0 {
					return connect.NewError(connect.CodeUnavailable, errors.New("the node is restarting"))
				}
				return nil
			})

		var seen []stubMessage
		for stub.Receive() {
			seen = append(seen, *stub.Msg())
		}
		if err := stub.Err(); err != nil {
			t.Fatalf("the stream failed with %v, want nil", err)
		}
		if len(seen) != 4 {
			t.Fatalf("the stream yielded %d messages, want 4: %+v", len(seen), seen)
		}
		for index, message := range seen {
			want := "c" + string(rune('1'+index))
			if message.Cursor != want {
				t.Errorf("message %d has cursor %q, want %q: a resumed stream repeated or skipped a message",
					index, message.Cursor, want)
			}
		}
		if len(stub.opened) != 2 {
			t.Fatalf("the stream was opened %d times, want 2: %v", len(stub.opened), stub.opened)
		}
		if stub.opened[1] != "c2" {
			t.Errorf("the re-open resumed from %q, want %q: the cursor of the last message applied",
				stub.opened[1], "c2")
		}
		if len(stub.cursors) != 4 {
			t.Errorf("OnCursor saw %d cursors, want 4: %v", len(stub.cursors), stub.cursors)
		}
		if got := stub.Cursor(); got != "c4" {
			t.Errorf("Stream.Cursor is %q, want %q", got, "c4")
		}
	})

	t.Run("progress earns a fresh retry budget", func(t *testing.T) {
		// `maxRetries` bounds the re-opens in one run of disconnects, not for the
		// life of the stream. A watch that recovers from a node restart and then
		// runs for days must not spend its budget on the first failure of each of
		// those days.
		batches := [][]stubMessage{{{Cursor: "c1"}}, {{Cursor: "c2"}}, {{Cursor: "c3"}}, {{Cursor: "c4"}}, {{Cursor: "c5"}}}
		stub := newStubStream(t, batches, func(open int) error {
			if open < len(batches)-1 {
				return connect.NewError(connect.CodeUnavailable, errors.New("the node is restarting"))
			}
			return nil
		})
		stub.maxRetries = 1

		received := 0
		for stub.Receive() {
			received++
		}
		if err := stub.Err(); err != nil {
			t.Fatalf("the stream failed with %v, want nil: a budget of 1 must reset after each message", err)
		}
		if received != len(batches) {
			t.Errorf("the stream yielded %d messages, want %d", received, len(batches))
		}
	})

	t.Run("a failure the retry class does not cover is reported, not spun on", func(t *testing.T) {
		// `unimplemented` is the important one: every `loams.live.v1` RPC answers
		// it in the standard variant, and a client that retried would hammer a
		// refusal that will never change.
		stub := newStubStream(t,
			[][]stubMessage{{{Cursor: "c1"}}, {{Cursor: "c2"}}, {{Cursor: "c3"}}},
			func(int) error {
				return connect.NewError(connect.CodeUnimplemented, errors.New("not in the standard variant"))
			})
		stub.retrySafe = true
		stub.maxRetries = 3

		received := 0
		for stub.Receive() {
			received++
		}
		if received != 1 {
			t.Errorf("the stream yielded %d messages, want 1 before the refusal", received)
		}
		err := stub.Err()
		if err == nil {
			t.Fatal("the stream ended without reporting the refusal")
		}
		var unimplemented *UnimplementedError
		if !errors.As(err, &unimplemented) {
			t.Errorf("the failure is %T, want an *UnimplementedError", err)
		}
		if len(stub.opened) != 1 {
			t.Errorf("the stream was re-opened %d times, want 1: %v", len(stub.opened), stub.opened)
		}
	})

	t.Run("a stream with no resume reports its failure rather than reopening", func(t *testing.T) {
		// Without `WithStreamResume` the SDK does not know how to re-open, so the
		// honest thing is to report. Guessing a resume would be a method that
		// silently restarts from the beginning and re-applies every message.
		stub := newStubStream(t,
			[][]stubMessage{{{Cursor: "c1"}}},
			func(int) error {
				return connect.NewError(connect.CodeUnavailable, errors.New("the node is restarting"))
			})
		stub.retrySafe = true
		stub.maxRetries = 3
		stub.open = nil

		for stub.Receive() {
		}
		if stub.Err() == nil {
			t.Fatal("the stream ended without reporting the failure")
		}
		if len(stub.opened) != 1 {
			t.Errorf("a stream with no resume was opened %d times, want 1", len(stub.opened))
		}
	})

	t.Run("a clean end is not an error and is not resumed", func(t *testing.T) {
		stub := newStubStream(t,
			[][]stubMessage{{{Cursor: "c1"}, {Cursor: "c2"}}},
			func(int) error { return nil })

		received := 0
		for stub.Receive() {
			received++
		}
		if err := stub.Err(); err != nil {
			t.Errorf("a clean end reported %v, want nil", err)
		}
		if received != 2 {
			t.Errorf("the stream yielded %d messages, want 2", received)
		}
		if len(stub.opened) != 1 {
			t.Errorf("the stream was opened %d times, want 1", len(stub.opened))
		}
		// And it stays ended: a second Receive must not restart it, because a
		// caller that loops on Receive would otherwise spin forever.
		if stub.Receive() {
			t.Error("a finished stream yielded another message")
		}
	})

	t.Run("Range is the range-over-func over the same stream", func(t *testing.T) {
		batches := [][]stubMessage{{{Cursor: "c1"}, {Cursor: "c2"}, {Cursor: "c3"}}}
		stub := newStubStream(t, batches, func(int) error { return nil })

		var seen []string
		for message := range stub.Range() {
			seen = append(seen, message.Cursor)
		}
		if len(seen) != 3 {
			t.Fatalf("Range yielded %v, want three cursors", seen)
		}
		if err := stub.Err(); err != nil {
			t.Errorf("Range left Err set to %v", err)
		}
		// Breaking out of Range stops it, and does not report an error: the
		// caller asked to stop, which is not a failure.
		stub = newStubStream(t, batches, func(int) error { return nil })
		count := 0
		for range stub.Range() {
			count++
			break
		}
		if count != 1 {
			t.Errorf("breaking out of Range yielded %d messages, want 1", count)
		}
		if err := stub.Err(); err != nil {
			t.Errorf("breaking out of Range reported %v, want nil", err)
		}
	})

	t.Run("Close is safe to call twice and after the end", func(t *testing.T) {
		source := &stubReceiver{messages: []stubMessage{{Cursor: "c1"}}}
		stream := &Stream[stubMessage]{ctx: context.Background(), source: source}
		if err := stream.Close(); err != nil {
			t.Errorf("the first Close returned %v", err)
		}
		if err := stream.Close(); err != nil {
			t.Errorf("the second Close returned %v", err)
		}
		if !source.closed {
			t.Error("Close did not reach the underlying stream")
		}
	})

	t.Run("a cancelled context stops the resume loop", func(t *testing.T) {
		ctx, cancel := context.WithCancel(context.Background())
		t.Cleanup(cancel)
		stub := newStubStream(t,
			[][]stubMessage{{{Cursor: "c1"}}, {{Cursor: "c2"}}},
			func(int) error {
				cancel()
				return connect.NewError(connect.CodeUnavailable, errors.New("the node is restarting"))
			})
		stub.ctx = ctx
		stub.retrySafe = true
		stub.maxRetries = 3

		for stub.Receive() {
		}
		if stub.Err() == nil {
			t.Fatal("the stream ended without reporting the failure it stopped on")
		}
		if len(stub.opened) != 1 {
			t.Errorf("a cancelled context still re-opened the stream: %v", stub.opened)
		}
	})
}

// TestGoStreamReportsEnvelopeRefusal runs the half of R7 the corpus can prove: a
// refusal on a server stream arrives inside the Connect envelope rather than as
// an HTTP status, so the SDK has to read the envelope to report a reason at all.
func TestGoStreamReportsEnvelopeRefusal(t *testing.T) {
	server := startFixtureServer(t)
	client := newTestClient(t, server.endpoint)
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()

	stream, err := client.Live().Watch(ctx, &WatchRequest{})
	if err != nil {
		// The refusal arriving at open time rather than at the first Receive is the
		// same failure; both are legal for a Connect client, and both must map to
		// the same type.
		assertEnvelopeRefusal(t, err)
		return
	}
	received := 0
	for stream.Receive() {
		received++
	}
	_ = stream.Close()
	if received != 0 {
		t.Errorf("watch yielded %d messages before the refusal, want none", received)
	}
	assertEnvelopeRefusal(t, stream.Err())
}

func assertEnvelopeRefusal(t *testing.T, err error) {
	t.Helper()
	if err == nil {
		t.Fatal("watch ended without a failure, but the corpus records a refusal")
	}
	var absent *FeatureNotInVariantError
	if !errors.As(err, &absent) {
		t.Fatalf("watch failed with %T (%v), want a *FeatureNotInVariantError: the refusal arrives in the "+
			"Connect end-stream envelope, so a client that reads only the HTTP status sees a 200 and no reason",
			err, err)
	}
	if absent.Reason != ReasonFeatureNotInVariant {
		t.Errorf("the refusal reason is %q, want %q", absent.Reason, ReasonFeatureNotInVariant)
	}
	if absent.Variant != "standard" {
		t.Errorf("the refusal variant is %q, want %q from metadata.variant", absent.Variant, "standard")
	}
}
