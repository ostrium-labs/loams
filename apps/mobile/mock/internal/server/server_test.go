package server_test

import (
	"context"
	"crypto/ecdsa"
	"crypto/elliptic"
	"crypto/rand"
	"crypto/sha256"
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"net/http"
	"net/http/httptest"
	"net/url"
	"strings"
	"sync"
	"testing"
	"time"

	"connectrpc.com/connect"
	"google.golang.org/protobuf/proto"

	approvalsv1 "github.com/ostrium-labs/loams-mobile/mock/gen/loams/approvals/v1"
	"github.com/ostrium-labs/loams-mobile/mock/gen/loams/approvals/v1/approvalsv1connect"
	devicesv1 "github.com/ostrium-labs/loams-mobile/mock/gen/loams/devices/v1"
	"github.com/ostrium-labs/loams-mobile/mock/gen/loams/devices/v1/devicesv1connect"
	errorsv1 "github.com/ostrium-labs/loams-mobile/mock/gen/loams/errors/v1"
	instancev1 "github.com/ostrium-labs/loams-mobile/mock/gen/loams/instance/v1"
	"github.com/ostrium-labs/loams-mobile/mock/gen/loams/instance/v1/instancev1connect"
	notificationsv1 "github.com/ostrium-labs/loams-mobile/mock/gen/loams/notifications/v1"
	operationsv1 "github.com/ostrium-labs/loams-mobile/mock/gen/loams/operations/v1"
	"github.com/ostrium-labs/loams-mobile/mock/gen/loams/operations/v1/operationsv1connect"
	"github.com/ostrium-labs/loams-mobile/mock/internal/decision"
	"github.com/ostrium-labs/loams-mobile/mock/internal/jose"
	"github.com/ostrium-labs/loams-mobile/mock/internal/seal"
	"github.com/ostrium-labs/loams-mobile/mock/internal/server"
)

type clock struct {
	mu sync.Mutex
	t  time.Time
}

func (c *clock) now() time.Time      { c.mu.Lock(); defer c.mu.Unlock(); return c.t }
func (c *clock) add(d time.Duration) { c.mu.Lock(); c.t = c.t.Add(d); c.mu.Unlock() }

type env struct {
	t     *testing.T
	s     *server.Server
	url   string
	clock *clock
}

func start(t *testing.T) *env {
	t.Helper()
	c := &clock{t: time.Unix(1790899200, 0)}
	s := server.New(server.Config{Seed: true, Heartbeat: 100 * time.Millisecond, Now: c.now})
	ts := httptest.NewServer(s.Handler())
	t.Cleanup(ts.Close)
	return &env{t: t, s: s, url: ts.URL, clock: c}
}

func bearer(token string) connect.ClientOption {
	return connect.WithInterceptors(connect.UnaryInterceptorFunc(func(next connect.UnaryFunc) connect.UnaryFunc {
		return func(ctx context.Context, req connect.AnyRequest) (connect.AnyResponse, error) {
			req.Header().Set("Authorization", "Bearer "+token)
			return next(ctx, req)
		}
	}), streamAuth(token))
}

type streamAuth string

func (s streamAuth) WrapUnary(next connect.UnaryFunc) connect.UnaryFunc { return next }
func (s streamAuth) WrapStreamingClient(next connect.StreamingClientFunc) connect.StreamingClientFunc {
	return func(ctx context.Context, spec connect.Spec) connect.StreamingClientConn {
		conn := next(ctx, spec)
		conn.RequestHeader().Set("Authorization", "Bearer "+string(s))
		return conn
	}
}
func (s streamAuth) WrapStreamingHandler(next connect.StreamingHandlerFunc) connect.StreamingHandlerFunc {
	return next
}

// paired pairs a device with a fresh decision key and returns its token and key.
func (e *env) paired() (string, *ecdsa.PrivateKey) {
	e.t.Helper()
	key, _ := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
	p, _ := e.s.NewPairing("usr_alice", e.url)
	resp, body := e.redeem(url.Values{"code": {p.Code}}, key)
	if resp.StatusCode != 200 {
		e.t.Fatalf("pairing: %d %s", resp.StatusCode, body)
	}
	var tok server.TokenResponse
	_ = json.Unmarshal(body, &tok)
	return tok.AccessToken, key
}

func jwkOf(k *ecdsa.PrivateKey) string {
	b, _ := json.Marshal(jose.JWK{Kty: "EC", Crv: "P-256", X: jose.B64.EncodeToString(k.X.FillBytes(make([]byte, 32))), Y: jose.B64.EncodeToString(k.Y.FillBytes(make([]byte, 32)))})
	return string(b)
}

func (e *env) redeem(extra url.Values, key *ecdsa.PrivateKey) (*http.Response, []byte) {
	f := url.Values{"grant_type": {server.GrantPairing}, "client_id": {"loams-android"}, "device_name": {"Pixel test"}, "platform": {"android"}}
	if key != nil {
		f.Set("decision_jwk", jwkOf(key))
	}
	for k, v := range extra {
		f[k] = v
	}
	resp, err := http.PostForm(e.url+"/api/v1/oauth/token", f)
	if err != nil {
		e.t.Fatal(err)
	}
	defer resp.Body.Close()
	var b strings.Builder
	buf := make([]byte, 4096)
	for {
		n, err := resp.Body.Read(buf)
		b.Write(buf[:n])
		if err != nil {
			break
		}
	}
	return resp, []byte(b.String())
}

func proof(t *testing.T, key *ecdsa.PrivateKey, c decision.Claims) string {
	t.Helper()
	h := jose.B64.EncodeToString([]byte(`{"alg":"ES256","typ":"loams-decision+jws"}`))
	p := jose.B64.EncodeToString(c.Canonical())
	d := sha256.Sum256([]byte(h + "." + p))
	r, s, err := ecdsa.Sign(rand.Reader, key, d[:])
	if err != nil {
		t.Fatal(err)
	}
	sig := append(r.FillBytes(make([]byte, 32)), s.FillBytes(make([]byte, 32))...)
	return h + "." + p + "." + jose.B64.EncodeToString(sig)
}

func reasonOf(err error) (connect.Code, string) {
	var ce *connect.Error
	if !errors.As(err, &ce) {
		return connect.CodeUnknown, ""
	}
	for _, d := range ce.Details() {
		if v, e := d.Value(); e == nil {
			if info, ok := v.(*errorsv1.ErrorInfo); ok {
				return ce.Code(), info.Reason
			}
		}
	}
	return ce.Code(), ""
}

func TestGetInstanceNeedsNoAuthButWhoAmIDoes(t *testing.T) {
	e := start(t)
	c := instancev1connect.NewInstanceServiceClient(http.DefaultClient, e.url)
	resp, err := c.GetInstance(context.Background(), connect.NewRequest(&instancev1.GetInstanceRequest{}))
	if err != nil {
		t.Fatal(err)
	}
	if resp.Msg.InstanceId != server.InstanceID || resp.Msg.Issuer != e.url {
		t.Fatalf("instance %v issuer %v", resp.Msg.InstanceId, resp.Msg.Issuer)
	}
	if _, err := c.WhoAmI(context.Background(), connect.NewRequest(&instancev1.WhoAmIRequest{})); connect.CodeOf(err) != connect.CodeUnauthenticated {
		t.Fatalf("WhoAmI without a token: %v", err)
	}
	authed := instancev1connect.NewInstanceServiceClient(http.DefaultClient, e.url, bearer("mock-access-usr_alice"), connect.WithProtoJSON())
	who, err := authed.WhoAmI(context.Background(), connect.NewRequest(&instancev1.WhoAmIRequest{}))
	if err != nil || who.Msg.Principal.Id != "usr_alice" {
		t.Fatalf("WhoAmI: %v %v", who, err)
	}
}

func TestJWKSContainsPinnedThumbprint(t *testing.T) {
	e := start(t)
	resp, err := http.Get(e.url + "/.well-known/jwks.json")
	if err != nil {
		t.Fatal(err)
	}
	defer resp.Body.Close()
	var set struct{ Keys []jose.JWK }
	_ = json.NewDecoder(resp.Body).Decode(&set)
	got, _ := jose.Thumbprint(set.Keys[0])
	if got != e.s.JKT() {
		t.Fatalf("jwks thumbprint %s, pairing pins %s", got, e.s.JKT())
	}
}

func TestPairingCodeIsSingleUse(t *testing.T) {
	e := start(t)
	p, _ := e.s.NewPairing("usr_alice", e.url)
	if r, b := e.redeem(url.Values{"code": {p.Code}}, nil); r.StatusCode != 200 {
		t.Fatalf("first redeem: %s", b)
	}
	if _, b := e.redeem(url.Values{"code": {p.Code}}, nil); !strings.Contains(string(b), `"loams_reason":"pairing_used"`) {
		t.Fatalf("second redeem: %s", b)
	}
}

func TestPairingCodeExpires(t *testing.T) {
	e := start(t)
	p, _ := e.s.NewPairing("usr_alice", e.url)
	e.clock.add(server.PairingTTL + time.Second)
	if _, b := e.redeem(url.Values{"code": {p.Code}}, nil); !strings.Contains(string(b), "pairing_expired") {
		t.Fatalf("expired redeem: %s", b)
	}
}

func TestUserCodeRedeemsSamePairingAndBothIsInvalid(t *testing.T) {
	e := start(t)
	p, _ := e.s.NewPairing("usr_alice", e.url)
	if _, b := e.redeem(url.Values{"code": {p.Code}, "user_code": {p.UserCode}}, nil); !strings.Contains(string(b), "invalid_request") {
		t.Fatalf("both: %s", b)
	}
	if r, b := e.redeem(url.Values{"user_code": {p.UserCode}}, nil); r.StatusCode != 200 {
		t.Fatalf("user_code: %s", b)
	}
	if _, b := e.redeem(url.Values{"code": {p.Code}}, nil); !strings.Contains(string(b), "pairing_used") {
		t.Fatalf("code after user_code: %s", b)
	}
}

func TestUserCodeBurnsAfterFiveFailures(t *testing.T) {
	e := start(t)
	p, _ := e.s.NewPairing("usr_alice", e.url)
	for i := 0; i < 5; i++ {
		e.redeem(url.Values{"user_code": {"00000000"}}, nil)
	}
	if _, b := e.redeem(url.Values{"user_code": {p.UserCode}}, nil); !strings.Contains(string(b), "pairing_used") {
		t.Fatalf("after 5 failures: %s", b)
	}
}

func decide(e *env, token string, m *approvalsv1.DecideApprovalRequest) (*approvalsv1.Approval, error) {
	c := approvalsv1connect.NewApprovalServiceClient(http.DefaultClient, e.url, bearer(token))
	resp, err := c.DecideApproval(context.Background(), connect.NewRequest(m))
	if err != nil {
		return nil, err
	}
	return resp.Msg.Approval, nil
}

func TestDecisionWithProofApprovesAndStartsOperation(t *testing.T) {
	e := start(t)
	tok, key := e.paired()
	claims := decision.Claims{ApprovalID: "apr_drop_logs", Revision: 1, Decision: "approve", Iat: e.clock.now().Unix(), Jti: "jti-1"}
	a, err := decide(e, tok, &approvalsv1.DecideApprovalRequest{ApprovalId: "apr_drop_logs", Revision: 1, Decision: approvalsv1.DecisionKind_DECISION_KIND_APPROVE, Reason: "cleanup", DecisionProof: proof(t, key, claims), IdempotencyKey: "idem-1"})
	if err != nil {
		t.Fatal(err)
	}
	if a.State != approvalsv1.ApprovalState_APPROVAL_STATE_APPROVED || a.Revision != 2 {
		t.Fatalf("state %v revision %d", a.State, a.Revision)
	}
	ops := operationsv1connect.NewOperationsServiceClient(http.DefaultClient, e.url, bearer(tok))
	op, err := ops.GetOperation(context.Background(), connect.NewRequest(&operationsv1.GetOperationRequest{OperationId: "op_drop_logs"}))
	if err != nil || op.Msg.Operation.State != operationsv1.OperationState_OPERATION_STATE_RUNNING {
		t.Fatalf("operation: %v %v", op, err)
	}
	// The same idempotency key returns the same answer instead of a second decision.
	again, err := decide(e, tok, &approvalsv1.DecideApprovalRequest{ApprovalId: "apr_drop_logs", Revision: 1, Decision: approvalsv1.DecisionKind_DECISION_KIND_APPROVE, IdempotencyKey: "idem-1"})
	if err != nil || again.Revision != 2 {
		t.Fatalf("idempotent retry: %v %v", again, err)
	}
	// A new key is a second decision on a decided approval.
	_, err = decide(e, tok, &approvalsv1.DecideApprovalRequest{ApprovalId: "apr_drop_logs", Revision: 2, Decision: approvalsv1.DecisionKind_DECISION_KIND_APPROVE, IdempotencyKey: "idem-2"})
	if code, reason := reasonOf(err); code != connect.CodeFailedPrecondition || reason != "approval_already_decided" {
		t.Fatalf("second decision: %v %v", code, reason)
	}
}

func TestDecisionWithoutProofIsStepUpRequired(t *testing.T) {
	e := start(t)
	tok, _ := e.paired()
	_, err := decide(e, tok, &approvalsv1.DecideApprovalRequest{ApprovalId: "apr_agent_reindex", Revision: 1, Decision: approvalsv1.DecisionKind_DECISION_KIND_APPROVE})
	if code, reason := reasonOf(err); code != connect.CodePermissionDenied || reason != "step_up_required" {
		t.Fatalf("%v %v", code, reason)
	}
}

func TestDecisionProofReplayIsRefused(t *testing.T) {
	e := start(t)
	tok, key := e.paired()
	claims := decision.Claims{ApprovalID: "apr_agent_reindex", Revision: 1, Decision: "reject", Iat: e.clock.now().Unix(), Jti: "same"}
	jws := proof(t, key, claims)
	_, err := decide(e, tok, &approvalsv1.DecideApprovalRequest{ApprovalId: "apr_agent_reindex", Revision: 1, Decision: approvalsv1.DecisionKind_DECISION_KIND_REJECT, Reason: "not now", DecisionProof: "x" + jws})
	if _, reason := reasonOf(err); reason != "decision_proof_invalid" {
		t.Fatalf("tampered proof: %v", err)
	}
	if _, err := decide(e, tok, &approvalsv1.DecideApprovalRequest{ApprovalId: "apr_agent_reindex", Revision: 1, Decision: approvalsv1.DecisionKind_DECISION_KIND_REJECT, Reason: "not now", DecisionProof: jws}); err != nil {
		t.Fatal(err)
	}
	// Reusing the jti on another approval is a replay.
	other := decision.Claims{ApprovalID: "apr_restore", Revision: 2, Decision: "approve", Iat: e.clock.now().Unix(), Jti: "same"}
	_, err = decide(e, tok, &approvalsv1.DecideApprovalRequest{ApprovalId: "apr_restore", Revision: 2, Decision: approvalsv1.DecisionKind_DECISION_KIND_APPROVE, DecisionProof: proof(t, key, other)})
	if _, reason := reasonOf(err); reason != "decision_proof_invalid" {
		t.Fatalf("replayed jti: %v", err)
	}
}

func TestStaleRevisionIsFailedPrecondition(t *testing.T) {
	e := start(t)
	tok, key := e.paired()
	claims := decision.Claims{ApprovalID: "apr_restore", Revision: 1, Decision: "approve", Iat: e.clock.now().Unix(), Jti: "j"}
	_, err := decide(e, tok, &approvalsv1.DecideApprovalRequest{ApprovalId: "apr_restore", Revision: 1, Decision: approvalsv1.DecisionKind_DECISION_KIND_APPROVE, DecisionProof: proof(t, key, claims)})
	if connect.CodeOf(err) != connect.CodeFailedPrecondition {
		t.Fatalf("%v", err)
	}
}

func TestRequesterCannotApproveAndRejectNeedsReason(t *testing.T) {
	e := start(t)
	tok, key := e.paired()
	claims := decision.Claims{ApprovalID: "apr_api_key", Revision: 1, Decision: "approve", Iat: e.clock.now().Unix(), Jti: "k"}
	_, err := decide(e, tok, &approvalsv1.DecideApprovalRequest{ApprovalId: "apr_api_key", Revision: 1, Decision: approvalsv1.DecisionKind_DECISION_KIND_APPROVE, DecisionProof: proof(t, key, claims)})
	if connect.CodeOf(err) != connect.CodePermissionDenied {
		t.Fatalf("requester approving: %v", err)
	}
	_, err = decide(e, tok, &approvalsv1.DecideApprovalRequest{ApprovalId: "apr_api_key", Revision: 1, Decision: approvalsv1.DecisionKind_DECISION_KIND_REJECT})
	if connect.CodeOf(err) != connect.CodeInvalidArgument {
		t.Fatalf("reject without reason: %v", err)
	}
}

func TestExpiredApprovalIsApprovalExpired(t *testing.T) {
	e := start(t)
	tok, key := e.paired()
	e.clock.add(11 * time.Minute) // apr_agent_reindex expires after 10
	claims := decision.Claims{ApprovalID: "apr_agent_reindex", Revision: 1, Decision: "approve", Iat: e.clock.now().Unix(), Jti: "e"}
	_, err := decide(e, tok, &approvalsv1.DecideApprovalRequest{ApprovalId: "apr_agent_reindex", Revision: 1, Decision: approvalsv1.DecisionKind_DECISION_KIND_APPROVE, DecisionProof: proof(t, key, claims)})
	if code, reason := reasonOf(err); code != connect.CodeFailedPrecondition || reason != "approval_expired" {
		t.Fatalf("%v %v", code, reason)
	}
}

func watchApprovals(t *testing.T, e *env, cursor string) *connect.ServerStreamForClient[approvalsv1.WatchApprovalsResponse] {
	t.Helper()
	c := approvalsv1connect.NewApprovalServiceClient(http.DefaultClient, e.url, bearer("mock-access-usr_alice"))
	st, err := c.WatchApprovals(context.Background(), connect.NewRequest(&approvalsv1.WatchApprovalsRequest{ResumeCursor: cursor}))
	if err != nil {
		t.Fatal(err)
	}
	return st
}

func next(t *testing.T, st *connect.ServerStreamForClient[approvalsv1.WatchApprovalsResponse]) *approvalsv1.WatchApprovalsResponse {
	t.Helper()
	if !st.Receive() {
		t.Fatalf("stream ended: %v", st.Err())
	}
	return st.Msg()
}

func TestWatchSnapshotThenChangesThenHeartbeat(t *testing.T) {
	e := start(t)
	st := watchApprovals(t, e, "")
	defer st.Close()
	first := next(t, st)
	if first.GetSnapshot() == nil || len(first.GetSnapshot().Approvals) != 4 || first.Cursor == "" {
		t.Fatalf("first message: %v", first)
	}
	a := e.s.NewDemoApproval()
	for {
		m := next(t, st)
		if m.GetHeartbeat() != nil {
			continue
		}
		if m.GetUpsert().GetId() != a.Id {
			t.Fatalf("want upsert of %s, got %v", a.Id, m)
		}
		break
	}
	for m := next(t, st); m.GetHeartbeat() == nil; m = next(t, st) {
	}
}

func TestResumeAfterDropDeliversMissed(t *testing.T) {
	e := start(t)
	st := watchApprovals(t, e, "")
	cursor := next(t, st).Cursor
	e.s.DropStreams()
	for st.Receive() {
		cursor = st.Msg().Cursor
	}
	if connect.CodeOf(st.Err()) != connect.CodeUnavailable {
		t.Fatalf("dropped stream error: %v", st.Err())
	}
	_ = st.Close()
	missed := e.s.NewDemoApproval()
	st2 := watchApprovals(t, e, cursor)
	defer st2.Close()
	m := next(t, st2)
	if m.GetSnapshot() != nil || m.GetUpsert().GetId() != missed.Id {
		t.Fatalf("resume: %v", m)
	}
}

func TestExpiredCursorResetsSnapshot(t *testing.T) {
	e := start(t)
	st := watchApprovals(t, e, "999999")
	defer st.Close()
	m := next(t, st)
	if m.GetSnapshot() == nil || !m.SnapshotReset {
		t.Fatalf("%v", m)
	}
}

func TestPushIsSealedAndCarriesNoPlaintext(t *testing.T) {
	e := start(t)
	tok, _ := e.paired()
	pub, priv, _ := seal.NewKeyPair()
	dc := devicesv1connect.NewDeviceServiceClient(http.DefaultClient, e.url, bearer(tok))
	if _, err := dc.RegisterPushTarget(context.Background(), connect.NewRequest(&devicesv1.RegisterPushTargetRequest{
		Provider: devicesv1.PushProvider_PUSH_PROVIDER_FCM, TokenOrEndpoint: "fcm-test-token", AppId: "dev.loams.app", HpkePublicKey: pub,
	})); err != nil {
		t.Fatal(err)
	}
	a := e.s.NewDemoApproval()
	resp, err := http.Get(e.url + "/mock/push-log?token=fcm-test-token")
	if err != nil {
		t.Fatal(err)
	}
	defer resp.Body.Close()
	var log []server.PushRecord
	_ = json.NewDecoder(resp.Body).Decode(&log)
	if len(log) != 1 {
		t.Fatalf("push log: %v", log)
	}
	raw, _ := json.Marshal(log[0])
	if strings.Contains(string(raw), "Erase") || strings.Contains(string(raw), "production") {
		t.Fatalf("plaintext leaked into the push: %s", raw)
	}
	for k := range log[0].Data {
		if k != "v" && k != "i" && k != "n" && k != "s" {
			t.Fatalf("unexpected push field %q", k)
		}
	}
	sealed, _ := base64.StdEncoding.DecodeString(log[0].Data["s"])
	plain, err := seal.Open(priv, log[0].Data["i"], log[0].Data["n"], sealed)
	if err != nil {
		t.Fatal(err)
	}
	var n notificationsv1.Notification
	if err := proto.Unmarshal(plain, &n); err != nil || n.GetApprovalId() != a.Id {
		t.Fatalf("unsealed: %v %v", &n, err)
	}
	if _, err := seal.Open(priv, log[0].Data["i"], "ntf_other", sealed); err == nil {
		t.Fatal("opened with the wrong notification id")
	}
}

func TestRevokedDeviceIsRefused(t *testing.T) {
	e := start(t)
	tok, _ := e.paired()
	dc := devicesv1connect.NewDeviceServiceClient(http.DefaultClient, e.url, bearer(tok))
	list, err := dc.ListDevices(context.Background(), connect.NewRequest(&devicesv1.ListDevicesRequest{}))
	if err != nil || len(list.Msg.Devices) != 1 {
		t.Fatalf("%v %v", list, err)
	}
	if _, err := dc.RevokeDevice(context.Background(), connect.NewRequest(&devicesv1.RevokeDeviceRequest{DeviceId: list.Msg.Devices[0].Id})); err != nil {
		t.Fatal(err)
	}
	_, err = dc.ListDevices(context.Background(), connect.NewRequest(&devicesv1.ListDevicesRequest{}))
	if code, reason := reasonOf(err); code != connect.CodeUnauthenticated || reason != "device_revoked" {
		t.Fatalf("%v %v (%v)", code, reason, err)
	}
}

func TestFakeAuthentikChecksPKCE(t *testing.T) {
	e := start(t)
	verifier := "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"
	challenge := "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
	noRedirect := &http.Client{CheckRedirect: func(*http.Request, []*http.Request) error { return http.ErrUseLastResponse }}
	q := url.Values{"client_id": {"loams-android"}, "redirect_uri": {"dev.loams.app:/oauth2redirect"}, "state": {"s1"}, "code_challenge": {challenge}, "code_challenge_method": {"S256"}, "response_type": {"code"}}
	resp, err := noRedirect.Get(e.url + "/mock/authentik/application/o/loams/authorize/?" + q.Encode())
	if err != nil || resp.StatusCode != http.StatusFound {
		t.Fatalf("authorize: %v %v", resp, err)
	}
	loc, _ := url.Parse(resp.Header.Get("Location"))
	if loc.Scheme != "dev.loams.app" || loc.Query().Get("state") != "s1" {
		t.Fatalf("redirect %s", loc)
	}
	tok := func(v string) int {
		r, err := http.PostForm(e.url+"/mock/authentik/application/o/loams/token/", url.Values{"grant_type": {"authorization_code"}, "code": {loc.Query().Get("code")}, "code_verifier": {v}})
		if err != nil {
			t.Fatal(err)
		}
		r.Body.Close()
		return r.StatusCode
	}
	if tok("wrong") != 400 {
		t.Fatal("wrong verifier accepted")
	}
	// The code is single use even after a failed attempt.
	if tok(verifier) != 400 {
		t.Fatal("code reused")
	}
}

func TestRefusesNonLoopback(t *testing.T) {
	s := server.New(server.Config{})
	err := s.Run(context.Background(), "0.0.0.0:0")
	if err == nil || !strings.Contains(err.Error(), "loopback") {
		t.Fatalf("%v", err)
	}
}

func TestDecisionCanonicalGolden(t *testing.T) {
	var f struct {
		Cases []struct {
			Name   string
			Claims struct {
				ApprovalID string `json:"approval_id"`
				Revision   uint64
				Decision   string
				Iat        int64
				Jti        string
			}
			Canonical string
		}
	}
	readFixture(t, "decision/claims.json", &f)
	for _, c := range f.Cases {
		got := decision.Claims{ApprovalID: c.Claims.ApprovalID, Revision: c.Claims.Revision, Decision: c.Claims.Decision, Iat: c.Claims.Iat, Jti: c.Claims.Jti}.Canonical()
		if string(got) != c.Canonical {
			t.Errorf("%s:\n got %s\nwant %s", c.Name, got, c.Canonical)
		}
	}
}

func TestSealedFixtureOpens(t *testing.T) {
	var f struct {
		RecipientPrivateKey string `json:"recipient_private_key"`
		InstanceID          string `json:"instance_id"`
		NotificationID      string `json:"notification_id"`
		Sealed              string
		Plaintext           string
	}
	readFixture(t, "push/sealed.json", &f)
	priv, _ := base64.StdEncoding.DecodeString(f.RecipientPrivateKey)
	sealed, _ := base64.StdEncoding.DecodeString(f.Sealed)
	got, err := seal.Open(priv, f.InstanceID, f.NotificationID, sealed)
	if err != nil {
		t.Fatal(err)
	}
	if base64.StdEncoding.EncodeToString(got) != f.Plaintext {
		t.Fatal("plaintext differs")
	}
}

func TestThumbprintGolden(t *testing.T) {
	var f struct {
		Cases []struct {
			Jwk        jose.JWK
			Thumbprint string
		}
	}
	readFixture(t, "jwk/thumbprints.json", &f)
	for _, c := range f.Cases {
		if got, _ := jose.Thumbprint(c.Jwk); got != c.Thumbprint {
			t.Errorf("got %s want %s", got, c.Thumbprint)
		}
	}
}

func readFixture(t *testing.T, name string, v any) {
	t.Helper()
	b, err := fixtures(name)
	if err != nil {
		t.Fatal(err)
	}
	if err := json.Unmarshal(b, v); err != nil {
		t.Fatal(fmt.Errorf("%s: %w", name, err))
	}
}

func TestResumeRemovesWhatWasDecidedWhileAway(t *testing.T) {
	e := start(t)
	st := watchApprovals(t, e, "")
	cursor := next(t, st).Cursor
	_ = st.Close()
	// Decided on another device while this stream was down.
	tok, key := e.paired()
	claims := decision.Claims{ApprovalID: "apr_agent_reindex", Revision: 1, Decision: "approve", Iat: e.clock.now().Unix(), Jti: "away"}
	if _, err := decide(e, tok, &approvalsv1.DecideApprovalRequest{ApprovalId: "apr_agent_reindex", Revision: 1, Decision: approvalsv1.DecisionKind_DECISION_KIND_APPROVE, DecisionProof: proof(t, key, claims)}); err != nil {
		t.Fatal(err)
	}
	st2 := watchApprovals(t, e, cursor)
	defer st2.Close()
	if m := next(t, st2); m.GetRemove() != "apr_agent_reindex" {
		t.Fatalf("want a remove on resume, got %v", m)
	}
}

func TestCanceledOperationStaysCanceledAfterApproval(t *testing.T) {
	e := start(t)
	tok, key := e.paired()
	ops := operationsv1connect.NewOperationsServiceClient(http.DefaultClient, e.url, bearer(tok))
	if _, err := ops.CancelOperation(context.Background(), connect.NewRequest(&operationsv1.CancelOperationRequest{OperationId: "op_drop_logs"})); err != nil {
		t.Fatal(err)
	}
	claims := decision.Claims{ApprovalID: "apr_drop_logs", Revision: 1, Decision: "approve", Iat: e.clock.now().Unix(), Jti: "c"}
	if _, err := decide(e, tok, &approvalsv1.DecideApprovalRequest{ApprovalId: "apr_drop_logs", Revision: 1, Decision: approvalsv1.DecisionKind_DECISION_KIND_APPROVE, Reason: "x", DecisionProof: proof(t, key, claims)}); err != nil {
		t.Fatal(err)
	}
	op, _ := ops.GetOperation(context.Background(), connect.NewRequest(&operationsv1.GetOperationRequest{OperationId: "op_drop_logs"}))
	if op.Msg.Operation.State != operationsv1.OperationState_OPERATION_STATE_CANCELED {
		t.Fatalf("state %v", op.Msg.Operation.State)
	}
}

func TestUnregisterPushTargetLeavesNoStaleRef(t *testing.T) {
	e := start(t)
	tok, _ := e.paired()
	pub, _, _ := seal.NewKeyPair()
	dc := devicesv1connect.NewDeviceServiceClient(http.DefaultClient, e.url, bearer(tok))
	reg := &devicesv1.RegisterPushTargetRequest{Provider: devicesv1.PushProvider_PUSH_PROVIDER_FCM, TokenOrEndpoint: "t1", AppId: "dev.loams.app", HpkePublicKey: pub}
	_, _ = dc.RegisterPushTarget(context.Background(), connect.NewRequest(reg))
	r2, err := dc.RegisterPushTarget(context.Background(), connect.NewRequest(reg)) // same token replaces
	if err != nil {
		t.Fatal(err)
	}
	list, _ := dc.ListDevices(context.Background(), connect.NewRequest(&devicesv1.ListDevicesRequest{}))
	if n := len(list.Msg.Devices[0].PushTargets); n != 1 {
		t.Fatalf("%d refs after re-registering", n)
	}
	_, _ = dc.UnregisterPushTarget(context.Background(), connect.NewRequest(&devicesv1.UnregisterPushTargetRequest{PushTargetId: r2.Msg.PushTargetId}))
	list, _ = dc.ListDevices(context.Background(), connect.NewRequest(&devicesv1.ListDevicesRequest{}))
	if n := len(list.Msg.Devices[0].PushTargets); n != 0 {
		t.Fatalf("%d refs after unregistering", n)
	}
}
