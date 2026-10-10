package server

import (
	"context"
	"slices"
	"time"

	"connectrpc.com/connect"
	"google.golang.org/protobuf/proto"
	"google.golang.org/protobuf/types/known/structpb"
	"google.golang.org/protobuf/types/known/timestamppb"

	operationsv1 "github.com/ostrium-labs/loams-mobile/mock/gen/loams/operations/v1"
)

type operationsService struct{ s *Server }

func cloneOp(o *operationsv1.Operation) *operationsv1.Operation {
	return proto.Clone(o).(*operationsv1.Operation)
}

func opMatcher(namespaces []string, states []operationsv1.OperationState) func(*operationsv1.Operation) bool {
	return func(o *operationsv1.Operation) bool {
		if len(namespaces) > 0 && !slices.Contains(namespaces, o.Namespace) {
			return false
		}
		return len(states) == 0 || slices.Contains(states, o.State)
	}
}

func (o operationsService) GetOperation(_ context.Context, req *connect.Request[operationsv1.GetOperationRequest]) (*connect.Response[operationsv1.GetOperationResponse], error) {
	op, ok := o.s.operations.Get(req.Msg.OperationId)
	if !ok {
		return nil, fail(connect.CodeNotFound, "", "no such operation")
	}
	return connect.NewResponse(&operationsv1.GetOperationResponse{Operation: cloneOp(op)}), nil
}

func (o operationsService) ListOperations(_ context.Context, req *connect.Request[operationsv1.ListOperationsRequest]) (*connect.Response[operationsv1.ListOperationsResponse], error) {
	match := opMatcher(req.Msg.Namespaces, req.Msg.States)
	out := &operationsv1.ListOperationsResponse{}
	for _, op := range o.s.operations.List() {
		if match(op) {
			out.Operations = append(out.Operations, cloneOp(op))
		}
	}
	return connect.NewResponse(out), nil
}

func (o operationsService) WatchOperations(ctx context.Context, req *connect.Request[operationsv1.WatchOperationsRequest], stream *connect.ServerStream[operationsv1.WatchOperationsResponse]) error {
	return runWatch(ctx, o.s, o.s.operations, req.Msg.ResumeCursor, sink[*operationsv1.Operation]{
		id:    func(x *operationsv1.Operation) string { return x.Id },
		match: opMatcher(req.Msg.Namespaces, req.Msg.States),
		snapshot: func(items []*operationsv1.Operation, cur string, reset bool) error {
			return stream.Send(&operationsv1.WatchOperationsResponse{Event: &operationsv1.WatchOperationsResponse_Snapshot{Snapshot: &operationsv1.OperationSnapshot{Operations: items}}, Cursor: cur, SnapshotReset: reset})
		},
		upsert: func(x *operationsv1.Operation, cur string) error {
			return stream.Send(&operationsv1.WatchOperationsResponse{Event: &operationsv1.WatchOperationsResponse_Upsert{Upsert: x}, Cursor: cur})
		},
		remove: func(id, cur string) error {
			return stream.Send(&operationsv1.WatchOperationsResponse{Event: &operationsv1.WatchOperationsResponse_Remove{Remove: id}, Cursor: cur})
		},
		heartbeat: func(cur string) error {
			return stream.Send(&operationsv1.WatchOperationsResponse{Event: &operationsv1.WatchOperationsResponse_Heartbeat{Heartbeat: &operationsv1.Heartbeat{}}, Cursor: cur})
		},
	})
}

func (o operationsService) CancelOperation(_ context.Context, req *connect.Request[operationsv1.CancelOperationRequest]) (*connect.Response[operationsv1.CancelOperationResponse], error) {
	next, err := o.s.operations.Update(req.Msg.OperationId, func(cur *operationsv1.Operation) (*operationsv1.Operation, error) {
		switch cur.State {
		case operationsv1.OperationState_OPERATION_STATE_SUCCEEDED, operationsv1.OperationState_OPERATION_STATE_FAILED, operationsv1.OperationState_OPERATION_STATE_CANCELED:
			return nil, fail(connect.CodeFailedPrecondition, "", "the operation already finished")
		}
		next := cloneOp(cur)
		next.State = operationsv1.OperationState_OPERATION_STATE_CANCELED
		next.UpdatedAt = timestamppb.New(o.s.now())
		return next, nil
	})
	if err != nil {
		if ce := new(connect.Error); asConnect(err, &ce) {
			return nil, ce
		}
		return nil, fail(connect.CodeNotFound, "", "no such operation")
	}
	return connect.NewResponse(&operationsv1.CancelOperationResponse{Operation: cloneOp(next)}), nil
}

// tickOperations advances running operations so the status screen moves,
// and starts a new one when none is running.
func (s *Server) tickOperations(ctx context.Context) {
	t := time.NewTicker(s.cfg.Tick)
	defer t.Stop()
	for {
		select {
		case <-ctx.Done():
			return
		case <-t.C:
			s.Tick()
		}
	}
}

// Tick advances every running operation by 5 %.
func (s *Server) Tick() {
	running := 0
	for _, op := range s.operations.List() {
		if op.State != operationsv1.OperationState_OPERATION_STATE_RUNNING {
			continue
		}
		running++
		_, _ = s.operations.Update(op.Id, func(cur *operationsv1.Operation) (*operationsv1.Operation, error) {
			next := cloneOp(cur)
			p := next.Progress
			if p == nil {
				p = &operationsv1.Progress{Total: 100}
				next.Progress = p
			}
			p.Done = min(p.Total, p.Done+5)
			p.Fraction = proto.Float64(float64(p.Done) / float64(max(1, p.Total)))
			next.UpdatedAt = timestamppb.New(s.now())
			if p.Done >= p.Total {
				next.State = operationsv1.OperationState_OPERATION_STATE_SUCCEEDED
				p.Message = "Done"
				next.Result, _ = structpb.NewStruct(map[string]any{"segments": 12})
			}
			return next, nil
		})
	}
	if running == 0 {
		id := s.newID("op")
		s.operations.Put(id, &operationsv1.Operation{
			Id: id, Kind: "collection.reindex", Namespace: "staging", Target: map[string]string{"collection": "docs"},
			State: operationsv1.OperationState_OPERATION_STATE_RUNNING, Progress: &operationsv1.Progress{Total: 100, Message: "Rebuilding segments"},
			CreatedAt: timestamppb.New(s.now()), UpdatedAt: timestamppb.New(s.now()),
		})
	}
}
