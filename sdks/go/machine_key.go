// MachineKey: an agent's token from its identity provider's private-key JWT
// key (design 76 §10, D1607; plan IA1, IA4 Task 2).
//
// An agent is a machine user at the identity provider (Zitadel). Its key file
// is the JSON the provider hands out once ({"type": "serviceaccount", "keyId",
// "key", "userId"}), named by LOAMS_AGENT_KEY_FILE. The source signs an RFC
// 7523 assertion (RS256, kid = keyId, iss = sub = userId, aud = the issuer,
// valid for 5 minutes), trades it at <issuer>/oauth/v2/token for an access
// token whose audience is the Loams project
// (urn:zitadel:iam:org:project:id:<project>:aud), caches it, and trades again a
// minute before it expires or when the server answers token_expired.
//
// The private key never leaves the process. A sandboxed agent holds no key:
// agentd vends it a 15-minute token file instead (D1618).

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
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"os"
	"strings"
	"sync"
	"time"
)

const (
	// MachineKeyAssertionTTL is how long an assertion is valid.
	MachineKeyAssertionTTL = 5 * time.Minute
	// MachineKeyRefreshEarly is how long before expiry a token is traded again.
	MachineKeyRefreshEarly = time.Minute
	// JWTBearerGrant is the RFC 7523 grant type.
	JWTBearerGrant = "urn:ietf:params:oauth:grant-type:jwt-bearer"
)

// MachineKeyTokenSource trades an agent's machine key for access tokens.
type MachineKeyTokenSource struct {
	keyID, userID string
	key           *rsa.PrivateKey
	issuer, scope string
	client        *http.Client
	now           func() time.Time

	mu        sync.Mutex
	token     string
	refreshAt time.Time
}

type machineKeyFile struct {
	Type   string `json:"type"`
	KeyID  string `json:"keyId"`
	Key    string `json:"key"`
	UserID string `json:"userId"`
}

// MachineKey builds a source from a key file's bytes, for issuer and the Loams
// project's id. A nil client is http.DefaultClient.
func MachineKey(keyFile []byte, issuer, project string, client *http.Client) (*MachineKeyTokenSource, error) {
	var f machineKeyFile
	if err := json.Unmarshal(keyFile, &f); err != nil {
		return nil, fmt.Errorf("loams: not a machine key file: %w", err)
	}
	if f.Type != "" && f.Type != "serviceaccount" {
		return nil, fmt.Errorf("loams: a %q key is not a machine user's key", f.Type)
	}
	if f.KeyID == "" || f.UserID == "" {
		return nil, errors.New("loams: the machine key file has no keyId or userId")
	}
	if project == "" {
		return nil, errors.New("loams: the Loams project id (the audience) is empty")
	}
	issuer = strings.TrimRight(issuer, "/")
	if u, err := url.Parse(issuer); err != nil || (u.Scheme != "https" && u.Scheme != "http") || u.Host == "" {
		return nil, fmt.Errorf("loams: the issuer %q is not a URL", issuer)
	}
	key, err := parseRSAKey(f.Key)
	if err != nil {
		return nil, err
	}
	if client == nil {
		client = http.DefaultClient
	}
	return &MachineKeyTokenSource{
		keyID:  f.KeyID,
		userID: f.UserID,
		key:    key,
		issuer: issuer,
		scope:  "openid urn:zitadel:iam:org:project:id:" + project + ":aud",
		client: client,
		now:    time.Now,
	}, nil
}

// MachineKeyFromEnv builds a source from LOAMS_AGENT_KEY_FILE,
// LOAMS_AGENT_ISSUER and LOAMS_AGENT_PROJECT.
func MachineKeyFromEnv(client *http.Client) (*MachineKeyTokenSource, error) {
	get := func(name string) (string, error) {
		v := strings.TrimSpace(os.Getenv(name))
		if v == "" {
			return "", fmt.Errorf("loams: %s is not set", name)
		}
		return v, nil
	}
	path, err := get("LOAMS_AGENT_KEY_FILE")
	if err != nil {
		return nil, err
	}
	issuer, err := get("LOAMS_AGENT_ISSUER")
	if err != nil {
		return nil, err
	}
	project, err := get("LOAMS_AGENT_PROJECT")
	if err != nil {
		return nil, err
	}
	bytes, err := os.ReadFile(path)
	if err != nil {
		return nil, fmt.Errorf("loams: reading %s: %w", path, err)
	}
	return MachineKey(bytes, issuer, project, client)
}

func parseRSAKey(text string) (*rsa.PrivateKey, error) {
	block, _ := pem.Decode([]byte(text))
	if block == nil {
		return nil, errors.New("loams: the machine key is not PEM")
	}
	if k, err := x509.ParsePKCS1PrivateKey(block.Bytes); err == nil {
		return k, nil
	}
	k, err := x509.ParsePKCS8PrivateKey(block.Bytes)
	if err != nil {
		return nil, fmt.Errorf("loams: the machine key does not load: %w", err)
	}
	rk, ok := k.(*rsa.PrivateKey)
	if !ok {
		return nil, errors.New("loams: the machine key is not an RSA key")
	}
	return rk, nil
}

// UserID is the machine user's id: the tokens' sub.
func (s *MachineKeyTokenSource) UserID() string { return s.userID }

func b64(b []byte) string { return base64.RawURLEncoding.EncodeToString(b) }

func (s *MachineKeyTokenSource) assertion(now time.Time) (string, error) {
	header, err := json.Marshal(map[string]string{"alg": "RS256", "typ": "JWT", "kid": s.keyID})
	if err != nil {
		return "", err
	}
	claims, err := json.Marshal(map[string]any{
		"iss": s.userID,
		"sub": s.userID,
		"aud": s.issuer,
		"iat": now.Unix(),
		"exp": now.Add(MachineKeyAssertionTTL).Unix(),
	})
	if err != nil {
		return "", err
	}
	signing := b64(header) + "." + b64(claims)
	digest := sha256.Sum256([]byte(signing))
	sig, err := rsa.SignPKCS1v15(rand.Reader, s.key, crypto.SHA256, digest[:])
	if err != nil {
		return "", fmt.Errorf("loams: signing the assertion failed: %w", err)
	}
	return signing + "." + b64(sig), nil
}

func (s *MachineKeyTokenSource) exchange(ctx context.Context) error {
	started := s.now()
	assertion, err := s.assertion(started)
	if err != nil {
		return err
	}
	form := url.Values{
		"grant_type": {JWTBearerGrant},
		"scope":      {s.scope},
		"assertion":  {assertion},
	}
	req, err := http.NewRequestWithContext(ctx, http.MethodPost, s.issuer+"/oauth/v2/token", strings.NewReader(form.Encode()))
	if err != nil {
		return err
	}
	req.Header.Set("Content-Type", "application/x-www-form-urlencoded")
	res, err := s.client.Do(req)
	if err != nil {
		return fmt.Errorf("loams: the token endpoint is unreachable: %w", err)
	}
	defer res.Body.Close()
	body, err := io.ReadAll(io.LimitReader(res.Body, 1<<20))
	if err != nil {
		return err
	}
	if res.StatusCode/100 != 2 {
		return fmt.Errorf("loams: the token endpoint refused the machine key (%d): %s", res.StatusCode, strings.TrimSpace(string(body[:min(len(body), 512)])))
	}
	var answer struct {
		AccessToken string `json:"access_token"`
		ExpiresIn   *int64 `json:"expires_in"`
	}
	if err := json.Unmarshal(body, &answer); err != nil || answer.AccessToken == "" {
		return errors.New("loams: the token endpoint answered no access_token")
	}
	life := 15 * time.Minute
	if answer.ExpiresIn != nil {
		life = time.Duration(*answer.ExpiresIn) * time.Second
	}
	early := min(MachineKeyRefreshEarly, life/2)
	s.token = answer.AccessToken
	s.refreshAt = started.Add(life - early)
	return nil
}

// Token returns the cached access token, trading the key for a new one when
// it is about to expire. Concurrent callers share one trade.
func (s *MachineKeyTokenSource) Token(ctx context.Context) (string, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.token != "" && s.now().Before(s.refreshAt) {
		return s.token, nil
	}
	if err := s.exchange(ctx); err != nil {
		s.token = ""
		return "", err
	}
	return s.token, nil
}

// Refresh trades the key for a new token now.
func (s *MachineKeyTokenSource) Refresh(ctx context.Context) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	if err := s.exchange(ctx); err != nil {
		s.token = ""
		return err
	}
	return nil
}
