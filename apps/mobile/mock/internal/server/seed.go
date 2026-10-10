package server

import (
	"google.golang.org/protobuf/proto"
	"time"

	"google.golang.org/protobuf/types/known/structpb"
	"google.golang.org/protobuf/types/known/timestamppb"

	approvalsv1 "github.com/ostrium-labs/loams-mobile/mock/gen/loams/approvals/v1"
	instancev1 "github.com/ostrium-labs/loams-mobile/mock/gen/loams/instance/v1"
	notificationsv1 "github.com/ostrium-labs/loams-mobile/mock/gen/loams/notifications/v1"
	operationsv1 "github.com/ostrium-labs/loams-mobile/mock/gen/loams/operations/v1"
)

func (s *Server) seedPrincipals() {
	for _, p := range []*instancev1.Principal{
		{Id: "usr_alice", Kind: instancev1.PrincipalKind_PRINCIPAL_KIND_USER, DisplayName: "Alice Example", Email: "alice@example.com"},
		{Id: "usr_bob", Kind: instancev1.PrincipalKind_PRINCIPAL_KIND_USER, DisplayName: "Bob Example", Email: "bob@example.com"},
		{Id: "usr_carol", Kind: instancev1.PrincipalKind_PRINCIPAL_KIND_USER, DisplayName: "Carol Example", Email: "carol@example.com"},
		{Id: "agt_reindexer", Kind: instancev1.PrincipalKind_PRINCIPAL_KIND_AGENT, DisplayName: "reindex-agent"},
	} {
		s.users[p.Id] = p
	}
	s.envs = []*instancev1.Environment{
		{Id: "env_prod", Project: "search", Name: "production", Namespace: "prod", Protected: true},
		{Id: "env_staging", Project: "search", Name: "staging", Namespace: "staging"},
	}
}

func (s *Server) env(id string) *instancev1.Environment {
	for _, e := range s.envs {
		if e.Id == id {
			return e
		}
	}
	return nil
}

func (s *Server) seedDemo() {
	now := s.now()
	ts := timestamppb.New
	bob, alice, agent := s.users["usr_bob"], s.users["usr_alice"], s.users["agt_reindexer"]
	one := &approvalsv1.ApprovalPolicy{RequiredApprovals: 1, ApproverRoles: []string{"owner"}, StepUp: approvalsv1.StepUp_STEP_UP_DEVICE}
	two := &approvalsv1.ApprovalPolicy{RequiredApprovals: 2, ApproverRoles: []string{"owner"}, StepUp: approvalsv1.StepUp_STEP_UP_DEVICE}

	s.approvals.Put("apr_drop_logs", &approvalsv1.Approval{
		Id: "apr_drop_logs", Revision: 1, OperationId: "op_drop_logs", PromiseId: "prm_drop_logs",
		Kind: "collection.drop", Environment: s.env("env_prod"), RequestedBy: bob,
		Summary:     "Drop collection logs-2026 in production",
		DetailLines: []string{"Namespace: prod", "Collection: logs-2026", "4.2 million documents, 18.3 GiB", "This cannot be undone."},
		Target:      map[string]string{"namespace": "prod", "collection": "logs-2026"},
		Risk:        approvalsv1.Risk_RISK_DESTRUCTIVE, Policy: one, State: approvalsv1.ApprovalState_APPROVAL_STATE_PENDING,
		CreatedAt: ts(now.Add(-20 * time.Minute)), ExpiresAt: ts(now.Add(72 * time.Hour)), ConfirmText: "logs-2026",
	})
	s.approvals.Put("apr_agent_reindex", &approvalsv1.Approval{
		Id: "apr_agent_reindex", Revision: 1, OperationId: "op_agent_reindex", PromiseId: "prm_agent_reindex",
		Kind: "agent.action", Environment: s.env("env_staging"), RequestedBy: agent, ActorChain: []*instancev1.Principal{agent, bob},
		Summary:     "reindex-agent wants to rebuild the products index in staging",
		DetailLines: []string{"Acting for Bob Example", "Tool: collections.reindex", "Estimated 6 minutes"},
		Target:      map[string]string{"namespace": "staging", "collection": "products"},
		Risk:        approvalsv1.Risk_RISK_MEDIUM, Policy: one, State: approvalsv1.ApprovalState_APPROVAL_STATE_PENDING,
		CreatedAt: ts(now.Add(-5 * time.Minute)), ExpiresAt: ts(now.Add(10 * time.Minute)),
	})
	s.approvals.Put("apr_restore", &approvalsv1.Approval{
		Id: "apr_restore", Revision: 2, OperationId: "op_restore", PromiseId: "prm_restore",
		Kind: "restore", Environment: s.env("env_prod"), RequestedBy: bob,
		Summary:     "Restore namespace prod from the 02:00 snapshot",
		DetailLines: []string{"Snapshot: 2026-10-02T02:00Z", "Overwrites 3 collections", "Needs 2 approvals"},
		Target:      map[string]string{"namespace": "prod", "snapshot": "snap_0200"},
		Risk:        approvalsv1.Risk_RISK_HIGH, Policy: two, State: approvalsv1.ApprovalState_APPROVAL_STATE_PENDING,
		Decisions: []*approvalsv1.Decision{{By: s.users["usr_carol"], Decision: approvalsv1.DecisionKind_DECISION_KIND_APPROVE, At: ts(now.Add(-time.Minute))}},
		CreatedAt: ts(now.Add(-2 * time.Minute)), ExpiresAt: ts(now.Add(72 * time.Hour)),
	})
	s.approvals.Put("apr_api_key", &approvalsv1.Approval{
		Id: "apr_api_key", Revision: 1, OperationId: "op_api_key", PromiseId: "prm_api_key",
		Kind: "key.create", Environment: s.env("env_prod"), RequestedBy: alice,
		Summary:     "Create an API key with write access to production",
		DetailLines: []string{"Requested by you: another owner must approve it"},
		Target:      map[string]string{"scope": "prod:write"},
		Risk:        approvalsv1.Risk_RISK_HIGH, Policy: one, State: approvalsv1.ApprovalState_APPROVAL_STATE_PENDING,
		CreatedAt: ts(now.Add(-1 * time.Minute)), ExpiresAt: ts(now.Add(72 * time.Hour)),
	})

	result, _ := structpb.NewStruct(map[string]any{"bytes": 1.9e10})
	s.operations.Put("op_reindex", &operationsv1.Operation{
		Id: "op_reindex", Kind: "collection.reindex", Namespace: "staging", Target: map[string]string{"collection": "docs"},
		State: operationsv1.OperationState_OPERATION_STATE_RUNNING, Progress: &operationsv1.Progress{Fraction: proto.Float64(0.35), Done: 35, Total: 100, Message: "Rebuilding segments"},
		CreatedAt: ts(now.Add(-3 * time.Minute)), UpdatedAt: ts(now),
	})
	s.operations.Put("op_drop_logs", &operationsv1.Operation{
		Id: "op_drop_logs", Kind: "collection.drop", Namespace: "prod", Target: map[string]string{"collection": "logs-2026"},
		State: operationsv1.OperationState_OPERATION_STATE_AWAITING_APPROVAL, ApprovalId: "apr_drop_logs",
		CreatedAt: ts(now.Add(-20 * time.Minute)), UpdatedAt: ts(now.Add(-20 * time.Minute)),
	})
	s.operations.Put("op_backup", &operationsv1.Operation{
		Id: "op_backup", Kind: "namespace.backup", Namespace: "prod",
		State: operationsv1.OperationState_OPERATION_STATE_SUCCEEDED, Progress: &operationsv1.Progress{Fraction: proto.Float64(1), Done: 1, Total: 1},
		Result: result, CreatedAt: ts(now.Add(-2 * time.Hour)), UpdatedAt: ts(now.Add(-110 * time.Minute)),
	})
	s.operations.Put("op_compact", &operationsv1.Operation{
		Id: "op_compact", Kind: "collection.compact", Namespace: "staging", Target: map[string]string{"collection": "events"},
		State: operationsv1.OperationState_OPERATION_STATE_FAILED, Error: &operationsv1.OperationError{Code: "object_store_unavailable", Message: "RustFS answered 503 for 3 minutes"},
		CreatedAt: ts(now.Add(-50 * time.Minute)), UpdatedAt: ts(now.Add(-45 * time.Minute)),
	})

	s.notifications.Put("ntf_0001", &notificationsv1.Notification{
		Id: "ntf_0001", Category: notificationsv1.Category_CATEGORY_APPROVALS, CloudeventType: "io.loams.dev.approval.requested.v1",
		Subject: "apr_drop_logs", Title: "Approval needed", Body: "Bob Example wants to drop logs-2026 in production",
		Ref: &notificationsv1.Notification_ApprovalId{ApprovalId: "apr_drop_logs"}, EnvironmentId: "env_prod", CreatedAt: ts(now.Add(-20 * time.Minute)),
	})
	s.notifications.Put("ntf_0002", &notificationsv1.Notification{
		Id: "ntf_0002", Category: notificationsv1.Category_CATEGORY_OPERATIONS, CloudeventType: "io.loams.dev.operation.failed.v1",
		Subject: "op_compact", Title: "Compaction failed", Body: "events in staging: the object store was unavailable",
		Ref: &notificationsv1.Notification_OperationId{OperationId: "op_compact"}, EnvironmentId: "env_staging", CreatedAt: ts(now.Add(-45 * time.Minute)),
	})
}
