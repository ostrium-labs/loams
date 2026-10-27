// SDKP Part A for Go: the token corpus, the session store, R11–R14, the
// collection handle and `QueryAll`, each pinned against a stub so no server is
// needed. The canonical names are what `check-languages.mjs --suite app` looks
// for:
//
//   - go_consistency_token_merge (R4, `sdks/fixtures/tokens.json`)
//   - go_session_read_your_writes (R4)
//   - go_default_namespace (R11)
//   - go_timeout_deadline (R12)
//   - go_write_batch_chunks (R13)
//   - go_streams_subscribe_resumes (R14)
//   - go_collection_handle_delegates (SDKP §2.8)
//   - go_search_query_all_requires_tiebreak (SDKP §2.4)

package loams

import (
	"context"
	"encoding/json"
	"errors"
	"net/http"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"sync"
	"testing"
	"time"

	connect "connectrpc.com/connect"
	"google.golang.org/protobuf/types/known/structpb"

	"loams.dev/go/gen/facade"
	collectionv1 "loams.dev/go/gen/loams/collection/v1"
	graphv1 "loams.dev/go/gen/loams/graph/v1"
)

// The app tests, by canonical name.
const (
	ConsistencyTokenMerge        = "go_consistency_token_merge"
	SessionReadYourWrites        = "go_session_read_your_writes"
	DefaultNamespace             = "go_default_namespace"
	TimeoutDeadline              = "go_timeout_deadline"
	WriteBatchChunks             = "go_write_batch_chunks"
	StreamsSubscribeResumes      = "go_streams_subscribe_resumes"
	BuildersMatchCorpus          = "go_builders_match_corpus"
	CollectionHandleDelegates    = "go_collection_handle_delegates"
	SearchQueryAllRequiresTiebrk = "go_search_query_all_requires_tiebreak"
)

// documentBinding is a DocumentService binding shaped as P0a's annotations
// will generate it.
func documentBinding(name, method string, retry facade.RetryClass, keyed bool) facade.CallBinding {
	idempotency := facade.NoSideEffects
	if retry == facade.RetryManual {
		idempotency = facade.IdempotencyLevel("none")
	}
	return facade.CallBinding{
		Module: "documents", Name: name, ProtoName: name, Method: method,
		RPC:     "loams.collection.v1.DocumentService/" + method,
		Service: "loams.collection.v1.DocumentService", Package: "loams.collection.v1",
		Idempotency: idempotency, Retry: retry, Streaming: facade.Unary, TakesIdempotencyKey: keyed,
	}
}

var (
	writeBinding = documentBinding("Write", "WriteDocuments", facade.RetryManual, true)
	getBinding   = documentBinding("Get", "GetDocuments", facade.RetrySafe, false)
	graphExecute = facade.CallBinding{
		Module: "graph", Name: "Query", ProtoName: "query", Method: "Execute",
		RPC: "loams.graph.v1.GraphService/Execute", Service: "loams.graph.v1.GraphService",
		Package: "loams.graph.v1", Idempotency: facade.NoSideEffects, Retry: facade.RetrySafe, Streaming: facade.Unary,
	}
)

// stubMethod is a generated unary method that records each request and answers
// with answer.
type stubMethod[Req, Res any] struct {
	mu       sync.Mutex
	requests []*Req
	headers  []http.Header
	answer   func(n int, ctx context.Context, request *Req) (*Res, http.Header, error)
}

func (s *stubMethod[Req, Res]) call(ctx context.Context, request *connect.Request[Req]) (*connect.Response[Res], error) {
	s.mu.Lock()
	s.requests = append(s.requests, request.Msg)
	s.headers = append(s.headers, request.Header().Clone())
	n := len(s.requests)
	s.mu.Unlock()
	message, header, err := s.answer(n, ctx, request.Msg)
	if err != nil {
		return nil, err
	}
	response := connect.NewResponse(message)
	for name, values := range header {
		response.Header()[name] = values
	}
	return response, nil
}

func newTestInvoker(session ConsistencyTokenStore, defaults InvokerDefaults, maxRetries int) *CallInvoker {
	invoker := NewCallInvoker(map[string]any{}, nil, maxRetries, session)
	invoker.SetDefaults(defaults)
	return invoker
}

// ---------------------------------------------------------------------------

type tokenCorpus struct {
	Parse []struct {
		Name  string  `json:"name"`
		Text  string  `json:"text"`
		Items [][]any `json:"items"`
	} `json:"parse"`
	Merge []struct {
		Name   string `json:"name"`
		A      string `json:"a"`
		B      string `json:"b"`
		Expect string `json:"expect"`
	} `json:"merge"`
	Malformed []string `json:"malformed"`
}

func loadTokenCorpus(t *testing.T) tokenCorpus {
	t.Helper()
	raw, err := os.ReadFile(filepath.Join("..", "fixtures", "tokens.json"))
	if err != nil {
		t.Fatal(err)
	}
	var corpus tokenCorpus
	if err := json.Unmarshal(raw, &corpus); err != nil {
		t.Fatal(err)
	}
	return corpus
}

func Test_go_consistency_token_merge(t *testing.T) {
	corpus := loadTokenCorpus(t)
	if len(corpus.Parse) == 0 || len(corpus.Merge) == 0 || len(corpus.Malformed) == 0 {
		t.Fatal("the token corpus is empty")
	}
	for _, c := range corpus.Parse {
		t.Run("parse/"+c.Name, func(t *testing.T) {
			token, err := ParseConsistencyToken(c.Text)
			if err != nil {
				t.Fatal(err)
			}
			if token.String() != c.Text {
				t.Errorf("prints %q, want %q", token.String(), c.Text)
			}
			positions := token.Positions()
			if len(positions) != len(c.Items) {
				t.Fatalf("%d items, want %d", len(positions), len(c.Items))
			}
			for i, item := range c.Items {
				got := []string{
					jsonNumber(positions[i].Stream), jsonNumber(uint64(positions[i].Partition)), jsonNumber(positions[i].Offset),
				}
				want := []string{item[0].(string), jsonNumber(uint64(item[1].(float64))), item[2].(string)}
				if !slices.Equal(got, want) {
					t.Errorf("item %d is %v, want %v", i, got, want)
				}
			}
		})
	}
	for _, c := range corpus.Merge {
		t.Run("merge/"+c.Name, func(t *testing.T) {
			a, errA := ParseConsistencyToken(c.A)
			b, errB := ParseConsistencyToken(c.B)
			want, errW := ParseConsistencyToken(c.Expect)
			if err := errors.Join(errA, errB, errW); err != nil {
				t.Fatal(err)
			}
			if got := a.Merge(b).String(); got != c.Expect {
				t.Errorf("a.Merge(b) = %q, want %q", got, c.Expect)
			}
			if got := b.Merge(a).String(); got != c.Expect {
				t.Errorf("b.Merge(a) = %q, want %q", got, c.Expect)
			}
			if !a.Merge(b).Equal(want) {
				t.Error("the merge is not Equal to the expectation")
			}
			session := NewConsistencySession()
			if err := errors.Join(session.Record(c.A), session.Record(c.B)); err != nil {
				t.Fatal(err)
			}
			if session.Current() != c.Expect {
				t.Errorf("the session holds %q, want %q", session.Current(), c.Expect)
			}
		})
	}
	for _, text := range corpus.Malformed {
		t.Run("malformed/"+text, func(t *testing.T) {
			_, err := ParseConsistencyToken(text)
			var invalid *InvalidArgumentError
			if !errors.As(err, &invalid) {
				t.Fatalf("%q: want an *InvalidArgumentError, got %v", text, err)
			}
		})
	}
}

func jsonNumber(n uint64) string {
	raw, _ := json.Marshal(n)
	return string(raw)
}

// ---------------------------------------------------------------------------

func Test_go_session_read_your_writes(t *testing.T) {
	ctx := context.Background()
	t.Run("a write's token is the next read's consistency.at_least", func(t *testing.T) {
		session := NewConsistencySession()
		invoker := newTestInvoker(session, InvokerDefaults{Namespace: "app"}, 0)
		write := &stubMethod[collectionv1.WriteDocumentsRequest, collectionv1.WriteDocumentsResponse]{
			answer: func(int, context.Context, *collectionv1.WriteDocumentsRequest) (*collectionv1.WriteDocumentsResponse, http.Header, error) {
				return &collectionv1.WriteDocumentsResponse{Token: "v1:s4/p0@12"}, nil, nil
			},
		}
		get := &stubMethod[collectionv1.GetDocumentsRequest, collectionv1.GetDocumentsResponse]{
			answer: func(int, context.Context, *collectionv1.GetDocumentsRequest) (*collectionv1.GetDocumentsResponse, http.Header, error) {
				return &collectionv1.GetDocumentsResponse{ReadToken: "v1:s4/p0@12"}, nil, nil
			},
		}
		if _, err := Unary(invoker, ctx, writeBinding, write.call, &collectionv1.WriteDocumentsRequest{Collection: "notes"}); err != nil {
			t.Fatal(err)
		}
		if session.CurrentIn("loams.collection.v1") != "v1:s4/p0@12" {
			t.Fatalf("the session holds %q", session.CurrentIn("loams.collection.v1"))
		}
		caller := &collectionv1.GetDocumentsRequest{Collection: "notes"}
		if _, err := Unary(invoker, ctx, getBinding, get.call, caller); err != nil {
			t.Fatal(err)
		}
		if got := get.requests[0].GetConsistency().GetAtLeast(); got != "v1:s4/p0@12" {
			t.Errorf("the read went out at_least %q", got)
		}
		if get.headers[0].Get(ConsistencyHeader) != "" {
			t.Error("the token went out as a header as well as in the field")
		}
		if caller.GetConsistency() != nil || caller.GetNamespace() != "" {
			t.Error("the caller's request was mutated")
		}
	})

	t.Run("two writes merge, an explicit consistency wins, and the header counts", func(t *testing.T) {
		session := NewConsistencySession()
		invoker := newTestInvoker(session, InvokerDefaults{Namespace: "app"}, 0)
		write := &stubMethod[collectionv1.WriteDocumentsRequest, collectionv1.WriteDocumentsResponse]{
			answer: func(n int, _ context.Context, _ *collectionv1.WriteDocumentsRequest) (*collectionv1.WriteDocumentsResponse, http.Header, error) {
				if n == 1 {
					return &collectionv1.WriteDocumentsResponse{Token: "v1:s4/p0@12"}, nil, nil
				}
				return &collectionv1.WriteDocumentsResponse{}, http.Header{ConsistencyHeader: {"v1:s4/p1@3"}}, nil
			},
		}
		for range 2 {
			if _, err := Unary(invoker, ctx, writeBinding, write.call, &collectionv1.WriteDocumentsRequest{Collection: "notes"}); err != nil {
				t.Fatal(err)
			}
		}
		if session.CurrentIn("loams.collection.v1") != "v1:s4/p0@12,s4/p1@3" {
			t.Fatalf("the session holds %q", session.CurrentIn("loams.collection.v1"))
		}
		get := &stubMethod[collectionv1.GetDocumentsRequest, collectionv1.GetDocumentsResponse]{
			answer: func(int, context.Context, *collectionv1.GetDocumentsRequest) (*collectionv1.GetDocumentsResponse, http.Header, error) {
				return &collectionv1.GetDocumentsResponse{}, nil, nil
			},
		}
		eventual := &collectionv1.GetDocumentsRequest{
			Collection: "notes", Consistency: &collectionv1.Consistency{Freshness: collectionv1.Freshness_FRESHNESS_EVENTUAL},
		}
		if _, err := Unary(invoker, ctx, getBinding, get.call, eventual); err != nil {
			t.Fatal(err)
		}
		if got := get.requests[0].GetConsistency(); got.GetAtLeast() != "" || got.GetFreshness() != collectionv1.Freshness_FRESHNESS_EVENTUAL {
			t.Errorf("an explicit consistency was overridden: %v", got)
		}
		if _, err := Unary(invoker, ctx, getBinding, get.call, &collectionv1.GetDocumentsRequest{Collection: "notes"}, WithoutSessionConsistency()); err != nil {
			t.Fatal(err)
		}
		if get.requests[1].GetConsistency() != nil {
			t.Error("an opted-out call was held to the session")
		}
	})

	t.Run("a graph read never takes the session's token (SDKP-17)", func(t *testing.T) {
		// A graph's token names one graph, and a Graph read refuses any other,
		// so the session's token, of any scope, never goes out on one.
		session := NewConsistencySession()
		_ = session.RecordIn("v1:s2/p0@5", "loams.graph.v1")
		_ = session.RecordIn("v1:s4/p0@1", "loams.collection.v1")
		invoker := newTestInvoker(session, InvokerDefaults{Namespace: "app"}, 0)
		execute := &stubMethod[graphv1.ExecuteRequest, graphv1.ExecuteResponse]{
			answer: func(int, context.Context, *graphv1.ExecuteRequest) (*graphv1.ExecuteResponse, http.Header, error) {
				return &graphv1.ExecuteResponse{ConsistencyToken: "v1:s2/p0@9"}, nil, nil
			},
		}
		if _, err := Unary(invoker, ctx, graphExecute, execute.call, &graphv1.ExecuteRequest{Graph: "g"}); err != nil {
			t.Fatal(err)
		}
		if got := execute.requests[0].GetConsistency(); got != nil {
			t.Errorf("the graph read went out with %v", got)
		}
		// Its answer's token is recorded in its own scope, unused by reads.
		if session.CurrentIn("loams.graph.v1") != "v1:s2/p0@9" {
			t.Errorf("the graph scope holds %q", session.CurrentIn("loams.graph.v1"))
		}
		if session.CurrentIn("loams.collection.v1") != "v1:s4/p0@1" {
			t.Errorf("the graph write moved the collection scope: %q", session.CurrentIn("loams.collection.v1"))
		}
	})

	t.Run("an unparseable token is counted, a foreign string is ignored", func(t *testing.T) {
		session := NewConsistencySession()
		invoker := newTestInvoker(session, InvokerDefaults{Namespace: "app"}, 0)
		tokens := []string{"v1:garbage", "page-2"}
		write := &stubMethod[collectionv1.WriteDocumentsRequest, collectionv1.WriteDocumentsResponse]{
			answer: func(n int, _ context.Context, _ *collectionv1.WriteDocumentsRequest) (*collectionv1.WriteDocumentsResponse, http.Header, error) {
				return &collectionv1.WriteDocumentsResponse{Token: tokens[n-1]}, nil, nil
			},
		}
		for range tokens {
			if _, err := Unary(invoker, ctx, writeBinding, write.call, &collectionv1.WriteDocumentsRequest{Collection: "notes"}); err != nil {
				t.Fatalf("a committed write failed over its token: %v", err)
			}
		}
		if session.Rejected() != 1 || session.Current() != "" {
			t.Errorf("rejected %d, holds %q", session.Rejected(), session.Current())
		}
	})
}

// ---------------------------------------------------------------------------

func okGet() *stubMethod[collectionv1.GetDocumentsRequest, collectionv1.GetDocumentsResponse] {
	return &stubMethod[collectionv1.GetDocumentsRequest, collectionv1.GetDocumentsResponse]{
		answer: func(int, context.Context, *collectionv1.GetDocumentsRequest) (*collectionv1.GetDocumentsResponse, http.Header, error) {
			return &collectionv1.GetDocumentsResponse{}, nil, nil
		},
	}
}

func Test_go_default_namespace(t *testing.T) {
	ctx := context.Background()
	t.Run("fills an empty namespace from the client default", func(t *testing.T) {
		get := okGet()
		if _, err := Unary(newTestInvoker(nil, InvokerDefaults{Namespace: "app"}, 0), ctx, getBinding, get.call, &collectionv1.GetDocumentsRequest{Collection: "notes"}); err != nil {
			t.Fatal(err)
		}
		if get.requests[0].GetNamespace() != "app" {
			t.Errorf("sent namespace %q", get.requests[0].GetNamespace())
		}
	})
	t.Run("an explicit namespace wins", func(t *testing.T) {
		get := okGet()
		if _, err := Unary(newTestInvoker(nil, InvokerDefaults{Namespace: "app"}, 0), ctx, getBinding, get.call, &collectionv1.GetDocumentsRequest{Namespace: "other", Collection: "notes"}); err != nil {
			t.Fatal(err)
		}
		if get.requests[0].GetNamespace() != "other" {
			t.Errorf("sent namespace %q", get.requests[0].GetNamespace())
		}
	})
	t.Run("neither is invalid_argument before any network call", func(t *testing.T) {
		get := okGet()
		_, err := Unary(newTestInvoker(nil, InvokerDefaults{}, 0), ctx, getBinding, get.call, &collectionv1.GetDocumentsRequest{Collection: "notes"})
		var invalid *InvalidArgumentError
		if !errors.As(err, &invalid) || invalid.Metadata["field"] != "namespace" {
			t.Fatalf("want an *InvalidArgumentError on namespace, got %v", err)
		}
		if len(get.requests) != 0 {
			t.Error("the call went out")
		}
	})
	t.Run("a namespace field outside the app packages is left alone", func(t *testing.T) {
		_, err := Unary(newTestInvoker(nil, InvokerDefaults{Namespace: "app"}, 0), ctx,
			facade.CallBinding{Package: "loams.live.v1", RPC: "loams.live.v1.LiveAdminService/CreateApp", Retry: facade.RetryManual},
			func(_ context.Context, request *connect.Request[CreateAppRequest]) (*connect.Response[CreateAppResponse], error) {
				if request.Msg.GetNamespace() != "" {
					t.Errorf("a live app's namespace was filled with %q", request.Msg.GetNamespace())
				}
				return connect.NewResponse(&CreateAppResponse{}), nil
			}, &CreateAppRequest{Name: "app"})
		if err != nil {
			t.Fatal(err)
		}
	})
	t.Run("is Options.Namespace and LOAMS_NAMESPACE", func(t *testing.T) {
		client := MustNew(Options{Endpoint: "http://127.0.0.1:1", Namespace: "app"})
		if client.Namespace() != "app" || client.invoker.defaults.Namespace != "app" {
			t.Errorf("the client's namespace is %q", client.Namespace())
		}
		env := map[string]string{"LOAMS_ENDPOINT": "http://127.0.0.1:1", "LOAMS_NAMESPACE": "env", "LOAMS_API_KEY": "k"}
		lookup := func(name string) (string, bool) { value, ok := env[name]; return value, ok }
		fromEnvClient, err := fromEnv(Options{}, lookup)
		if err != nil {
			t.Fatal(err)
		}
		if fromEnvClient.Namespace() != "env" || fromEnvClient.invoker.source == nil {
			t.Errorf("FromEnv: namespace %q, auth %v", fromEnvClient.Namespace(), fromEnvClient.invoker.source)
		}
		if _, err := fromEnv(Options{}, func(string) (string, bool) { return "", false }); err == nil ||
			!strings.Contains(err.Error(), "LOAMS_ENDPOINT") {
			t.Errorf("FromEnv with no endpoint: %v", err)
		}
	})
}

// ---------------------------------------------------------------------------

// slowGet answers after d, or with the context's error when it ends first, and
// records the deadline each attempt saw.
func slowGet(d time.Duration) (*stubMethod[collectionv1.GetDocumentsRequest, collectionv1.GetDocumentsResponse], *[]time.Duration) {
	var budgets []time.Duration
	var mu sync.Mutex
	stub := &stubMethod[collectionv1.GetDocumentsRequest, collectionv1.GetDocumentsResponse]{
		answer: func(_ int, ctx context.Context, _ *collectionv1.GetDocumentsRequest) (*collectionv1.GetDocumentsResponse, http.Header, error) {
			if deadline, ok := ctx.Deadline(); ok {
				mu.Lock()
				budgets = append(budgets, time.Until(deadline))
				mu.Unlock()
			}
			select {
			case <-time.After(d):
				return &collectionv1.GetDocumentsResponse{}, nil, nil
			case <-ctx.Done():
				return nil, nil, connect.NewError(connect.CodeDeadlineExceeded, ctx.Err())
			}
		},
	}
	return stub, &budgets
}

func Test_go_timeout_deadline(t *testing.T) {
	ctx := context.Background()
	t.Run("raises client_deadline_exceeded and does not retry", func(t *testing.T) {
		get, budgets := slowGet(10 * time.Second)
		started := time.Now()
		_, err := Unary(newTestInvoker(nil, InvokerDefaults{Namespace: "app", Timeout: 40 * time.Millisecond}, 3), ctx, getBinding, get.call, &collectionv1.GetDocumentsRequest{Collection: "notes"})
		var deadline *DeadlineExceededError
		if !errors.As(err, &deadline) {
			t.Fatalf("want a *DeadlineExceededError, got %v", err)
		}
		if deadline.Reason != ReasonClientDeadlineExceeded || deadline.Code != connect.CodeDeadlineExceeded {
			t.Errorf("reason %q code %v", deadline.Reason, deadline.Code)
		}
		if len(get.requests) != 1 {
			t.Errorf("%d attempts, want 1", len(get.requests))
		}
		if time.Since(started) > 2*time.Second {
			t.Error("the deadline did not stop the call")
		}
		// The budget is the context's deadline, which connect-go sends as
		// connect-timeout-ms.
		if len(*budgets) != 1 || (*budgets)[0] <= 0 || (*budgets)[0] > 40*time.Millisecond {
			t.Errorf("the attempt saw budgets %v", *budgets)
		}
	})
	t.Run("takes a per-call override, and zero means none", func(t *testing.T) {
		get, _ := slowGet(60 * time.Millisecond)
		invoker := newTestInvoker(nil, InvokerDefaults{Namespace: "app", Timeout: 20 * time.Millisecond}, 0)
		request := &collectionv1.GetDocumentsRequest{Collection: "notes"}
		if _, err := Unary(invoker, ctx, getBinding, get.call, request, WithTimeout(0)); err != nil {
			t.Errorf("WithTimeout(0): %v", err)
		}
		if _, err := Unary(invoker, ctx, getBinding, get.call, request, WithTimeout(time.Second)); err != nil {
			t.Errorf("WithTimeout(1s): %v", err)
		}
		if _, err := Unary(invoker, ctx, getBinding, get.call, request); ReasonOf(err) != ReasonClientDeadlineExceeded {
			t.Errorf("the client default did not apply: %v", err)
		}
	})
	t.Run("the caller's own deadline is the caller's", func(t *testing.T) {
		get, _ := slowGet(10 * time.Second)
		callerCtx, cancel := context.WithTimeout(ctx, 20*time.Millisecond)
		defer cancel()
		_, err := Unary(newTestInvoker(nil, InvokerDefaults{Namespace: "app", Timeout: time.Minute}, 0), callerCtx, getBinding, get.call, &collectionv1.GetDocumentsRequest{Collection: "notes"})
		if ReasonOf(err) == ReasonClientDeadlineExceeded {
			t.Error("the caller's deadline was reported as the client's")
		}
		if !errors.Is(err, context.DeadlineExceeded) && CodeOf(err) != connect.CodeDeadlineExceeded {
			t.Errorf("got %v", err)
		}
	})
	t.Run("New defaults unary calls to 30 s", func(t *testing.T) {
		if got := MustNew(Options{Endpoint: "http://127.0.0.1:1"}).invoker.defaults.Timeout; got != DefaultTimeout {
			t.Errorf("default timeout %v", got)
		}
		if got := MustNew(Options{Endpoint: "http://127.0.0.1:1", NoTimeout: true}).invoker.defaults.Timeout; got != 0 {
			t.Errorf("NoTimeout left %v", got)
		}
	})
}

// ---------------------------------------------------------------------------

func notes(n int) []DocumentInput {
	docs := make([]DocumentInput, n)
	for i := range docs {
		docs[i] = DocumentInput{ID: UintID(uint64(i + 1)), Source: JSONObject{"title": "note"}}
	}
	return docs
}

func Test_go_write_batch_chunks(t *testing.T) {
	ctx := context.Background()
	invoker := newTestInvoker(nil, InvokerDefaults{Namespace: "app"}, 1)
	documentsOver := func(stub *stubMethod[collectionv1.WriteDocumentsRequest, collectionv1.WriteDocumentsResponse]) DocumentCalls {
		return DocumentCalls{Write: func(ctx context.Context, r *collectionv1.WriteDocumentsRequest, o ...CallOption) (*collectionv1.WriteDocumentsResponse, error) {
			return Unary(invoker, ctx, writeBinding, stub.call, r, o...)
		}}
	}

	t.Run("chunks by rows, one key per chunk, and merges the tokens", func(t *testing.T) {
		stub := &stubMethod[collectionv1.WriteDocumentsRequest, collectionv1.WriteDocumentsResponse]{
			answer: func(n int, _ context.Context, _ *collectionv1.WriteDocumentsRequest) (*collectionv1.WriteDocumentsResponse, http.Header, error) {
				return &collectionv1.WriteDocumentsResponse{Token: "v1:s1/p" + jsonNumber(uint64(n%2)) + "@" + jsonNumber(uint64(n*10))}, nil, nil
			},
		}
		result, err := documentsOver(stub).WriteBatch(ctx, Target{Collection: "notes"}, slices.Values(notes(5)), BatchOptions{ChunkRows: 2})
		if err != nil {
			t.Fatal(err)
		}
		if result.Chunks != 3 || result.Rows != 5 {
			t.Errorf("%d chunks, %d rows", result.Chunks, result.Rows)
		}
		var sizes []int
		keys := map[string]bool{}
		for _, request := range stub.requests {
			sizes = append(sizes, len(request.GetOps()))
			keys[request.GetIdempotencyKey()] = true
		}
		if !slices.Equal(sizes, []int{2, 2, 1}) || len(keys) != 3 {
			t.Errorf("chunk sizes %v, %d distinct keys", sizes, len(keys))
		}
		if result.Token != "v1:s1/p0@20,s1/p1@30" {
			t.Errorf("merged token %q", result.Token)
		}
	})

	t.Run("chunks by bytes", func(t *testing.T) {
		stub := &stubMethod[collectionv1.WriteDocumentsRequest, collectionv1.WriteDocumentsResponse]{
			answer: func(int, context.Context, *collectionv1.WriteDocumentsRequest) (*collectionv1.WriteDocumentsResponse, http.Header, error) {
				return &collectionv1.WriteDocumentsResponse{}, nil, nil
			},
		}
		big := func(id uint64) DocumentInput {
			return DocumentInput{ID: UintID(id), Source: JSONObject{"body": strings.Repeat("x", 400)}}
		}
		docs := []DocumentInput{big(1), big(2), big(3)}
		if _, err := documentsOver(stub).WriteBatch(ctx, Target{Collection: "notes"}, slices.Values(docs), BatchOptions{ChunkBytes: 1000}); err != nil {
			t.Fatal(err)
		}
		var sizes []int
		for _, request := range stub.requests {
			sizes = append(sizes, len(request.GetOps()))
		}
		if !slices.Equal(sizes, []int{2, 1}) {
			t.Errorf("chunk sizes %v", sizes)
		}
	})

	t.Run("a chunk's retry reuses its key", func(t *testing.T) {
		stub := &stubMethod[collectionv1.WriteDocumentsRequest, collectionv1.WriteDocumentsResponse]{
			answer: func(n int, _ context.Context, _ *collectionv1.WriteDocumentsRequest) (*collectionv1.WriteDocumentsResponse, http.Header, error) {
				if n == 1 {
					return nil, nil, connect.NewError(connect.CodeUnavailable, errors.New("restarting"))
				}
				return &collectionv1.WriteDocumentsResponse{}, nil, nil
			},
		}
		if _, err := documentsOver(stub).WriteBatch(ctx, Target{Collection: "notes"}, slices.Values(notes(1)), BatchOptions{}); err != nil {
			t.Fatal(err)
		}
		if len(stub.requests) != 2 || stub.requests[0].GetIdempotencyKey() != stub.requests[1].GetIdempotencyKey() {
			t.Errorf("%d attempts; keys differ or are missing", len(stub.requests))
		}
	})

	t.Run("stops at the first failed chunk and says how much was applied", func(t *testing.T) {
		stub := &stubMethod[collectionv1.WriteDocumentsRequest, collectionv1.WriteDocumentsResponse]{
			answer: func(n int, _ context.Context, _ *collectionv1.WriteDocumentsRequest) (*collectionv1.WriteDocumentsResponse, http.Header, error) {
				if n == 2 {
					return nil, nil, connect.NewError(connect.CodeInvalidArgument, errors.New("bad doc"))
				}
				return &collectionv1.WriteDocumentsResponse{Token: "v1:s1/p0@5"}, nil, nil
			},
		}
		_, err := documentsOver(stub).WriteBatch(ctx, Target{Collection: "notes"}, slices.Values(notes(6)), BatchOptions{ChunkRows: 2})
		var batch *BatchError
		if !errors.As(err, &batch) {
			t.Fatalf("want a *BatchError, got %v", err)
		}
		if batch.Reason != ReasonBatchPartiallyApplied || batch.Code != connect.CodeInvalidArgument ||
			batch.ChunksApplied != 1 || batch.RowsApplied != 2 || batch.Token != "v1:s1/p0@5" {
			t.Errorf("%+v", batch)
		}
		var invalid *InvalidArgumentError
		if !errors.As(err, &invalid) {
			t.Error("the failed chunk's own class is not reachable through the BatchError")
		}
		if len(stub.requests) != 2 {
			t.Errorf("%d chunks were sent", len(stub.requests))
		}
	})
}

// ---------------------------------------------------------------------------

type record struct {
	offset uint64
	value  string
}

func Test_go_streams_subscribe_resumes(t *testing.T) {
	ctx := context.Background()
	t.Run("yields from the offset, and resumes where it stopped", func(t *testing.T) {
		var requested []uint64
		fetch := func(_ context.Context, from uint64) ([]record, error) {
			requested = append(requested, from)
			var out []record
			for offset := from; offset < 5 && len(out) < 2; offset++ {
				out = append(out, record{offset, "r" + jsonNumber(offset)})
			}
			return out, nil
		}
		offsetOf := func(r record) uint64 { return r.offset }
		sub := Subscribe(ctx, fetch, offsetOf, 1)
		var seen []string
		for sub.Next() {
			seen = append(seen, sub.Record().value)
			if len(seen) == 3 {
				break
			}
		}
		_ = sub.Close()
		if !slices.Equal(seen, []string{"r1", "r2", "r3"}) || sub.Offset() != 4 {
			t.Errorf("seen %v, offset %d", seen, sub.Offset())
		}
		if !slices.Equal(requested, []uint64{1, 3}) {
			t.Errorf("fetched from %v", requested)
		}
		resumed := Subscribe(ctx, fetch, offsetOf, sub.Offset())
		if !resumed.Next() || resumed.Record().value != "r4" {
			t.Errorf("resumed at %v", resumed.Record())
		}
		_ = resumed.Close()
		if resumed.Next() || resumed.Err() != nil {
			t.Error("a closed subscription advanced or reported an error")
		}
	})
	t.Run("backs off on an empty fetch and stops when cancelled", func(t *testing.T) {
		var mu sync.Mutex
		fetches := 0
		fetch := func(context.Context, uint64) ([]record, error) {
			mu.Lock()
			fetches++
			mu.Unlock()
			return nil, nil
		}
		cancelCtx, cancel := context.WithCancel(ctx)
		sub := Subscribe(cancelCtx, fetch, func(r record) uint64 { return r.offset }, 0)
		go func() {
			time.Sleep(250 * time.Millisecond)
			cancel()
		}()
		if sub.Next() {
			t.Fatal("an empty partition yielded a record")
		}
		mu.Lock()
		defer mu.Unlock()
		// 100 ms then 200 ms of backoff fit in 250 ms: two or three fetches.
		if fetches < 2 || fetches > 3 {
			t.Errorf("%d fetches in 250 ms", fetches)
		}
		if sub.Err() != nil {
			t.Errorf("a cancelled subscription reported %v", sub.Err())
		}
	})
	t.Run("a failed fetch ends it with the error and keeps the offset", func(t *testing.T) {
		sub := Subscribe(ctx, func(context.Context, uint64) ([]record, error) {
			return nil, connect.NewError(connect.CodeNotFound, errors.New("no stream"))
		}, func(r record) uint64 { return r.offset }, 7)
		if sub.Next() {
			t.Fatal("advanced")
		}
		var notFound *NotFoundError
		if !errors.As(sub.Err(), &notFound) || sub.Offset() != 7 {
			t.Errorf("err %v, offset %d", sub.Err(), sub.Offset())
		}
	})
}

// ---------------------------------------------------------------------------

func Test_go_collection_handle_delegates(t *testing.T) {
	ctx := context.Background()
	var calls []string
	var writes []*collectionv1.WriteDocumentsRequest
	var searches []*collectionv1.SearchRequest
	check := func(name, namespace, collection string) {
		calls = append(calls, name)
		if namespace != "app" || collection != "notes" {
			t.Errorf("%s went to %q/%q", name, namespace, collection)
		}
	}
	modules := HandleModules{
		Documents: DocumentCalls{
			Write: func(_ context.Context, r *collectionv1.WriteDocumentsRequest, _ ...CallOption) (*collectionv1.WriteDocumentsResponse, error) {
				check("write", r.GetNamespace(), r.GetCollection())
				writes = append(writes, r)
				return &collectionv1.WriteDocumentsResponse{}, nil
			},
			Scroll: func(_ context.Context, r *collectionv1.ScrollDocumentsRequest, _ ...CallOption) (*collectionv1.ScrollDocumentsResponse, error) {
				check("scroll", r.GetNamespace(), r.GetCollection())
				return &collectionv1.ScrollDocumentsResponse{}, nil
			},
		},
		GetDocuments: func(_ context.Context, r *collectionv1.GetDocumentsRequest, _ ...CallOption) (*collectionv1.GetDocumentsResponse, error) {
			check("get", r.GetNamespace(), r.GetCollection())
			return &collectionv1.GetDocumentsResponse{}, nil
		},
		CountDocuments: func(_ context.Context, r *collectionv1.CountDocumentsRequest, _ ...CallOption) (*collectionv1.CountDocumentsResponse, error) {
			check("count", r.GetNamespace(), r.GetCollection())
			return &collectionv1.CountDocumentsResponse{}, nil
		},
		Search: SearchCalls{Query: func(_ context.Context, r *collectionv1.SearchRequest, _ ...CallOption) (*collectionv1.SearchResponse, error) {
			check("search", r.GetNamespace(), r.GetCollection())
			searches = append(searches, r)
			return &collectionv1.SearchResponse{}, nil
		}},
		GetCollection: func(_ context.Context, r *collectionv1.GetCollectionRequest, _ ...CallOption) (*collectionv1.CollectionInfo, error) {
			check("info", r.GetNamespace(), r.GetCollection())
			return &collectionv1.CollectionInfo{}, nil
		},
		DropCollection: func(_ context.Context, r *collectionv1.DropCollectionRequest, _ ...CallOption) (*collectionv1.DropCollectionResponse, error) {
			check("drop", r.GetNamespace(), r.GetCollection())
			return &collectionv1.DropCollectionResponse{}, nil
		},
	}
	h := NewCollectionHandle(modules, "notes", "app")
	steps := []error{}
	_, err := h.Upsert(ctx, notes(1))
	steps = append(steps, err)
	_, err = h.Delete(ctx, []DocID{UintID(1)})
	steps = append(steps, err)
	_, err = h.Patch(ctx, UintID(1), PatchChange{Set: JSONObject{"a": 1}, Unset: []string{"b"}})
	steps = append(steps, err)
	_, err = h.WriteBatch(ctx, slices.Values(notes(2)), BatchOptions{})
	steps = append(steps, err)
	_, err = h.Get(ctx, []DocID{UintID(1), StringID("k"), UUIDID("0190f5c4-6c1e-7b3a-9d2e-4f5a6b7c8d9e")})
	steps = append(steps, err)
	_, err = h.Count(ctx, nil)
	steps = append(steps, err)
	it, err := h.ScrollAll(ctx, ScrollOptions{Limit: 2})
	steps = append(steps, err)
	if it != nil {
		for range it.Seq() {
		}
		steps = append(steps, it.Err())
	}
	_, err = h.Search(ctx, NewSearch("ignored").Text("refund"))
	steps = append(steps, err)
	_, err = h.VectorSearch(ctx, "embedding", []float32{1, 0, 0, 0}, VectorSearchOptions{Limit: 3})
	steps = append(steps, err)
	_, err = h.Info(ctx)
	steps = append(steps, err)
	_, err = h.Drop(ctx)
	steps = append(steps, err)
	if err := errors.Join(steps...); err != nil {
		t.Fatal(err)
	}
	want := []string{"write", "write", "write", "write", "get", "count", "scroll", "search", "search", "info", "drop"}
	if !slices.Equal(calls, want) {
		t.Errorf("calls %v, want %v", calls, want)
	}
	if patch := writes[2].GetOps()[0].GetPatch(); patch.GetSource().AsMap()["a"] != float64(1) || !slices.Equal(patch.GetDeleteKeys(), []string{"b"}) {
		t.Errorf("patch %v", patch)
	}
	if searches[1].GetLimit() != 3 || searches[1].GetRetrievers()[0].GetVector().GetField() != "embedding" {
		t.Errorf("vector search %v", searches[1])
	}
	t.Run("a handle over modules without the call is unimplemented", func(t *testing.T) {
		_, err := NewCollectionHandle(HandleModules{}, "notes").Upsert(ctx, notes(1))
		var unimplemented *UnimplementedError
		if !errors.As(err, &unimplemented) {
			t.Errorf("got %v", err)
		}
	})
	t.Run("a client's handle reaches its app modules", func(t *testing.T) {
		// The client's handle is wired to `documents.write`: with no namespace
		// anywhere the call is refused before the network, which proves the
		// call went through the module (R11) rather than answering unimplemented.
		client := MustNew(Options{Endpoint: "http://127.0.0.1:1"})
		_, err := client.Collection("notes").Upsert(ctx, notes(1))
		if ReasonOf(err) != ReasonNamespaceRequired {
			t.Errorf("got %v", err)
		}
	})
}

// ---------------------------------------------------------------------------

func Test_go_search_query_all_requires_tiebreak(t *testing.T) {
	ctx := context.Background()
	t.Run("refuses a sort without a primary-key tiebreaker, before any call", func(t *testing.T) {
		calls := 0
		search := SearchCalls{Query: func(context.Context, *collectionv1.SearchRequest, ...CallOption) (*collectionv1.SearchResponse, error) {
			calls++
			return &collectionv1.SearchResponse{}, nil
		}}
		for _, s := range []Search{
			NewSearch("notes").Text("refund").Limit(2),
			NewSearch("notes").Text("refund").Sort(Sort.Score()).Limit(2),
		} {
			_, err := search.QueryAll(ctx, s)
			var invalid *InvalidArgumentError
			if !errors.As(err, &invalid) {
				t.Errorf("want an *InvalidArgumentError, got %v", err)
			}
		}
		if calls != 0 {
			t.Errorf("%d calls went out", calls)
		}
	})
	t.Run("pages with search_after until a short page", func(t *testing.T) {
		pages := [][]uint64{{1, 2}, {3, 4}, {5}}
		var requests []*collectionv1.SearchRequest
		search := SearchCalls{Query: func(_ context.Context, r *collectionv1.SearchRequest, _ ...CallOption) (*collectionv1.SearchResponse, error) {
			requests = append(requests, r)
			var hits []*collectionv1.Hit
			if len(requests) <= len(pages) {
				for _, pk := range pages[len(requests)-1] {
					hits = append(hits, &collectionv1.Hit{
						Pk:         UintID(pk).Proto(),
						SortValues: []*structpb.Value{structpb.NewNumberValue(float64(pk))},
					})
				}
			}
			return &collectionv1.SearchResponse{Hits: hits}, nil
		}}
		it, err := search.QueryAll(ctx, NewSearch("notes").Text("refund").Sort(Sort.Score(), Sort.PK()).Limit(2))
		if err != nil {
			t.Fatal(err)
		}
		var seen []uint64
		for hit := range it.Seq() {
			seen = append(seen, hit.GetPk().GetUint())
		}
		if it.Err() != nil {
			t.Fatal(it.Err())
		}
		if !slices.Equal(seen, []uint64{1, 2, 3, 4, 5}) || len(requests) != 3 {
			t.Errorf("seen %v over %d requests", seen, len(requests))
		}
		if len(requests[0].GetSearchAfter()) != 0 || requests[1].GetSearchAfter()[0].GetNumberValue() != 2 {
			t.Errorf("search_after %v then %v", requests[0].GetSearchAfter(), requests[1].GetSearchAfter())
		}
	})
}

// The client-side refusals carry their registered reasons (SDKP P0e):
// namespace_required (R11), invalid_consistency_token (R4) and
// search_after_needs_tiebreak (QueryAll).
func TestGoClientSideReasons(t *testing.T) {
	ctx := context.Background()
	invoker := newTestInvoker(nil, InvokerDefaults{}, 0)
	get := &stubMethod[collectionv1.GetDocumentsRequest, collectionv1.GetDocumentsResponse]{
		answer: func(int, context.Context, *collectionv1.GetDocumentsRequest) (*collectionv1.GetDocumentsResponse, http.Header, error) {
			return &collectionv1.GetDocumentsResponse{}, nil, nil
		},
	}
	if _, err := Unary(invoker, ctx, getBinding, get.call, &collectionv1.GetDocumentsRequest{Collection: "notes"}); ReasonOf(err) != ReasonNamespaceRequired {
		t.Errorf("no namespace: reason %v", ReasonOf(err))
	}
	if _, err := ParseConsistencyToken("v1:s01/p0@1"); ReasonOf(err) != ReasonInvalidConsistencyToken {
		t.Errorf("a malformed token: reason %v", ReasonOf(err))
	}
	calls := SearchCalls{Query: func(context.Context, *collectionv1.SearchRequest, ...CallOption) (*collectionv1.SearchResponse, error) {
		return &collectionv1.SearchResponse{}, nil
	}}
	if _, err := calls.QueryAll(ctx, NewSearch("notes").Text("refund", TextOptions{}).Limit(2)); ReasonOf(err) != ReasonSearchAfterNeedsTiebreak {
		t.Errorf("no tiebreak: reason %v", ReasonOf(err))
	}
}
