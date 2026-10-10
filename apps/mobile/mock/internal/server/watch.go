package server

import (
	"context"
	"time"

	"connectrpc.com/connect"

	"github.com/ostrium-labs/loams-mobile/mock/internal/hub"
)

// sink turns hub events into one service's Watch*Response messages.
type sink[T any] struct {
	id        func(T) string
	match     func(T) bool
	snapshot  func(items []T, cursor string, reset bool) error
	upsert    func(item T, cursor string) error
	remove    func(id, cursor string) error
	heartbeat func(cursor string) error
}

// runWatch is AP0 Ruling 3: a snapshot (or the changes after a valid resume
// cursor), then changes, then a heartbeat every interval, until the client
// goes away, the mock drops streams, or the subscriber lags.
func runWatch[T any](ctx context.Context, s *Server, h *hub.Hub[T], cursor string, k sink[T]) error {
	sub := h.Subscribe(cursor)
	defer sub.Cancel()
	drop := s.dropSignal()
	sent := map[string]bool{}
	apply := func(e hub.Event[T], cur string) error {
		if !e.Gone && k.match(e.Item) {
			sent[e.ID] = true
			return k.upsert(e.Item, cur)
		}
		if sent[e.ID] {
			delete(sent, e.ID)
			return k.remove(e.ID, cur)
		}
		return nil
	}
	if sub.Snapshot != nil {
		items := make([]T, 0, len(sub.Snapshot))
		for _, it := range sub.Snapshot {
			if k.match(it) {
				items = append(items, it)
				sent[k.id(it)] = true
			}
		}
		if err := k.snapshot(items, sub.Cursor, sub.Reset); err != nil {
			return err
		}
	} else {
		// A valid resume: the client already holds what it saw. Seed `sent`
		// from the current set so later removals reach it.
		for _, it := range h.List() {
			if k.match(it) {
				sent[k.id(it)] = true
			}
		}
		for _, e := range sub.Replay {
			// The client may have held this item at its cursor even if it no
			// longer matches (or is gone): mark it, so the replay sends a
			// remove. A redundant remove is harmless.
			sent[e.ID] = true
			if err := apply(e, itoa(e.Seq)); err != nil {
				return err
			}
		}
		if len(sub.Replay) == 0 {
			if err := k.heartbeat(sub.Cursor); err != nil {
				return err
			}
		}
	}
	tick := time.NewTicker(s.cfg.Heartbeat)
	defer tick.Stop()
	last := sub.Cursor
	for {
		select {
		case <-ctx.Done():
			return nil
		case <-drop:
			return connect.NewError(connect.CodeUnavailable, errDropped)
		case <-tick.C:
			if err := k.heartbeat(last); err != nil {
				return err
			}
		case e, ok := <-sub.Changes:
			if !ok {
				return connect.NewError(connect.CodeUnavailable, errLagging)
			}
			last = itoa(e.Seq)
			if err := apply(e, last); err != nil {
				return err
			}
		}
	}
}
