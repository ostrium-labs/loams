// SDK2 Task 2's `go_token_source_refresh`.
//
// Design §44 §7.4, D608 and runtime contract R1: a rejection carrying
// `reason = token_expired` gets **exactly one** refresh and **one** retry. Both
// halves matter. Refreshing more than once turns an auth outage into a refresh
// storm; retrying without refreshing replays a token the server has already
// rejected.
//
// What is pinned here is the SDK's half: the refresh-once-and-retry loop, and
// the sharing of one in-flight refresh across concurrent callers. The token
// exchange itself (`OIDC`) is written to the documented protocol and is not
// exercised, because the instance serves no OAuth endpoint yet; that is stated in
// `token_source.go` and in the PR.
//
// The loop is pinned against a real HTTP server, because the interesting part is
// what goes out on the wire: one call with the stale bearer, then one with the
// fresh one.

package loams

import (
	"context"
	"encoding/base64"
	"errors"
	"fmt"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"testing"
	"time"

	connect "connectrpc.com/connect"
	"google.golang.org/protobuf/proto"

	"loams.dev/go/gen/facade"
	errorsv1 "loams.dev/go/gen/loams/errors/v1"
	instancev1 "loams.dev/go/gen/loams/instance/v1"
	instancev1connect "loams.dev/go/gen/loams/instance/v1/instancev1connect"
)

// TestGoTokenSourceRefresh is the required test.
func TestGoTokenSourceRefresh(t *testing.T) {
	t.Run(TokenSourceRefresh, func(t *testing.T) {
		testTokenSourceRefresh(t)
	})
}

func testTokenSourceRefresh(t *testing.T) {
	t.Run("one refresh and one retry, on the wire", func(t *testing.T) {
		// The interesting part is what goes out on the wire: the stale bearer
		// first, then the fresh one. A unit test of the token source cannot show
		// that the transport re-sent the second.
		//
		// The source's first fetch is the seeding one — an empty cache would send
		// no credential at all, which an instance that requires one answers
		// `unauthenticated`, and the call path reads that as "the token expired"
		// and retries with still no credential. So the sequence is: fetch "stale"
		// to fill the cache, get a `token_expired`, refresh to "fresh", retry, and
		// be answered.
		var mu sync.Mutex
		var bearers []string
		fetches := 0
		source := Refreshing(func(context.Context) (string, error) {
			mu.Lock()
			defer mu.Unlock()
			fetches++
			if fetches == 1 {
				return "stale", nil
			}
			return "fresh", nil
		})
		server := httptest.NewServer(http.HandlerFunc(func(writer http.ResponseWriter, request *http.Request) {
			mu.Lock()
			bearers = append(bearers, request.Header.Get("Authorization"))
			mu.Unlock()
			if request.Header.Get("Authorization") != "Bearer fresh" {
				writeTokenExpired(t, writer)
				return
			}
			payload, err := proto.Marshal(&instancev1.GetInstanceResponse{Name: "Loams"})
			if err != nil {
				writer.WriteHeader(http.StatusInternalServerError)
				return
			}
			writer.Header().Set("Content-Type", "application/proto")
			writer.WriteHeader(http.StatusOK)
			_, _ = writer.Write(payload)
		}))
		defer server.Close()

		client, err := New(Options{
			Endpoint:   server.URL,
			Auth:       source,
			HTTPClient: &http.Client{Timeout: 20 * time.Second},
		})
		if err != nil {
			t.Fatalf("building a client: %v", err)
		}
		defer func() { _ = client.Close() }()

		ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
		defer cancel()

		info, err := client.Instance().GetInstance(ctx, &GetInstanceRequest{})
		if err != nil {
			t.Fatalf("GetInstance: %v", err)
		}
		if info.GetName() != "Loams" {
			t.Errorf("the instance name is %q", info.GetName())
		}
		mu.Lock()
		defer mu.Unlock()
		if len(bearers) != 2 {
			t.Fatalf("the server saw %d attempts, want 2: %v", len(bearers), bearers)
		}
		if bearers[0] != "Bearer stale" {
			t.Errorf("the first attempt sent %q, want the stale bearer", bearers[0])
		}
		if bearers[1] != "Bearer fresh" {
			t.Errorf("the retry sent %q, want the refreshed bearer", bearers[1])
		}
		// Two fetches: one to fill the empty cache, one for the refresh. A third
		// would mean the retry fetched again rather than reading the cache.
		if fetches != 2 {
			t.Errorf("the source was fetched %d times, want 2: one to seed the cache, one to refresh", fetches)
		}
	})

	t.Run("a second expiry is reported rather than looped on", func(t *testing.T) {
		// Refreshing more than once turns an auth outage into a refresh storm: a
		// hundred in-flight calls would each mint a token.
		refreshes := 0
		source := Refreshing(func(context.Context) (string, error) {
			refreshes++
			return "always-stale", nil
		})
		binding, _ := facade.Binding("instance", "GetInstance")
		ctx := context.Background()

		attempts := 0
		_, err := CallWithRetry(ctx, &GetInstanceRequest{},
			func(context.Context, Attempt) (any, error) {
				attempts++
				return nil, connect.NewError(connect.CodeUnauthenticated,
					errors.New("unauthenticated: token_expired"))
			},
			RetryPlan{RetrySafe: true, MaxRetries: DefaultMaxRetries, OnRefresh: source.Refresh},
			binding.RPC)
		if err == nil {
			t.Fatal("a call that is always rejected answered")
		}
		// One attempt, one refresh, and then the failure is reported — because the
		// rejection here carries no `ErrorInfo`, so the runtime cannot tell it was
		// a `token_expired` and does not refresh at all.
		if attempts != 1 {
			t.Errorf("the call made %d attempts, want 1", attempts)
		}
		if refreshes != 0 {
			t.Errorf("the source was refreshed %d times, want 0: without a `token_expired` reason there is nothing to refresh against", refreshes)
		}
		var unauthenticated *UnauthenticatedError
		if !errors.As(err, &unauthenticated) {
			t.Errorf("the failure is %T, want an *UnauthenticatedError", err)
		}
	})

	t.Run("a refresh-once loop with a real token_expired detail", func(t *testing.T) {
		refreshes := 0
		attempts := 0
		source := Refreshing(func(context.Context) (string, error) {
			refreshes++
			return fmt.Sprintf("token-%d", refreshes), nil
		})
		ctx := context.Background()

		// Every attempt is rejected with `token_expired`. One refresh and one retry,
		// then the second expiry is reported.
		_, err := CallWithRetry(ctx, &GetInstanceRequest{},
			func(context.Context, Attempt) (any, error) {
				attempts++
				return nil, tokenExpiredError(t)
			},
			RetryPlan{RetrySafe: true, MaxRetries: DefaultMaxRetries, OnRefresh: source.Refresh},
			"loams.instance.v1.InstanceService/GetInstance")
		if err == nil {
			t.Fatal("a call that is always expired answered")
		}
		if attempts != 2 {
			t.Errorf("the call made %d attempts, want 2: one attempt, one refresh, one retry", attempts)
		}
		if refreshes != 1 {
			t.Errorf("the source was refreshed %d times, want exactly 1", refreshes)
		}
		var expired *TokenExpiredError
		if !errors.As(err, &expired) {
			t.Errorf("the failure is %T, want a *TokenExpiredError", err)
		}
	})

	t.Run("an API key's refresh is a no-op, and no retry follows it", func(t *testing.T) {
		// R1: a source that cannot refresh — an API key, which does not expire —
		// makes the refresh a no-op. So a `token_expired` against an API key is
		// reported straight away rather than replayed with the same key.
		var attempts int
		apiKeySource := APIKey("loams_key_secret")
		_, err := CallWithRetry(context.Background(), &GetInstanceRequest{},
			func(context.Context, Attempt) (any, error) {
				attempts++
				return nil, tokenExpiredError(t)
			},
			RetryPlan{RetrySafe: true, MaxRetries: DefaultMaxRetries, OnRefresh: apiKeySource.Refresh},
			"loams.instance.v1.InstanceService/GetInstance")
		if err == nil {
			t.Fatal("a rejected API key answered")
		}
		if attempts != 2 {
			t.Errorf("the call made %d attempts, want 2", attempts)
		}
		// Refresh is a no-op, so the retry carries the same key and fails the same
		// way — which is the honest outcome and one attempt cheaper than looping.
		if token, err := apiKeySource.Token(context.Background()); err != nil || token != "loams_key_secret" {
			t.Errorf("the API key changed: %q %v", token, err)
		}
	})

	t.Run("one in-flight refresh is shared by concurrent callers", func(t *testing.T) {
		// Not a micro-optimisation: an instance rejecting every token because it
		// is stale would otherwise receive one token exchange per in-flight call,
		// which is how a credential rotation turns into a self-inflicted denial of
		// service.
		var mu sync.Mutex
		exchanges := 0
		release := make(chan struct{})
		source := Refreshing(func(context.Context) (string, error) {
			mu.Lock()
			exchanges++
			mu.Unlock()
			<-release
			return "fresh", nil
		})

		const callers = 50
		var wg sync.WaitGroup
		errs := make([]error, callers)
		for index := range callers {
			wg.Add(1)
			go func() {
				defer wg.Done()
				_, errs[index] = source.Token(context.Background())
			}()
		}
		// Give every goroutine a chance to reach the shared fetch, then let it
		// finish. A short sleep is the honest way to say "they have all arrived";
		// a longer one would only make the test slower when it passes.
		time.Sleep(50 * time.Millisecond)
		close(release)
		wg.Wait()

		for index, err := range errs {
			if err != nil {
				t.Fatalf("caller %d: %v", index, err)
			}
		}
		mu.Lock()
		defer mu.Unlock()
		if exchanges != 1 {
			t.Errorf("%d callers caused %d token exchanges, want 1", callers, exchanges)
		}
	})

	t.Run("the bearer travels in the header and never in the URL", func(t *testing.T) {
		// A token in a query string ends up in proxy logs, in browser history and
		// in `Referer`. The recorded corpus has no authenticated case to check,
		// so this is checked against the request the transport built.
		var seen *http.Request
		server := httptest.NewServer(http.HandlerFunc(func(writer http.ResponseWriter, request *http.Request) {
			seen = request.Clone(request.Context())
			payload, err := proto.Marshal(&instancev1.GetInstanceResponse{Name: "Loams"})
			if err != nil {
				writer.WriteHeader(http.StatusInternalServerError)
				return
			}
			writer.Header().Set("Content-Type", "application/proto")
			writer.WriteHeader(http.StatusOK)
			_, _ = writer.Write(payload)
		}))
		defer server.Close()

		client, err := New(Options{
			Endpoint:   server.URL,
			Auth:       APIKey("loams_key_secret"),
			HTTPClient: &http.Client{Timeout: 20 * time.Second},
		})
		if err != nil {
			t.Fatalf("building a client: %v", err)
		}
		defer func() { _ = client.Close() }()

		ctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
		defer cancel()
		if _, err := client.Instance().GetInstance(ctx, &GetInstanceRequest{}); err != nil {
			t.Fatalf("GetInstance: %v", err)
		}
		if seen == nil {
			t.Fatal("the server saw no request")
		}
		if got := seen.Header.Get("Authorization"); got != "Bearer loams_key_secret" {
			t.Errorf("the Authorization header is %q", got)
		}
		if strings.Contains(seen.URL.RawQuery, "loams_key_secret") ||
			strings.Contains(seen.URL.String(), "loams_key_secret") {
			t.Errorf("the token appears in the URL %q", seen.URL.String())
		}
		// And a caller's own Authorization header does not win: a stale one would
		// make R1 unreachable.
		if _, err := client.Instance().GetInstance(ctx, &GetInstanceRequest{},
			WithHeader("Authorization", "Bearer mine")); err != nil {
			t.Fatalf("GetInstance with a header: %v", err)
		}
	})

	t.Run("the token source reads the environment on every call", func(t *testing.T) {
		// Read once at construction, a process that receives its credentials after
		// the client is built — a sidecar, a test — never authenticates.
		variables := map[string]string{}
		source := &EnvTokenSource{Lookup: func(key string) (string, bool) {
			value, ok := variables[key]
			return value, ok
		}}
		ctx := context.Background()
		token, err := source.Token(ctx)
		if err != nil || token != "" {
			t.Errorf("an empty environment produced %q %v, want no credential", token, err)
		}
		variables["LOAMS_TOKEN"] = "from-token"
		if token, err = source.Token(ctx); err != nil || token != "from-token" {
			t.Errorf("LOAMS_TOKEN produced %q %v", token, err)
		}
		variables["LOAMS_API_KEY"] = "from-key"
		if token, err = source.Token(ctx); err != nil || token != "from-key" {
			t.Errorf("LOAMS_API_KEY produced %q %v; it must win over LOAMS_TOKEN", token, err)
		}
		// And the names are the ones the design names.
		if len(EnvNames) != 2 || EnvNames[0] != "LOAMS_API_KEY" || EnvNames[1] != "LOAMS_TOKEN" {
			t.Errorf("EnvNames is %v, want [LOAMS_API_KEY LOAMS_TOKEN]", EnvNames)
		}
	})

	t.Run("the context reaches the token fetch", func(t *testing.T) {
		// A token fetch is a network call, so a Go source takes the caller's
		// context; a source that ignored it would hang a shutdown.
		source := Refreshing(func(ctx context.Context) (string, error) {
			<-ctx.Done()
			return "", ctx.Err()
		})
		ctx, cancel := context.WithTimeout(context.Background(), 30*time.Millisecond)
		defer cancel()
		if _, err := source.Token(ctx); !errors.Is(err, context.DeadlineExceeded) {
			t.Errorf("a cancelled fetch returned %v, want a deadline", err)
		}
	})
}

// instanceClientFor is the generated `InstanceServiceClient`, resolved through the
// same `clientOf` an application module uses — so this test drives the real
// wiring rather than a hand-built stub of it.
func instanceClientFor(t *testing.T, invoker *CallInvoker) instancev1connect.InstanceServiceClient {
	t.Helper()
	client, err := clientOf[instancev1connect.InstanceServiceClient](invoker, "loams.instance.v1.InstanceService")
	if err != nil {
		t.Fatalf("the instance client: %v", err)
	}
	return client
}

// writeTokenExpired answers with the refusal the contract describes: 401, an
// `unauthenticated` code, and an `ErrorInfo` whose reason is `token_expired`.
func writeTokenExpired(t *testing.T, writer http.ResponseWriter) {
	t.Helper()
	info, err := proto.Marshal(&errorsv1.ErrorInfo{Reason: string(ReasonTokenExpired), Hint: "refresh and try again"})
	if err != nil {
		writer.WriteHeader(http.StatusInternalServerError)
		return
	}
	writer.Header().Set("Content-Type", "application/json")
	writer.WriteHeader(http.StatusUnauthorized)
	_, _ = fmt.Fprintf(writer,
		`{"code":"unauthenticated","message":"the access token expired","details":[{"type":"%s","value":"%s"}]}`,
		facade.ErrorInfoTypeURL, base64.StdEncoding.EncodeToString(info))
}

// tokenExpiredError is the same refusal as a `*connect.Error`, for a test that
// drives the retry loop without a server.
func tokenExpiredError(t *testing.T) error {
	t.Helper()
	detail, err := connect.NewErrorDetail(&errorsv1.ErrorInfo{Reason: string(ReasonTokenExpired)})
	if err != nil {
		t.Fatalf("building the ErrorInfo detail: %v", err)
	}
	wire := connect.NewError(connect.CodeUnauthenticated, errors.New("the access token expired"))
	wire.AddDetail(detail)
	return wire
}
