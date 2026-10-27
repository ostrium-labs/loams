package loams

import (
	"context"
	"crypto"
	"crypto/rand"
	"crypto/rsa"
	"crypto/sha256"
	"crypto/x509"
	"encoding/base64"
	"encoding/json"
	"encoding/pem"
	"fmt"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"
)

func testKeyFile(t *testing.T, kind string) ([]byte, *rsa.PublicKey) {
	t.Helper()
	key, err := rsa.GenerateKey(rand.Reader, 2048)
	if err != nil {
		t.Fatal(err)
	}
	pemKey := pem.EncodeToMemory(&pem.Block{Type: "RSA PRIVATE KEY", Bytes: x509.MarshalPKCS1PrivateKey(key)})
	file, _ := json.Marshal(map[string]string{"type": kind, "keyId": "k-1", "key": string(pemKey), "userId": "agent-7"})
	return file, &key.PublicKey
}

type tokenEndpoint struct {
	calls     atomic.Int32
	expiresIn int
	fail      bool
	mu        sync.Mutex
	last      map[string]string
}

func (e *tokenEndpoint) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	n := e.calls.Add(1)
	_ = r.ParseForm()
	e.mu.Lock()
	e.last = map[string]string{"path": r.URL.Path, "grant_type": r.PostForm.Get("grant_type"), "scope": r.PostForm.Get("scope"), "assertion": r.PostForm.Get("assertion")}
	e.mu.Unlock()
	if e.fail {
		http.Error(w, `{"error":"invalid_grant"}`, http.StatusBadRequest)
		return
	}
	fmt.Fprintf(w, `{"access_token":"at-%d","token_type":"Bearer","expires_in":%d}`, n, e.expiresIn)
}

func verifyRS256(t *testing.T, jwt string, pub *rsa.PublicKey) (map[string]any, map[string]any) {
	t.Helper()
	parts := strings.Split(jwt, ".")
	if len(parts) != 3 {
		t.Fatalf("not a JWT: %q", jwt)
	}
	sig, err := base64.RawURLEncoding.DecodeString(parts[2])
	if err != nil {
		t.Fatal(err)
	}
	digest := sha256.Sum256([]byte(parts[0] + "." + parts[1]))
	if err := rsa.VerifyPKCS1v15(pub, crypto.SHA256, digest[:], sig); err != nil {
		t.Fatalf("the assertion is not signed by the key: %v", err)
	}
	decode := func(s string) map[string]any {
		b, _ := base64.RawURLEncoding.DecodeString(s)
		var m map[string]any
		_ = json.Unmarshal(b, &m)
		return m
	}
	return decode(parts[0]), decode(parts[1])
}

func TestMachineKeyAssertionIsRFC7523(t *testing.T) {
	file, pub := testKeyFile(t, "serviceaccount")
	ep := &tokenEndpoint{expiresIn: 900}
	srv := httptest.NewServer(ep)
	defer srv.Close()
	s, err := MachineKey(file, srv.URL+"/", "3141", srv.Client())
	if err != nil {
		t.Fatal(err)
	}
	tok, err := s.Token(context.Background())
	if err != nil || tok != "at-1" {
		t.Fatalf("token = %q, %v", tok, err)
	}
	ep.mu.Lock()
	last := ep.last
	ep.mu.Unlock()
	if last["path"] != "/oauth/v2/token" || last["grant_type"] != JWTBearerGrant {
		t.Fatalf("request: %v", last)
	}
	if last["scope"] != "openid urn:zitadel:iam:org:project:id:3141:aud" {
		t.Fatalf("scope: %q", last["scope"])
	}
	header, claims := verifyRS256(t, last["assertion"], pub)
	if header["kid"] != "k-1" || header["alg"] != "RS256" {
		t.Fatalf("header: %v", header)
	}
	if claims["iss"] != "agent-7" || claims["sub"] != "agent-7" || claims["aud"] != srv.URL {
		t.Fatalf("claims: %v", claims)
	}
	if life := claims["exp"].(float64) - claims["iat"].(float64); life != MachineKeyAssertionTTL.Seconds() {
		t.Fatalf("assertion life %v", life)
	}
}

func TestMachineKeyCachesAndRefreshes(t *testing.T) {
	file, _ := testKeyFile(t, "serviceaccount")
	ep := &tokenEndpoint{expiresIn: 900}
	srv := httptest.NewServer(ep)
	defer srv.Close()
	s, _ := MachineKey(file, srv.URL, "p", srv.Client())
	clock := time.Now()
	s.now = func() time.Time { return clock }
	ctx := context.Background()
	for range 3 {
		if tok, _ := s.Token(ctx); tok != "at-1" {
			t.Fatalf("cached token %q", tok)
		}
	}
	clock = clock.Add(900*time.Second - MachineKeyRefreshEarly + time.Second)
	if tok, _ := s.Token(ctx); tok != "at-2" {
		t.Fatalf("traded again a minute before expiry, got %q", tok)
	}
	if err := s.Refresh(ctx); err != nil {
		t.Fatal(err)
	}
	if tok, _ := s.Token(ctx); tok != "at-3" {
		t.Fatalf("after refresh %q", tok)
	}
	// Concurrent callers share one trade.
	clock = clock.Add(time.Hour)
	var wg sync.WaitGroup
	for range 8 {
		wg.Go(func() { _, _ = s.Token(ctx) })
	}
	wg.Wait()
	if got := ep.calls.Load(); got != 4 {
		t.Fatalf("%d trades, want 4", got)
	}
}

func TestMachineKeyRefusals(t *testing.T) {
	file, _ := testKeyFile(t, "serviceaccount")
	app, _ := testKeyFile(t, "application")
	for name, build := range map[string]func() error{
		"application key": func() error { _, err := MachineKey(app, "https://i", "p", nil); return err },
		"not json":        func() error { _, err := MachineKey([]byte("{"), "https://i", "p", nil); return err },
		"no project":      func() error { _, err := MachineKey(file, "https://i", "", nil); return err },
		"bad issuer":      func() error { _, err := MachineKey(file, "auth.example", "p", nil); return err },
		"bad pem": func() error {
			_, err := MachineKey([]byte(`{"keyId":"k","key":"nope","userId":"u"}`), "https://i", "p", nil)
			return err
		},
	} {
		if build() == nil {
			t.Errorf("%s: accepted", name)
		}
	}
	ep := &tokenEndpoint{fail: true}
	srv := httptest.NewServer(ep)
	defer srv.Close()
	s, _ := MachineKey(file, srv.URL, "p", srv.Client())
	if tok, err := s.Token(context.Background()); err == nil || tok != "" || !strings.Contains(err.Error(), "invalid_grant") {
		t.Fatalf("a refused trade: %q, %v", tok, err)
	}
}
