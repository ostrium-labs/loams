// Package server is the loams-mobile mock: the app protos of design §37 §8
// over Connect (and gRPC and gRPC-Web, which connect-go serves on the same
// port), plus the HTTP endpoints the apps need around them (the OAuth token
// endpoint for pairing, the JWKS) and a few /mock/ controls for testing.
//
// It is stateful: deciding an approval changes it, and the change reaches
// every open Watch stream. Tokens are fake fixed strings; nothing here is a
// secret. It binds loopback only (D111): the Android emulator reaches the
// host's loopback at 10.0.2.2, and a USB device through `adb reverse`.
package server

import (
	"context"
	"crypto/ecdsa"
	"crypto/ed25519"
	"crypto/sha256"
	"errors"
	"fmt"
	"log"
	"net"
	"net/http"
	"sync"
	"time"

	"connectrpc.com/connect"

	approvalsv1 "github.com/ostrium-labs/loams-mobile/mock/gen/loams/approvals/v1"
	"github.com/ostrium-labs/loams-mobile/mock/gen/loams/approvals/v1/approvalsv1connect"
	devicesv1 "github.com/ostrium-labs/loams-mobile/mock/gen/loams/devices/v1"
	"github.com/ostrium-labs/loams-mobile/mock/gen/loams/devices/v1/devicesv1connect"
	errorsv1 "github.com/ostrium-labs/loams-mobile/mock/gen/loams/errors/v1"
	instancev1 "github.com/ostrium-labs/loams-mobile/mock/gen/loams/instance/v1"
	"github.com/ostrium-labs/loams-mobile/mock/gen/loams/instance/v1/instancev1connect"
	notificationsv1 "github.com/ostrium-labs/loams-mobile/mock/gen/loams/notifications/v1"
	"github.com/ostrium-labs/loams-mobile/mock/gen/loams/notifications/v1/notificationsv1connect"
	operationsv1 "github.com/ostrium-labs/loams-mobile/mock/gen/loams/operations/v1"
	"github.com/ostrium-labs/loams-mobile/mock/gen/loams/operations/v1/operationsv1connect"
	"github.com/ostrium-labs/loams-mobile/mock/internal/hub"
	"github.com/ostrium-labs/loams-mobile/mock/internal/jose"
)

// InstanceID is the mock's fixed instance id.
const InstanceID = "01J9Z3MOCK0000000000000000"

// Config configures a mock.
type Config struct {
	// PublicURL is the issuer printed in the startup pairing QR, as the
	// phone will reach it (the emulator: http://10.0.2.2:8084).
	PublicURL string
	// Heartbeat is the interval of empty Watch messages (15 s in AP0).
	Heartbeat time.Duration
	// Tick advances running operations; 0 disables it.
	Tick time.Duration
	// Seed loads the demo data.
	Seed bool
	// Now is the clock; nil means time.Now.
	Now func() time.Time
	// Log receives one line per notable event; nil discards.
	Log *log.Logger
}

type device struct {
	proto       *devicesv1.Device
	userID      string
	decisionKey *ecdsa.PublicKey
}

type pairing struct {
	code      string
	userCode  string
	userID    string
	expiresAt time.Time
	used      bool
	failures  int
}

type pushTarget struct {
	id       string
	deviceID string
	req      *devicesv1.RegisterPushTargetRequest
}

// PushRecord is what a push gateway would receive for one device: no
// plaintext, only the sealed payload and routing fields.
type PushRecord struct {
	Seq        int               `json:"seq"`
	TargetID   string            `json:"target_id"`
	Provider   string            `json:"provider"`
	Token      string            `json:"token"`
	AppID      string            `json:"app_id"`
	CollapseID string            `json:"collapse_id"`
	Priority   string            `json:"priority"`
	Data       map[string]string `json:"data"`
}

// Server is a running mock's state.
type Server struct {
	cfg Config

	instanceKey ed25519.PrivateKey
	instanceJWK jose.JWK
	jkt         string

	approvals     *hub.Hub[*approvalsv1.Approval]
	operations    *hub.Hub[*operationsv1.Operation]
	notifications *hub.Hub[*notificationsv1.Notification]

	mu          sync.Mutex
	users       map[string]*instancev1.Principal
	envs        []*instancev1.Environment
	pairings    map[string]*pairing
	userCodes   map[string]string
	devices     map[string]*device
	access      map[string]string // access token -> device id, or "user:<id>"
	refresh     map[string]string // refresh token -> device id
	jtis        map[string]bool
	pushTargets map[string]*pushTarget
	pushLog     []PushRecord
	prefs       *devicesv1.NotificationPreferences
	idem        map[string]any
	drop        chan struct{}
	nextID      int
}

// New builds a mock with its demo data.
func New(cfg Config) *Server {
	if cfg.Now == nil {
		cfg.Now = time.Now
	}
	if cfg.Heartbeat == 0 {
		cfg.Heartbeat = 15 * time.Second
	}
	if cfg.Log == nil {
		cfg.Log = log.New(discard{}, "", 0)
	}
	seed := sha256.Sum256([]byte("loams-mobile mock instance key (test only, not a secret)"))
	key := ed25519.NewKeyFromSeed(seed[:])
	jwk := jose.JWK{Kty: "OKP", Crv: "Ed25519", X: jose.B64.EncodeToString(key.Public().(ed25519.PublicKey)), Alg: "EdDSA", Use: "sig"}
	jkt, _ := jose.Thumbprint(jwk)
	jwk.Kid = jkt
	s := &Server{
		cfg:           cfg,
		instanceKey:   key,
		instanceJWK:   jwk,
		jkt:           jkt,
		approvals:     hub.New[*approvalsv1.Approval](1000),
		operations:    hub.New[*operationsv1.Operation](1000),
		notifications: hub.New[*notificationsv1.Notification](1000),
		users:         map[string]*instancev1.Principal{},
		pairings:      map[string]*pairing{},
		userCodes:     map[string]string{},
		devices:       map[string]*device{},
		access:        map[string]string{},
		refresh:       map[string]string{},
		jtis:          map[string]bool{},
		pushTargets:   map[string]*pushTarget{},
		prefs:         &devicesv1.NotificationPreferences{ApprovalsBypassQuietHours: true},
		idem:          map[string]any{},
		drop:          make(chan struct{}),
	}
	s.seedPrincipals()
	if cfg.Seed {
		s.seedDemo()
	}
	return s
}

// JKT is the instance key thumbprint a pairing pins.
func (s *Server) JKT() string { return s.jkt }

// Handler returns every route of the mock.
func (s *Server) Handler() http.Handler {
	mux := http.NewServeMux()
	opts := connect.WithInterceptors(s.authInterceptor())
	mux.Handle(instancev1connect.NewInstanceServiceHandler(instanceService{s}, opts))
	mux.Handle(devicesv1connect.NewDeviceServiceHandler(deviceService{s}, opts))
	mux.Handle(approvalsv1connect.NewApprovalServiceHandler(approvalService{s}, opts))
	mux.Handle(operationsv1connect.NewOperationsServiceHandler(operationsService{s}, opts))
	mux.Handle(notificationsv1connect.NewNotificationServiceHandler(notificationService{s}, opts))
	s.mountHTTP(mux)
	return withHost(mux)
}

// Run serves on addr until ctx ends. It refuses a non-loopback address.
func (s *Server) Run(ctx context.Context, addr string) error {
	host, _, err := net.SplitHostPort(addr)
	if err != nil {
		return err
	}
	if ip := net.ParseIP(host); host != "localhost" && (ip == nil || !ip.IsLoopback()) {
		return fmt.Errorf("refusing to listen on %s: the mock binds loopback only (use adb reverse for a USB device)", addr)
	}
	ln, err := net.Listen("tcp", addr)
	if err != nil {
		return err
	}
	p := new(http.Protocols)
	p.SetHTTP1(true)
	p.SetUnencryptedHTTP2(true)
	srv := &http.Server{Handler: s.Handler(), Protocols: p, ReadHeaderTimeout: 10 * time.Second}
	if s.cfg.Tick > 0 {
		go s.tickOperations(ctx)
	}
	go func() {
		<-ctx.Done()
		shutdown, cancel := context.WithTimeout(context.Background(), 2*time.Second)
		defer cancel()
		_ = srv.Shutdown(shutdown)
	}()
	if err := srv.Serve(ln); err != nil && !errors.Is(err, http.ErrServerClosed) {
		return err
	}
	return nil
}

func (s *Server) now() time.Time { return s.cfg.Now() }

func (s *Server) newID(prefix string) string {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.nextID++
	return fmt.Sprintf("%s_%04d", prefix, s.nextID)
}

// DropStreams ends every open Watch stream with Unavailable, as a flaky
// network or a proxy would; clients resume from their cursor.
func (s *Server) DropStreams() {
	s.mu.Lock()
	defer s.mu.Unlock()
	close(s.drop)
	s.drop = make(chan struct{})
}

func (s *Server) dropSignal() <-chan struct{} {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.drop
}

// fail builds a Connect error carrying a loams.errors.v1.ErrorInfo.
func fail(code connect.Code, reason, msg string, meta ...string) error {
	err := connect.NewError(code, errors.New(msg))
	info := &errorsv1.ErrorInfo{Reason: reason, Metadata: map[string]string{}}
	for i := 0; i+1 < len(meta); i += 2 {
		info.Metadata[meta[i]] = meta[i+1]
	}
	if d, derr := connect.NewErrorDetail(info); derr == nil {
		err.AddDetail(d)
	}
	return err
}

type discard struct{}

func (discard) Write(p []byte) (int, error) { return len(p), nil }

type hostKey struct{}

func withHost(h http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		h.ServeHTTP(w, r.WithContext(context.WithValue(r.Context(), hostKey{}, r.Host)))
	})
}

// issuerFrom is the issuer as the caller reached the mock.
func issuerFrom(ctx context.Context, fallback string) string {
	if h, ok := ctx.Value(hostKey{}).(string); ok && h != "" {
		return "http://" + h
	}
	return fallback
}
