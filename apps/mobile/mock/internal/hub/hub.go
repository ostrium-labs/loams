// Package hub keeps one resource's objects and fans their changes out to
// Watch streams, with cursors that let a dropped stream resume (AP0 Ruling 3).
package hub

import (
	"strconv"
	"sync"
)

// Event is one change: Item is nil for a removal.
type Event[T any] struct {
	Seq  uint64
	ID   string
	Item T
	Gone bool
}

// Hub holds objects by id in insertion order, a bounded change log and the
// open subscriptions.
type Hub[T any] struct {
	mu    sync.Mutex
	items map[string]T
	order []string
	seq   uint64
	log   []Event[T]
	keep  int
	subs  map[chan Event[T]]struct{}
}

// New returns a hub that keeps the last `keep` changes for resumption.
func New[T any](keep int) *Hub[T] {
	return &Hub[T]{items: map[string]T{}, keep: keep, subs: map[chan Event[T]]struct{}{}}
}

// Put inserts or replaces an object and notifies subscribers.
func (h *Hub[T]) Put(id string, v T) {
	h.mu.Lock()
	defer h.mu.Unlock()
	if _, ok := h.items[id]; !ok {
		h.order = append(h.order, id)
	}
	h.items[id] = v
	h.emit(Event[T]{ID: id, Item: v})
}

// Remove deletes an object and notifies subscribers.
func (h *Hub[T]) Remove(id string) {
	h.mu.Lock()
	defer h.mu.Unlock()
	if _, ok := h.items[id]; !ok {
		return
	}
	delete(h.items, id)
	for i, o := range h.order {
		if o == id {
			h.order = append(h.order[:i], h.order[i+1:]...)
			break
		}
	}
	var zero T
	h.emit(Event[T]{ID: id, Item: zero, Gone: true})
}

func (h *Hub[T]) emit(e Event[T]) {
	h.seq++
	e.Seq = h.seq
	h.log = append(h.log, e)
	if len(h.log) > h.keep {
		h.log = h.log[len(h.log)-h.keep:]
	}
	for ch := range h.subs {
		select {
		case ch <- e:
		default:
			// A subscriber that cannot keep up is dropped; it resumes by cursor.
			close(ch)
			delete(h.subs, ch)
		}
	}
}

// Get returns one object.
func (h *Hub[T]) Get(id string) (T, bool) {
	h.mu.Lock()
	defer h.mu.Unlock()
	v, ok := h.items[id]
	return v, ok
}

// List returns every object in insertion order.
func (h *Hub[T]) List() []T {
	h.mu.Lock()
	defer h.mu.Unlock()
	out := make([]T, 0, len(h.order))
	for _, id := range h.order {
		out = append(out, h.items[id])
	}
	return out
}

// Update applies f to an object under the lock and emits the result.
func (h *Hub[T]) Update(id string, f func(T) (T, error)) (T, error) {
	h.mu.Lock()
	defer h.mu.Unlock()
	cur, ok := h.items[id]
	if !ok {
		var zero T
		return zero, ErrNotFound
	}
	next, err := f(cur)
	if err != nil {
		return cur, err
	}
	h.items[id] = next
	h.emit(Event[T]{ID: id, Item: next})
	return next, nil
}

// Subscription is what a Watch stream starts from.
type Subscription[T any] struct {
	// Snapshot is set when the stream must start from a full snapshot.
	Snapshot []T
	// Reset is true when a resume cursor was given but is no longer valid.
	Reset bool
	// Replay holds the changes after a still-valid resume cursor.
	Replay []Event[T]
	// Cursor is the position the snapshot or replay ends at.
	Cursor string
	// Changes delivers later events; it is closed if the subscriber lags.
	Changes <-chan Event[T]
	Cancel  func()
}

// Subscribe starts a watch. An empty cursor means a snapshot; a valid one
// replays what was missed; an unknown or expired one resets to a snapshot.
func (h *Hub[T]) Subscribe(cursor string) Subscription[T] {
	h.mu.Lock()
	defer h.mu.Unlock()
	ch := make(chan Event[T], 256)
	h.subs[ch] = struct{}{}
	sub := Subscription[T]{Changes: ch, Cursor: strconv.FormatUint(h.seq, 10)}
	sub.Cancel = func() {
		h.mu.Lock()
		defer h.mu.Unlock()
		if _, ok := h.subs[ch]; ok {
			delete(h.subs, ch)
			close(ch)
		}
	}
	if cursor != "" {
		if at, err := strconv.ParseUint(cursor, 10, 64); err == nil && h.resumable(at) {
			for _, e := range h.log {
				if e.Seq > at {
					sub.Replay = append(sub.Replay, e)
				}
			}
			return sub
		}
		sub.Reset = true
	}
	sub.Snapshot = make([]T, 0, len(h.order))
	for _, id := range h.order {
		sub.Snapshot = append(sub.Snapshot, h.items[id])
	}
	return sub
}

// resumable reports whether every change after `at` is still in the log.
func (h *Hub[T]) resumable(at uint64) bool {
	if at > h.seq {
		return false
	}
	if at == h.seq {
		return true
	}
	return len(h.log) > 0 && h.log[0].Seq <= at+1
}

// ErrNotFound is returned by Update for an unknown id.
var ErrNotFound = errNotFound{}

type errNotFound struct{}

func (errNotFound) Error() string { return "not found" }
