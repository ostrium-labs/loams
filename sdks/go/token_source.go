// Token sources (design §44 §7.4, D608; runtime contract R1).
//
// A TokenSource returns a bearer and can be asked for a new one. Tokens travel
// in `Authorization: Bearer` and **never** in a URL: a query string ends up in
// proxy logs, in browser history and in `Referer`. A `401` carrying
// `reason = token_expired` triggers one refresh and one retry; that logic lives
// in the call path, so a source stays a source.
//
// In Go a source is an interface with one method plus an optional refresh, and
// the caller's context reaches it, because fetching a token is a network call
// and Go will not let a call be cancelled without one:
//
//	type TokenSource interface {
//	    Token(ctx context.Context) (string, error)
//	    Refresh(ctx context.Context) error
//	}
//
// Refresh is required rather than optional because Go has no way to ask "does
// this interface have a method" without reflection, and an API key — which has
// nothing to refresh — is expressed by returning nil from Refresh. See
// APIKeyTokenSource.

package loams

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"os"
	"strings"
	"sync"
)

// TokenSource is where a call's bearer comes from.
//
// Token is called once per attempt, so a source may return a different token
// each time. Refresh is called only after the server said the current token
// expired; a source whose token does not expire (an API key) returns nil and
// the runtime does not retry.
type TokenSource interface {
	// Token is the bearer to send, or "" to send no credential at all.
	Token(ctx context.Context) (string, error)
	// Refresh fetches a new token after the server reported the current one
	// expired. Returning nil from a source that cannot refresh makes the
	// runtime's refresh a no-op (R1).
	Refresh(ctx context.Context) error
}

// APIKeyTokenSource is a Loams API key. The key does not expire, so there is
// nothing to refresh and Refresh is a no-op.
type APIKeyTokenSource struct {
	key string
}

// APIKey is a Loams API key: what a script or a CI job has.
func APIKey(key string) *APIKeyTokenSource {
	return &APIKeyTokenSource{key: key}
}

// Token returns the key.
func (s *APIKeyTokenSource) Token(context.Context) (string, error) { return s.key, nil }

// Refresh is a no-op: an API key does not expire, so R1's refresh is the no-op
// the contract describes.
func (s *APIKeyTokenSource) Refresh(context.Context) error { return nil }

// StaticTokenSource is a bearer that is already valid, for a caller that
// manages its own.
type StaticTokenSource struct {
	token string
}

// StaticToken is a bearer the caller manages.
func StaticToken(token string) *StaticTokenSource {
	return &StaticTokenSource{token: token}
}

// Token returns the token.
func (s *StaticTokenSource) Token(context.Context) (string, error) { return s.token, nil }

// Refresh is a no-op: the caller owns this token's lifetime.
func (s *StaticTokenSource) Refresh(context.Context) error { return nil }

// EnvTokenSource reads the environment. The variables are named
// `LOAMS_API_KEY` and `LOAMS_TOKEN`, after design §44 §7.4.
type EnvTokenSource struct {
	// Lookup reads one variable. It is a field so a test can supply an
	// environment without touching the process's own, and so a caller who
	// keeps its configuration somewhere other than the environment can say so.
	// `os.LookupEnv` satisfies it.
	Lookup func(key string) (string, bool)
}

// EnvToken reads `LOAMS_API_KEY`, then `LOAMS_TOKEN`, then nothing.
//
// The environment is read on **every** call rather than once at construction,
// so a process that receives its credentials after the client is built — a
// sidecar, a test — still authenticates.
func EnvToken() *EnvTokenSource {
	return &EnvTokenSource{Lookup: os.LookupEnv}
}

// EnvNames are the variables EnvTokenSource reads, in order.
var EnvNames = []string{"LOAMS_API_KEY", "LOAMS_TOKEN"}

// Token returns the first of EnvNames that is set.
func (s *EnvTokenSource) Token(context.Context) (string, error) {
	lookup := s.Lookup
	if lookup == nil {
		lookup = os.LookupEnv
	}
	for _, name := range EnvNames {
		if value, ok := lookup(name); ok && value != "" {
			return value, nil
		}
	}
	return "", nil
}

// Refresh is a no-op: an environment variable is not something the SDK can
// renew. A caller who wants a refreshed token points this at their own source.
func (s *EnvTokenSource) Refresh(context.Context) error { return nil }

// RefreshingTokenSource caches a token and calls a fetch function when it is
// asked to refresh. This is the shape every refreshing source has.
//
// One in-flight refresh is shared by concurrent callers, so a burst of `401`s
// produces **one** token exchange rather than one per request. That is not a
// micro-optimisation: an instance that is rejecting every token because it is
// stale would otherwise be hit with one exchange per in-flight call, which is
// how a credential rotation turns into a self-inflicted denial of service.
type RefreshingTokenSource struct {
	mu       sync.Mutex
	cache    string
	inFlight chan error
	// Fetch mints a new token. It is called with the caller's context.
	Fetch func(ctx context.Context) (string, error)
}

// Refreshing is a source that caches a token and calls fetchToken when asked to
// refresh.
func Refreshing(fetchToken func(ctx context.Context) (string, error)) *RefreshingTokenSource {
	return &RefreshingTokenSource{Fetch: fetchToken}
}

// Token returns the cached token, fetching one first if the cache is empty.
//
// A source whose cache starts empty would send no credential at all, and an
// instance that requires one answers `unauthenticated` — which the call path
// treats as "the token expired" and retries, with still no credential. So the
// first Token fetches.
func (s *RefreshingTokenSource) Token(ctx context.Context) (string, error) {
	s.mu.Lock()
	cached := s.cache
	s.mu.Unlock()
	if cached != "" {
		return cached, nil
	}
	if err := s.Refresh(ctx); err != nil {
		return "", err
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.cache, nil
}

// Refresh mints a new token, sharing one in-flight fetch across callers.
func (s *RefreshingTokenSource) Refresh(ctx context.Context) error {
	s.mu.Lock()
	if s.inFlight != nil {
		inFlight := s.inFlight
		s.mu.Unlock()
		return wait(ctx, inFlight)
	}
	// The context of the first caller bounds the shared fetch, which is the
	// right trade: if that caller goes away the others did not, but they will
	// fall through to their own refresh on the next `401` rather than hanging
	// on a fetch nobody is waiting for.
	shared := make(chan error, 1)
	s.inFlight = shared
	s.mu.Unlock()

	go func() {
		token, err := s.Fetch(ctx)
		if err == nil {
			s.mu.Lock()
			s.cache = token
			s.mu.Unlock()
		}
		shared <- err
		s.mu.Lock()
		s.inFlight = nil
		s.mu.Unlock()
		close(shared)
	}()
	return wait(ctx, shared)
}

func wait(ctx context.Context, done <-chan error) error {
	select {
	case err := <-done:
		return err
	case <-ctx.Done():
		return ctx.Err()
	}
}

// OIDCExchange is the RFC 8693 token exchange a person signed in through
// Authentik needs (design §44 §7.4, D608; §19 §5.2).
//
// The instance's `/oauth/token` protocol endpoint takes the identity token and
// answers with a Loams access token, which is then cached until the server says
// it expired.
//
// **Not exercised by the conformance suite.** The instance serves no OAuth
// endpoint yet (the auth plan, MT, and API1 Task 7 build it), so this is
// written to the documented request and response and cannot be run against a
// live server. `TestGoTokenSourceRefresh` covers the caching and the
// refresh-once-and-retry behaviour that `Refreshing` implements, which is the
// part the SDK owns.
type OIDCExchange struct {
	// Endpoint is the instance's `/oauth/token` endpoint.
	Endpoint string
	// ClientID is the public OAuth client id; the gateway exchanges the token,
	// so the client secret is never involved (D447/D449).
	ClientID string
	// SubjectToken mints the current identity token, from the host's OIDC
	// session.
	SubjectToken func(ctx context.Context) (string, error)
	// Do posts the request. It defaults to the transport's HTTP client; a test
	// and a host with its own client supply their own.
	Do func(ctx context.Context, endpoint, body string) (string, error)

	inner *RefreshingTokenSource
}

// OIDCExchangeOptions configures OIDCExchange.
type OIDCExchangeOptions struct {
	// Endpoint is the instance's `/oauth/token` endpoint.
	Endpoint string
	// ClientID is the public OAuth client id.
	ClientID string
	// SubjectToken mints the current identity token.
	SubjectToken func(ctx context.Context) (string, error)
	// HTTPClient posts the form body. Optional: `http.DefaultClient` is used.
	HTTPClient *http.Client
}

// OIDC is the RFC 8693 token exchange.
func OIDC(options OIDCExchangeOptions) *OIDCExchange {
	client := options.HTTPClient
	if client == nil {
		client = http.DefaultClient
	}
	source := &OIDCExchange{
		Endpoint:     options.Endpoint,
		ClientID:     options.ClientID,
		SubjectToken: options.SubjectToken,
	}
	source.Do = func(ctx context.Context, endpoint, body string) (string, error) {
		request, err := http.NewRequestWithContext(ctx, http.MethodPost, endpoint, strings.NewReader(body))
		if err != nil {
			return "", err
		}
		request.Header.Set("Content-Type", "application/x-www-form-urlencoded")
		response, err := client.Do(request)
		if err != nil {
			return "", err
		}
		defer response.Body.Close()
		payload, err := io.ReadAll(response.Body)
		if err != nil {
			return "", err
		}
		if response.StatusCode < 200 || response.StatusCode > 299 {
			return "", fmt.Errorf("the token exchange answered %d: %s", response.StatusCode, strings.TrimSpace(string(payload)))
		}
		var parsed struct {
			AccessToken string `json:"access_token"`
		}
		if err := json.Unmarshal(payload, &parsed); err != nil {
			return "", err
		}
		if parsed.AccessToken == "" {
			return "", errors.New("the token exchange answered no access_token")
		}
		return parsed.AccessToken, nil
	}
	source.inner = Refreshing(source.exchange)
	return source
}

// exchange posts the RFC 8693 token exchange.
func (s *OIDCExchange) exchange(ctx context.Context) (string, error) {
	if s.SubjectToken == nil {
		return "", errors.New("loams: OIDC token source has no SubjectToken")
	}
	subject, err := s.SubjectToken(ctx)
	if err != nil {
		return "", err
	}
	form := url.Values{
		"grant_type":           {"urn:ietf:params:oauth:grant-type:token-exchange"},
		"subject_token_type":   {"urn:ietf:params:oauth:token-type:id_token"},
		"requested_token_type": {"urn:ietf:params:oauth:token-type:access_token"},
		"subject_token":        {subject},
		"client_id":            {s.ClientID},
		"audience":             {s.Endpoint},
	}
	return s.Do(ctx, s.Endpoint, form.Encode())
}

// Token returns the cached Loams access token, exchanging one first if the
// cache is empty.
func (s *OIDCExchange) Token(ctx context.Context) (string, error) { return s.inner.Token(ctx) }

// Refresh exchanges a new access token.
func (s *OIDCExchange) Refresh(ctx context.Context) error { return s.inner.Refresh(ctx) }
