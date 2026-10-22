// `Streams.Subscribe` (runtime contract R14, SDKP §2.5 and SDKP-2).
//
//	sub := loams.Subscribe(ctx, fetch, 1)
//	defer sub.Close()
//	for sub.Next() {
//	    handle(sub.Record())
//	}
//	if err := sub.Err(); err != nil { ... }
//	resumeFrom := sub.Offset() // the next offset to read
//
// A client loop over unary `Fetch` rather than a server stream, so it works on
// every transport and through every proxy:
//
//   - The next offset is the last record's offset + 1.
//   - An empty fetch backs off 100 ms, doubling, up to 1 s; the backoff resets
//     when a record arrives. A server long poll (`max_wait_ms`, P0c) is the
//     fetch function's business, and the backoff then only applies when it
//     answers empty at its deadline.
//   - `Offset` is always the next offset to read, so a caller can stop and
//     resume without losing or repeating a record.
//   - Cancelling the context or calling Close stops the loop; no further fetch
//     is sent. Neither is an error.

package loams

import (
	"context"
	"errors"
	"sync"
	"time"
)

// The first and the largest pause after an empty fetch.
const (
	SubscribeMinBackoff = 100 * time.Millisecond
	SubscribeMaxBackoff = time.Second
)

// RecordFetcher is one `Fetch` from offset; offsetOf reads a record's offset.
type RecordFetcher[R any] func(ctx context.Context, offset uint64) ([]R, error)

// Subscription is a running subscription.
type Subscription[R any] struct {
	ctx      context.Context
	cancel   context.CancelFunc
	fetch    RecordFetcher[R]
	offsetOf func(R) uint64

	mu      sync.Mutex
	offset  uint64
	buffer  []R
	current R
	err     error
	done    bool
	backoff time.Duration
}

// Subscribe starts a subscription at fromOffset. Nothing is fetched until the
// first Next.
func Subscribe[R any](ctx context.Context, fetch RecordFetcher[R], offsetOf func(R) uint64, fromOffset uint64) *Subscription[R] {
	inner, cancel := context.WithCancel(ctx)
	return &Subscription[R]{
		ctx: inner, cancel: cancel, fetch: fetch, offsetOf: offsetOf,
		offset: fromOffset, backoff: SubscribeMinBackoff,
	}
}

// Next advances to the next record, fetching and backing off as needed. It
// returns false when the subscription is closed, its context is done, or a
// fetch failed (see Err).
func (s *Subscription[R]) Next() bool {
	for {
		s.mu.Lock()
		if s.done {
			s.mu.Unlock()
			return false
		}
		if len(s.buffer) > 0 {
			s.current = s.buffer[0]
			s.buffer = s.buffer[1:]
			s.offset = s.offsetOf(s.current) + 1
			s.mu.Unlock()
			return true
		}
		offset := s.offset
		s.mu.Unlock()

		records, err := s.fetch(s.ctx, offset)
		if s.ctx.Err() != nil {
			s.finish(nil)
			return false
		}
		if err != nil {
			s.finish(err)
			return false
		}
		kept := records[:0:0]
		for _, record := range records {
			// A record below the next offset was already yielded (a server that
			// answers from a segment start); skipping it keeps resume exact.
			if s.offsetOf(record) >= offset {
				kept = append(kept, record)
			}
		}
		s.mu.Lock()
		s.buffer = kept
		wait := s.backoff
		if len(kept) > 0 {
			s.backoff = SubscribeMinBackoff
		} else {
			s.backoff = min(s.backoff*2, SubscribeMaxBackoff)
		}
		s.mu.Unlock()
		if len(kept) > 0 {
			continue
		}
		if err := Sleep(s.ctx, wait); err != nil {
			s.finish(nil)
			return false
		}
	}
}

func (s *Subscription[R]) finish(err error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	if !s.done {
		s.done = true
		if err != nil && !errors.Is(err, context.Canceled) {
			s.err = ToLoamsError(err, "")
		}
	}
	s.cancel()
}

// Record is the record Next advanced to.
func (s *Subscription[R]) Record() R {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.current
}

// Offset is the next offset to read.
func (s *Subscription[R]) Offset() uint64 {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.offset
}

// Err is the fetch failure that ended the subscription, or nil.
func (s *Subscription[R]) Err() error {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.err
}

// Close stops the subscription; a Next in progress returns false.
func (s *Subscription[R]) Close() error {
	s.finish(nil)
	return nil
}
