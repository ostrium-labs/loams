package server

import (
	"crypto/sha256"
	"encoding/json"
	"fmt"
	"net/http"
	"net/url"
	"strconv"
	"strings"
	"time"

	"google.golang.org/protobuf/types/known/timestamppb"

	approvalsv1 "github.com/ostrium-labs/loams-mobile/mock/gen/loams/approvals/v1"
	devicesv1 "github.com/ostrium-labs/loams-mobile/mock/gen/loams/devices/v1"
	notificationsv1 "github.com/ostrium-labs/loams-mobile/mock/gen/loams/notifications/v1"
	operationsv1 "github.com/ostrium-labs/loams-mobile/mock/gen/loams/operations/v1"
	"github.com/ostrium-labs/loams-mobile/mock/internal/jose"
)

// Grant types the token endpoint accepts (§37 §7.2.2).
const (
	GrantPairing  = "urn:loams:params:oauth:grant-type:pairing"
	GrantExchange = "urn:ietf:params:oauth:grant-type:token-exchange"
	GrantRefresh  = "refresh_token"
)

// The fake Authentik lives under this prefix so browser sign-in can run
// end to end against the mock: authorize (redirects straight back with a
// code), token (checks PKCE S256) and discovery.
const authentikPrefix = "/mock/authentik/application/o/loams/"

// TokenResponse is the token endpoint's answer.
type TokenResponse struct {
	AccessToken  string `json:"access_token"`
	TokenType    string `json:"token_type"`
	ExpiresIn    int    `json:"expires_in"`
	RefreshToken string `json:"refresh_token"`
	DeviceID     string `json:"device_id"`
}

func writeJSON(w http.ResponseWriter, status int, v any) {
	w.Header().Set("Content-Type", "application/json")
	w.Header().Set("Cache-Control", "no-store")
	w.WriteHeader(status)
	_ = json.NewEncoder(w).Encode(v)
}

func oauthError(w http.ResponseWriter, code, reason, desc string) {
	writeJSON(w, http.StatusBadRequest, map[string]string{"error": code, "error_description": desc, "loams_reason": reason})
}

func (s *Server) mountHTTP(mux *http.ServeMux) {
	mux.HandleFunc("GET /healthz", func(w http.ResponseWriter, _ *http.Request) { _, _ = w.Write([]byte("ok\n")) })
	mux.HandleFunc("GET /.well-known/jwks.json", func(w http.ResponseWriter, _ *http.Request) {
		writeJSON(w, 200, map[string]any{"keys": []jose.JWK{s.instanceJWK}})
	})
	mux.HandleFunc("POST /api/v1/oauth/token", s.token)

	mux.HandleFunc("GET "+authentikPrefix+".well-known/openid-configuration", func(w http.ResponseWriter, r *http.Request) {
		base := "http://" + r.Host + authentikPrefix
		writeJSON(w, 200, map[string]any{
			"issuer":                                base,
			"authorization_endpoint":                base + "authorize/",
			"token_endpoint":                        base + "token/",
			"jwks_uri":                              base + "jwks/",
			"response_types_supported":              []string{"code"},
			"subject_types_supported":               []string{"public"},
			"id_token_signing_alg_values_supported": []string{"EdDSA"},
			"code_challenge_methods_supported":      []string{"S256"},
		})
	})
	mux.HandleFunc("GET "+authentikPrefix+"authorize/", s.fakeAuthorize)
	mux.HandleFunc("POST "+authentikPrefix+"token/", s.fakeAuthentikToken)

	// Test controls. Not part of any Loams API.
	mux.HandleFunc("GET /mock/pairing", func(w http.ResponseWriter, r *http.Request) {
		issuer := "http://" + r.Host
		if q := r.URL.Query().Get("issuer"); q != "" {
			issuer = q
		}
		p, _ := s.NewPairing("usr_alice", issuer)
		writeJSON(w, 200, p)
	})
	mux.HandleFunc("POST /mock/approvals", func(w http.ResponseWriter, _ *http.Request) {
		a := s.NewDemoApproval()
		writeJSON(w, 200, map[string]string{"approval_id": a.Id})
	})
	mux.HandleFunc("POST /mock/drop-streams", func(w http.ResponseWriter, _ *http.Request) {
		s.DropStreams()
		w.WriteHeader(http.StatusNoContent)
	})
	mux.HandleFunc("POST /mock/tick", func(w http.ResponseWriter, _ *http.Request) {
		s.Tick()
		w.WriteHeader(http.StatusNoContent)
	})
	mux.HandleFunc("GET /mock/push-log", func(w http.ResponseWriter, r *http.Request) {
		after, _ := strconv.Atoi(r.URL.Query().Get("after"))
		token := r.URL.Query().Get("token")
		s.mu.Lock()
		out := []PushRecord{}
		for _, p := range s.pushLog {
			if p.Seq > after && (token == "" || p.Token == token) {
				out = append(out, p)
			}
		}
		s.mu.Unlock()
		writeJSON(w, 200, out)
	})
}

func (s *Server) token(w http.ResponseWriter, r *http.Request) {
	if err := r.ParseForm(); err != nil {
		oauthError(w, "invalid_request", "", "not a form")
		return
	}
	f := r.PostForm
	switch f.Get("grant_type") {
	case GrantPairing:
		s.redeemPairing(w, f)
	case GrantExchange:
		s.exchange(w, f)
	case GrantRefresh:
		s.refreshGrant(w, f)
	default:
		oauthError(w, "unsupported_grant_type", "", "grant_type "+f.Get("grant_type"))
	}
}

func (s *Server) redeemPairing(w http.ResponseWriter, f url.Values) {
	code, userCode := f.Get("code"), f.Get("user_code")
	if (code == "") == (userCode == "") {
		oauthError(w, "invalid_request", "", "send exactly one of code or user_code")
		return
	}
	s.mu.Lock()
	if userCode != "" {
		c, ok := s.userCodes[userCode]
		if !ok {
			// Count the failure against every live pairing (a mock simplification
			// of AP0's per-instance limit) and burn one after 5.
			for _, p := range s.pairings {
				if !p.used {
					p.failures++
					if p.failures >= 5 {
						p.used = true
					}
				}
			}
			s.mu.Unlock()
			oauthError(w, "invalid_grant", "pairing_expired", "unknown user code")
			return
		}
		code = c
	}
	p := s.pairings[code]
	switch {
	case p == nil || s.now().After(p.expiresAt):
		s.mu.Unlock()
		oauthError(w, "invalid_grant", "pairing_expired", "this pairing code expired")
		return
	case p.used:
		s.mu.Unlock()
		oauthError(w, "invalid_grant", "pairing_used", "this pairing code was already used")
		return
	}
	p.used = true
	s.mu.Unlock()
	s.issueForDevice(w, p.userID, f)
}

func (s *Server) exchange(w http.ResponseWriter, f url.Values) {
	if !strings.HasPrefix(f.Get("subject_token"), "mock-authentik-") {
		oauthError(w, "invalid_grant", "", "subject_token is not a token from the mock Authentik")
		return
	}
	s.issueForDevice(w, "usr_alice", f)
}

// issueForDevice registers the device described by the form and returns its
// tokens. A real gateway also binds them to the DPoP key; the mock only
// records whether a DPoP header was sent.
func (s *Server) issueForDevice(w http.ResponseWriter, userID string, f url.Values) {
	var decisionKey jose.JWK
	thumb := ""
	if raw := f.Get("decision_jwk"); raw != "" {
		if err := json.Unmarshal([]byte(raw), &decisionKey); err != nil {
			oauthError(w, "invalid_request", "", "decision_jwk is not JSON")
			return
		}
		if _, err := jose.P256(decisionKey); err != nil {
			oauthError(w, "invalid_request", "", err.Error())
			return
		}
		thumb, _ = jose.Thumbprint(decisionKey)
	}
	platform := devicesv1.Platform_PLATFORM_ANDROID
	if f.Get("platform") == "ios" {
		platform = devicesv1.Platform_PLATFORM_IOS
	}
	id := s.newID("dev")
	dev := &device{
		userID: userID,
		proto: &devicesv1.Device{
			Id: id, Name: f.Get("device_name"), Platform: platform, Model: f.Get("model"), AppVersion: f.Get("app_version"),
			CreatedAt: timestamppb.New(s.now()), LastSeenAt: timestamppb.New(s.now()), DecisionKeyThumbprint: thumb,
		},
	}
	if thumb != "" {
		dev.decisionKey, _ = jose.P256(decisionKey)
	}
	resp := TokenResponse{AccessToken: "mock-access-" + id, TokenType: "DPoP", ExpiresIn: 3600, RefreshToken: randomToken("mock-refresh-"), DeviceID: id}
	s.mu.Lock()
	s.devices[id] = dev
	s.access[resp.AccessToken] = id
	s.refresh[resp.RefreshToken] = id
	s.mu.Unlock()
	s.cfg.Log.Printf("device %s paired for %s (decision key %s)", id, userID, thumb)
	writeJSON(w, 200, resp)
}

func (s *Server) refreshGrant(w http.ResponseWriter, f url.Values) {
	old := f.Get("refresh_token")
	s.mu.Lock()
	defer s.mu.Unlock()
	id, ok := s.refresh[old]
	if !ok {
		oauthError(w, "invalid_grant", "", "unknown refresh token")
		return
	}
	delete(s.refresh, old)
	resp := TokenResponse{AccessToken: "mock-access-" + id, TokenType: "DPoP", ExpiresIn: 3600, RefreshToken: randomToken("mock-refresh-"), DeviceID: id}
	s.access[resp.AccessToken] = id
	s.refresh[resp.RefreshToken] = id
	writeJSON(w, 200, resp)
}

// fakeAuthorize stands in for Authentik's login page: it redirects straight
// back to the app with a code bound to the PKCE challenge.
func (s *Server) fakeAuthorize(w http.ResponseWriter, r *http.Request) {
	q := r.URL.Query()
	redirect, err := url.Parse(q.Get("redirect_uri"))
	if err != nil || redirect.Scheme == "" {
		http.Error(w, "redirect_uri is required", http.StatusBadRequest)
		return
	}
	if q.Get("code_challenge_method") != "S256" || q.Get("code_challenge") == "" {
		http.Error(w, "PKCE S256 is required", http.StatusBadRequest)
		return
	}
	code := randomToken("mock-authz-")
	s.mu.Lock()
	s.idem["authz:"+code] = q.Get("code_challenge")
	s.mu.Unlock()
	back := redirect.Query()
	back.Set("code", code)
	back.Set("state", q.Get("state"))
	redirect.RawQuery = back.Encode()
	http.Redirect(w, r, redirect.String(), http.StatusFound)
}

func (s *Server) fakeAuthentikToken(w http.ResponseWriter, r *http.Request) {
	if err := r.ParseForm(); err != nil {
		oauthError(w, "invalid_request", "", "not a form")
		return
	}
	code, verifier := r.PostForm.Get("code"), r.PostForm.Get("code_verifier")
	s.mu.Lock()
	challenge, ok := s.idem["authz:"+code].(string)
	delete(s.idem, "authz:"+code)
	s.mu.Unlock()
	sum := sha256.Sum256([]byte(verifier))
	if !ok || jose.B64.EncodeToString(sum[:]) != challenge {
		oauthError(w, "invalid_grant", "", "unknown code or PKCE verifier mismatch")
		return
	}
	// No id_token: AppAuth would validate one (issuer, audience, nonce), and the app only needs
	// the access token to exchange at the gateway.
	writeJSON(w, 200, map[string]any{
		"access_token": "mock-authentik-" + code[len("mock-authz-"):], "token_type": "Bearer", "expires_in": 300,
	})
}

// NewDemoApproval creates a pending approval, as an agent asking for
// something would, and notifies (and pushes) it.
func (s *Server) NewDemoApproval() *approvalsv1.Approval {
	now := s.now()
	id := s.newID("apr")
	opID := "op_" + id[len("apr_"):]
	a := &approvalsv1.Approval{
		Id: id, Revision: 1, OperationId: opID, PromiseId: "prm_" + id,
		Kind: "namespace.erasure", Environment: s.env("env_prod"), RequestedBy: s.users["usr_bob"],
		Summary:     "Erase user 4711's data in production",
		DetailLines: []string{"GDPR erasure request", "Touches 3 collections", "Requested at " + now.UTC().Format(time.Kitchen)},
		Target:      map[string]string{"namespace": "prod", "subject": "user-4711"},
		Risk:        approvalsv1.Risk_RISK_HIGH,
		Policy:      &approvalsv1.ApprovalPolicy{RequiredApprovals: 1, StepUp: approvalsv1.StepUp_STEP_UP_DEVICE},
		State:       approvalsv1.ApprovalState_APPROVAL_STATE_PENDING,
		CreatedAt:   timestamppb.New(now), ExpiresAt: timestamppb.New(now.Add(72 * time.Hour)),
	}
	s.operations.Put(opID, &operationsv1.Operation{
		Id: opID, Kind: "namespace.erasure", Namespace: "prod", State: operationsv1.OperationState_OPERATION_STATE_AWAITING_APPROVAL,
		ApprovalId: id, CreatedAt: timestamppb.New(now), UpdatedAt: timestamppb.New(now),
	})
	s.approvals.Put(id, a)
	s.Notify(&notificationsv1.Notification{
		Category: notificationsv1.Category_CATEGORY_APPROVALS, CloudeventType: "io.loams.dev.approval.requested.v1",
		Subject: id, Title: "Approval needed", Body: fmt.Sprintf("Bob Example: %s", a.Summary),
		Ref: &notificationsv1.Notification_ApprovalId{ApprovalId: id}, EnvironmentId: "env_prod",
	})
	return a
}
