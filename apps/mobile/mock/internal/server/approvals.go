package server

import (
	"context"
	"encoding/json"
	"errors"
	"slices"
	"strconv"
	"time"

	"connectrpc.com/connect"
	"google.golang.org/protobuf/proto"
	"google.golang.org/protobuf/types/known/timestamppb"

	approvalsv1 "github.com/ostrium-labs/loams-mobile/mock/gen/loams/approvals/v1"
	operationsv1 "github.com/ostrium-labs/loams-mobile/mock/gen/loams/operations/v1"
	"github.com/ostrium-labs/loams-mobile/mock/internal/decision"
	"github.com/ostrium-labs/loams-mobile/mock/internal/jose"
)

// ProofMaxAge bounds a decision proof's iat in either direction.
const ProofMaxAge = 5 * time.Minute

type approvalService struct{ s *Server }

func clone(a *approvalsv1.Approval) *approvalsv1.Approval {
	return proto.Clone(a).(*approvalsv1.Approval)
}

func approvalMatcher(envs []string, states []approvalsv1.ApprovalState) func(*approvalsv1.Approval) bool {
	if len(states) == 0 {
		states = []approvalsv1.ApprovalState{approvalsv1.ApprovalState_APPROVAL_STATE_PENDING}
	}
	return func(a *approvalsv1.Approval) bool {
		if len(envs) > 0 && !slices.Contains(envs, a.GetEnvironment().GetId()) {
			return false
		}
		return slices.Contains(states, a.State)
	}
}

func (a approvalService) ListApprovals(_ context.Context, req *connect.Request[approvalsv1.ListApprovalsRequest]) (*connect.Response[approvalsv1.ListApprovalsResponse], error) {
	a.s.expireDue()
	match := approvalMatcher(req.Msg.Environments, req.Msg.States)
	out := &approvalsv1.ListApprovalsResponse{}
	for _, ap := range a.s.approvals.List() {
		if match(ap) {
			out.Approvals = append(out.Approvals, clone(ap))
		}
	}
	return connect.NewResponse(out), nil
}

func (a approvalService) GetApproval(_ context.Context, req *connect.Request[approvalsv1.GetApprovalRequest]) (*connect.Response[approvalsv1.GetApprovalResponse], error) {
	a.s.expireDue()
	ap, ok := a.s.approvals.Get(req.Msg.ApprovalId)
	if !ok {
		return nil, fail(connect.CodeNotFound, "", "no such approval")
	}
	return connect.NewResponse(&approvalsv1.GetApprovalResponse{Approval: clone(ap)}), nil
}

func (a approvalService) WatchApprovals(ctx context.Context, req *connect.Request[approvalsv1.WatchApprovalsRequest], stream *connect.ServerStream[approvalsv1.WatchApprovalsResponse]) error {
	a.s.expireDue()
	return runWatch(ctx, a.s, a.s.approvals, req.Msg.ResumeCursor, sink[*approvalsv1.Approval]{
		id:    func(x *approvalsv1.Approval) string { return x.Id },
		match: approvalMatcher(req.Msg.Environments, req.Msg.States),
		snapshot: func(items []*approvalsv1.Approval, cur string, reset bool) error {
			return stream.Send(&approvalsv1.WatchApprovalsResponse{Event: &approvalsv1.WatchApprovalsResponse_Snapshot{Snapshot: &approvalsv1.ApprovalSnapshot{Approvals: items}}, Cursor: cur, SnapshotReset: reset})
		},
		upsert: func(x *approvalsv1.Approval, cur string) error {
			return stream.Send(&approvalsv1.WatchApprovalsResponse{Event: &approvalsv1.WatchApprovalsResponse_Upsert{Upsert: x}, Cursor: cur})
		},
		remove: func(id, cur string) error {
			return stream.Send(&approvalsv1.WatchApprovalsResponse{Event: &approvalsv1.WatchApprovalsResponse_Remove{Remove: id}, Cursor: cur})
		},
		heartbeat: func(cur string) error {
			return stream.Send(&approvalsv1.WatchApprovalsResponse{Event: &approvalsv1.WatchApprovalsResponse_Heartbeat{Heartbeat: &approvalsv1.Heartbeat{}}, Cursor: cur})
		},
	})
}

// expireDue moves pending approvals past expires_at to EXPIRED.
func (s *Server) expireDue() {
	now := s.now()
	for _, ap := range s.approvals.List() {
		if ap.State == approvalsv1.ApprovalState_APPROVAL_STATE_PENDING && ap.ExpiresAt != nil && now.After(ap.ExpiresAt.AsTime()) {
			_, _ = s.approvals.Update(ap.Id, func(cur *approvalsv1.Approval) (*approvalsv1.Approval, error) {
				next := clone(cur)
				next.State = approvalsv1.ApprovalState_APPROVAL_STATE_EXPIRED
				next.Revision++
				return next, nil
			})
		}
	}
}

func decisionWord(d approvalsv1.DecisionKind) string {
	if d == approvalsv1.DecisionKind_DECISION_KIND_APPROVE {
		return "approve"
	}
	return "reject"
}

// checkProof verifies a decision proof against the caller's decision key
// (AP0 Ruling 7): ES256 over the canonical claims, the request's id,
// revision and decision, a fresh iat and an unused jti.
func (s *Server) checkProof(c caller, m *approvalsv1.DecideApprovalRequest) error {
	if m.DecisionProof == "" {
		return fail(connect.CodePermissionDenied, "step_up_required", "this decision needs a proof from a paired device's decision key")
	}
	if c.device == nil || c.device.decisionKey == nil {
		return fail(connect.CodePermissionDenied, "step_up_required", "this session has no registered decision key")
	}
	_, payload, err := jose.VerifyES256(m.DecisionProof, c.device.decisionKey)
	if err != nil {
		return fail(connect.CodePermissionDenied, "decision_proof_invalid", err.Error())
	}
	var claims struct {
		ApprovalID string `json:"approval_id"`
		Decision   string `json:"decision"`
		Iat        int64  `json:"iat"`
		Jti        string `json:"jti"`
		Revision   uint64 `json:"revision"`
	}
	if err := json.Unmarshal(payload, &claims); err != nil {
		return fail(connect.CodePermissionDenied, "decision_proof_invalid", "claims are not JSON")
	}
	want := decision.Claims{ApprovalID: m.ApprovalId, Revision: m.Revision, Decision: decisionWord(m.Decision), Iat: claims.Iat, Jti: claims.Jti}
	if string(want.Canonical()) != string(payload) {
		return fail(connect.CodePermissionDenied, "decision_proof_invalid", "claims are not the canonical form of this request")
	}
	age := s.now().Sub(time.Unix(claims.Iat, 0))
	if age > ProofMaxAge || age < -ProofMaxAge {
		return fail(connect.CodePermissionDenied, "decision_proof_invalid", "iat is not within 5 minutes")
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	if claims.Jti == "" || s.jtis[claims.Jti] {
		return fail(connect.CodePermissionDenied, "decision_proof_invalid", "jti was already used")
	}
	s.jtis[claims.Jti] = true
	return nil
}

func (a approvalService) DecideApproval(ctx context.Context, req *connect.Request[approvalsv1.DecideApprovalRequest]) (*connect.Response[approvalsv1.DecideApprovalResponse], error) {
	s := a.s
	m := req.Msg
	c := callerFrom(ctx)
	if m.IdempotencyKey != "" {
		s.mu.Lock()
		prev, ok := s.idem[m.IdempotencyKey].(*approvalsv1.DecideApprovalResponse)
		s.mu.Unlock()
		if ok {
			return connect.NewResponse(prev), nil
		}
	}
	s.expireDue()
	cur, ok := s.approvals.Get(m.ApprovalId)
	if !ok {
		return nil, fail(connect.CodeNotFound, "", "no such approval")
	}
	switch cur.State {
	case approvalsv1.ApprovalState_APPROVAL_STATE_PENDING:
	case approvalsv1.ApprovalState_APPROVAL_STATE_EXPIRED:
		return nil, fail(connect.CodeFailedPrecondition, "approval_expired", "this approval expired")
	default:
		return nil, fail(connect.CodeFailedPrecondition, "approval_already_decided", "this approval was already decided")
	}
	if m.Revision != cur.Revision {
		return nil, fail(connect.CodeFailedPrecondition, "", "the approval changed; refresh it", "current_revision", strconv.FormatUint(cur.Revision, 10))
	}
	if m.Decision == approvalsv1.DecisionKind_DECISION_KIND_UNSPECIFIED {
		return nil, fail(connect.CodeInvalidArgument, "", "decision is required")
	}
	if m.Decision == approvalsv1.DecisionKind_DECISION_KIND_REJECT && m.Reason == "" {
		return nil, fail(connect.CodeInvalidArgument, "", "a rejection needs a reason")
	}
	if !cur.GetPolicy().GetRequesterMayApprove() && m.Decision == approvalsv1.DecisionKind_DECISION_KIND_APPROVE {
		if cur.GetRequestedBy().GetId() == c.user.Id {
			return nil, fail(connect.CodePermissionDenied, "", "you requested this operation, so you cannot approve it")
		}
		for _, p := range cur.ActorChain {
			if p.Id == c.user.Id {
				return nil, fail(connect.CodePermissionDenied, "", "an agent acting for you requested this, so you cannot approve it")
			}
		}
	}
	for _, d := range cur.Decisions {
		if d.GetBy().GetId() == c.user.Id {
			return nil, fail(connect.CodeFailedPrecondition, "approval_already_decided", "you already decided this approval")
		}
	}
	if err := s.checkProof(c, m); err != nil {
		return nil, err
	}
	next, err := s.approvals.Update(m.ApprovalId, func(cur *approvalsv1.Approval) (*approvalsv1.Approval, error) {
		if cur.Revision != m.Revision || cur.State != approvalsv1.ApprovalState_APPROVAL_STATE_PENDING {
			return nil, errors.New("raced")
		}
		next := clone(cur)
		d := &approvalsv1.Decision{By: c.user, Decision: m.Decision, Reason: m.Reason, At: timestamppb.New(s.now())}
		if c.device != nil {
			d.DeviceId = c.device.proto.Id
		}
		next.Decisions = append(next.Decisions, d)
		approvals := 0
		for _, x := range next.Decisions {
			if x.Decision == approvalsv1.DecisionKind_DECISION_KIND_APPROVE {
				approvals++
			}
		}
		switch {
		case m.Decision == approvalsv1.DecisionKind_DECISION_KIND_REJECT:
			next.State = approvalsv1.ApprovalState_APPROVAL_STATE_REJECTED
		case uint32(approvals) >= max(1, next.GetPolicy().GetRequiredApprovals()):
			next.State = approvalsv1.ApprovalState_APPROVAL_STATE_APPROVED
		}
		next.Revision++
		return next, nil
	})
	if err != nil {
		return nil, fail(connect.CodeAborted, "", "the approval changed while deciding; retry")
	}
	s.settleOperation(next)
	resp := &approvalsv1.DecideApprovalResponse{Approval: clone(next)}
	if m.IdempotencyKey != "" {
		s.mu.Lock()
		s.idem[m.IdempotencyKey] = resp
		s.mu.Unlock()
	}
	s.cfg.Log.Printf("approval %s %s by %s (now %s)", next.Id, decisionWord(m.Decision), c.user.Id, next.State)
	return connect.NewResponse(resp), nil
}

// settleOperation moves the operation an approval gates.
func (s *Server) settleOperation(a *approvalsv1.Approval) {
	var state operationsv1.OperationState
	switch a.State {
	case approvalsv1.ApprovalState_APPROVAL_STATE_APPROVED:
		state = operationsv1.OperationState_OPERATION_STATE_RUNNING
	case approvalsv1.ApprovalState_APPROVAL_STATE_REJECTED:
		state = operationsv1.OperationState_OPERATION_STATE_CANCELED
	default:
		return
	}
	_, _ = s.operations.Update(a.OperationId, func(cur *operationsv1.Operation) (*operationsv1.Operation, error) {
		// Only an operation still waiting on this approval moves; a canceled
		// one never starts again.
		if cur.State != operationsv1.OperationState_OPERATION_STATE_AWAITING_APPROVAL || cur.ApprovalId != a.Id {
			return nil, errors.New("operation is not waiting on this approval")
		}
		next := proto.Clone(cur).(*operationsv1.Operation)
		next.State = state
		next.ApprovalId = ""
		next.UpdatedAt = timestamppb.New(s.now())
		if state == operationsv1.OperationState_OPERATION_STATE_RUNNING {
			next.Progress = &operationsv1.Progress{Total: 100, Message: "Approved; starting"}
		}
		return next, nil
	})
}
