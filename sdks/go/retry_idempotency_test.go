// SDK2 Task 2's `go_retry_reuses_idempotency_key`.
//
// Design §44 §7.4, D610 and runtime contract R2 and R3: a mutation may only be
// retried when it carries an idempotency key, and **the same key has to go out
// on every attempt**. A key regenerated per attempt turns one write into two,
// which is the exact failure the key exists to prevent — and it is invisible in a
// test that only counts attempts.
//
// So this pins three things: that a keyed mutation becomes retryable, that the
// key is a UUIDv7 generated once per logical call, and that a retried attempt
// carries the identical request. The last one is asserted end to end against a
// real HTTP server that fails twice and then answers, because a unit test of the
// request object cannot show that the transport re-sent it — and Go is where that
// matters most, because the message is a value the caller may still be holding.

package loams

import (
	"context"
	"errors"
	"net/http"
	"net/http/httptest"
	"sync"
	"testing"
	"time"

	connect "connectrpc.com/connect"
	"google.golang.org/protobuf/proto"

	"loams.dev/go/gen/facade"
	livev1 "loams.dev/go/gen/loams/live/v1"
)

// TestGoRetryReusesIdempotencyKey is the required test.
func TestGoRetryReusesIdempotencyKey(t *testing.T) {
	t.Run(RetryReusesIdempotencyKey, func(t *testing.T) {
		testRetryReusesIdempotencyKey(t)
	})
}

func testRetryReusesIdempotencyKey(t *testing.T) {
	t.Run("mints one UUIDv7 per logical call, before the first attempt", func(t *testing.T) {
		before := time.Now().Add(-time.Second)
		first := ApplyIdempotencyKey(&MutateRequest{}, "", true)
		second := ApplyIdempotencyKey(&MutateRequest{}, "", true)

		if !first.Keyed || !second.Keyed {
			t.Fatalf("a keyed mutation was not keyed: %v %v", first.Keyed, second.Keyed)
		}
		firstKey := first.Request.(*MutateRequest).GetIdempotencyKey()
		secondKey := second.Request.(*MutateRequest).GetIdempotencyKey()
		if firstKey == "" {
			t.Fatal("the minted key is empty")
		}
		if firstKey == secondKey {
			t.Fatal("two logical calls were given the same key, so one would be deduplicated against the other")
		}
		// UUIDv7: version 7 nibble, RFC 4122 variant bit, and a millisecond stamp
		// between the two calls.
		if firstKey[14] != '7' {
			t.Errorf("the key %q does not have the version 7 nibble", firstKey)
		}
		switch firstKey[19] {
		case '8', '9', 'a', 'b':
		default:
			t.Errorf("the key %q does not have the RFC 4122 variant bit", firstKey)
		}
		stamped, ok := UUIDv7Time(firstKey)
		if !ok {
			t.Fatalf("the key %q does not parse as a UUIDv7", firstKey)
		}
		if stamped.Before(before) || stamped.After(time.Now().Add(time.Second)) {
			t.Errorf("the key %q is stamped %v, outside the window the test ran in", firstKey, stamped)
		}
	})

	t.Run("keys a call whose schema declares the field, even when the caller omitted it", func(t *testing.T) {
		// `MutateRequest.idempotency_key` is proto3 `optional`, so a caller who
		// leaves it out sends no key at all and the mutation is not retryable.
		// The decision is read from the generated schema, so it does not depend
		// on what the caller's object happens to contain.
		generated := ApplyIdempotencyKey(&MutateRequest{Function: "go"}, "", true)
		if !generated.Keyed {
			t.Fatal("a request whose schema declares idempotency_key was not keyed")
		}
		if generated.Request.(*MutateRequest).GetIdempotencyKey() == "" {
			t.Error("the keyed request has an empty key")
		}

		// `DeployRequest` has no such field, so a key would be a field the schema
		// does not know: still nothing to key.
		undeclared := ApplyIdempotencyKey(&DeployRequest{Bundle: []byte("js")}, "", false)
		if undeclared.Keyed {
			t.Error("a request with no idempotency_key field was keyed anyway")
		}
		if got := undeclared.Request.(*DeployRequest).GetBundle(); string(got) != "js" {
			t.Errorf("the unkeyed request was modified: bundle is %q", got)
		}
	})

	t.Run("keeps a key the caller supplied, and never mutates the caller's message", func(t *testing.T) {
		caller := &MutateRequest{Function: "go", IdempotencyKey: proto.String("my-key")}
		mine := ApplyIdempotencyKey(caller, "", true)
		if got := mine.Request.(*MutateRequest).GetIdempotencyKey(); got != "my-key" {
			t.Errorf("the caller's key became %q", got)
		}

		supplied := ApplyIdempotencyKey(&MutateRequest{Function: "go"}, "supplied", true)
		if got := supplied.Request.(*MutateRequest).GetIdempotencyKey(); got != "supplied" {
			t.Errorf("the supplied key became %q", got)
		}

		// The minted key goes on a **clone**. Mutating the caller's message would
		// be a data race the moment the same `*MutateRequest` is reused, and Go
		// makes that easy to do by accident.
		fresh := &MutateRequest{Function: "go"}
		keyed := ApplyIdempotencyKey(fresh, "", true)
		if fresh.GetIdempotencyKey() != "" {
			t.Error("the caller's message was mutated; the key must go on a clone")
		}
		if keyed.Request.(*MutateRequest) == fresh {
			t.Error("the keyed request is the caller's own message")
		}
	})

	t.Run("the generated schemas say which calls are keyed", func(t *testing.T) {
		// The live service is the only one with a keyed mutation today, and its
		// messages are generated, so this asserts the schema walk itself:
		// `Mutate` is keyed, `Deploy` and `Query` are not. It is the check that
		// catches a proto changing its shape without the facade table changing.
		if !DeclaresIdempotencyKey(&livev1.MutateRequest{}) {
			t.Error("MutateRequest no longer declares idempotency_key")
		}
		for _, message := range []proto.Message{
			&livev1.DeployRequest{}, &livev1.QueryRequest{}, &livev1.WatchRequest{},
		} {
			if DeclaresIdempotencyKey(message) {
				t.Errorf("%T declares idempotency_key, which it should not", message)
			}
		}
		// And the binding table agrees with the schemas.
		mutate, ok := facade.Binding("tables", "Mutate")
		if !ok {
			t.Fatal("the binding table has no tables.Mutate")
		}
		if !mutate.TakesIdempotencyKey {
			t.Error("tables.Mutate is not marked as taking an idempotency key, though MutateRequest has one")
		}
		deploy, _ := facade.Binding("tables", "Deploy")
		if deploy.TakesIdempotencyKey {
			t.Error("tables.Deploy is marked as taking an idempotency key, though DeployRequest has none")
		}
	})

	t.Run("a keyed mutation retries; an unkeyed one does not", func(t *testing.T) {
		unkeyed := &callSettings{}
		invoker := NewCallInvoker(nil, nil, DefaultMaxRetries, nil)
		mutation, _ := facade.Binding("tables", "Mutate")
		deploy, _ := facade.Binding("tables", "Deploy")
		read, _ := facade.Binding("instance", "GetInstance")

		if plan := invoker.plan(mutation, unkeyed, false); plan.RetrySafe {
			t.Error("a mutation with no key is marked retryable")
		}
		if plan := invoker.plan(mutation, unkeyed, true); !plan.RetrySafe {
			t.Error("a keyed mutation is not marked retryable; the key is what makes the repeat safe")
		}
		if plan := invoker.plan(deploy, unkeyed, false); plan.RetrySafe {
			t.Error("a mutation whose request has no key field is marked retryable")
		}
		if plan := invoker.plan(read, unkeyed, false); !plan.RetrySafe {
			t.Error("a NO_SIDE_EFFECTS read is not marked retryable")
		}
		if !IsRetryableCode(CodeUnavailable) {
			t.Error("unavailable is not one of the retryable codes")
		}
		if IsRetryableCode(CodeNotFound) {
			t.Error("not_found is retryable, which it is not")
		}
	})

	t.Run("every attempt sends the identical key, end to end over HTTP", func(t *testing.T) {
		// The assertion a unit test cannot make: that the *wire* carries the same
		// key three times. The server fails twice with `unavailable` and answers
		// on the third attempt, so a client that regenerated the key per attempt
		// would arrive with three different ones and the test fails.
		var mu sync.Mutex
		var keys []string
		attempts := 0
		server := httptest.NewServer(http.HandlerFunc(func(writer http.ResponseWriter, request *http.Request) {
			body, err := readAll(request)
			if err != nil {
				writer.WriteHeader(http.StatusBadRequest)
				return
			}
			// The unary Connect request body is the marshalled message with no
			// envelope, so it decodes directly.
			var mutate livev1.MutateRequest
			if err := proto.Unmarshal(body, &mutate); err != nil {
				writer.WriteHeader(http.StatusBadRequest)
				return
			}
			mu.Lock()
			attempts++
			current := attempts
			keys = append(keys, mutate.GetIdempotencyKey())
			mu.Unlock()

			if current < 3 {
				writer.Header().Set("Content-Type", "application/json")
				writer.WriteHeader(http.StatusServiceUnavailable)
				_, _ = writer.Write([]byte(`{"code":"unavailable","message":"the node is restarting"}`))
				return
			}
			payload, err := proto.Marshal(&livev1.MutateResponse{CommitTs: 42})
			if err != nil {
				writer.WriteHeader(http.StatusInternalServerError)
				return
			}
			writer.Header().Set("Content-Type", "application/proto")
			writer.WriteHeader(http.StatusOK)
			_, _ = writer.Write(payload)
		}))
		defer server.Close()

		client := newTestClient(t, server.URL)
		ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
		defer cancel()

		// `WithMaxRetries` and a small backoff keep the test quick; the policy
		// itself is asserted above, not here.
		response, err := client.Tables().Mutate(ctx, &MutateRequest{Function: "go"})
		if err != nil {
			t.Fatalf("mutate: %v", err)
		}
		if response.GetCommitTs() != 42 {
			t.Errorf("commit_ts is %d, want 42", response.GetCommitTs())
		}
		mu.Lock()
		defer mu.Unlock()
		if len(keys) != 3 {
			t.Fatalf("the server saw %d attempts, want 3: %v", len(keys), keys)
		}
		for index := 1; index < len(keys); index++ {
			if keys[index] != keys[0] {
				t.Fatalf("attempt %d sent key %q, attempt 1 sent %q: a regenerated key is two writes",
					index+1, keys[index], keys[0])
			}
		}
		if keys[0] == "" {
			t.Error("no attempt carried a key")
		}
	})

	t.Run("a cancelled context is not retried, whatever the class", func(t *testing.T) {
		// The context is the caller's decision and it outranks the retry policy.
		// Charging a retry to a cancelled call is how a shutdown stalls.
		ctx, cancel := context.WithCancel(context.Background())
		cancel()
		attempts := 0
		_, err := CallWithRetry(ctx, &GetInstanceRequest{},
			func(callCtx context.Context, _ Attempt) (any, error) {
				attempts++
				// A real transport fails the attempt with the context's own error;
				// the stub does the same so the assertion is about the SDK's
				// behaviour rather than about a hand-rolled failure.
				if callCtx.Err() != nil {
					return nil, callCtx.Err()
				}
				return nil, newUnavailableError("the node is restarting")
			},
			RetryPlan{RetrySafe: true, MaxRetries: DefaultMaxRetries},
			instanceGetInstanceRPC)
		if err == nil {
			t.Fatal("a cancelled call answered")
		}
		if attempts != 1 {
			t.Errorf("a cancelled call made %d attempts, want 1", attempts)
		}
		if !errors.Is(err, context.Canceled) {
			t.Errorf("the failure is %v, want one that unwraps to context.Canceled", err)
		}
	})

	t.Run("a deadline covers the whole call, not each attempt", func(t *testing.T) {
		// The Go-specific property: `context.WithTimeout` bounds three attempts
		// and the backoff between them together. In a language where the deadline
		// is an option per call, this is the mistake that is easy to make.
		ctx, cancel := context.WithTimeout(context.Background(), 150*time.Millisecond)
		defer cancel()
		attempts := 0
		started := time.Now()
		// A retry budget large enough that only the deadline can stop the loop, so
		// the assertion is about the deadline and not about arithmetic on jittered
		// backoff — which would be a flaky test rather than a true one.
		_, err := CallWithRetry(ctx, &GetInstanceRequest{},
			func(callCtx context.Context, _ Attempt) (any, error) {
				attempts++
				if callCtx.Err() != nil {
					return nil, callCtx.Err()
				}
				return nil, newUnavailableError("the node is restarting")
			},
			RetryPlan{RetrySafe: true, MaxRetries: 10_000},
			instanceGetInstanceRPC)
		if err == nil {
			t.Fatal("a call past its deadline answered")
		}
		if !errors.Is(err, context.DeadlineExceeded) {
			t.Errorf("the failure is %#v, want one that unwraps to context.DeadlineExceeded", err)
		}
		elapsed := time.Since(started)
		if elapsed > 5*time.Second {
			t.Errorf("a 150 ms deadline took %v to stop the loop; the backoff is not watching the context", elapsed)
		}
		if attempts < 2 {
			t.Errorf("the call made only %d attempts; the first backoff should have fitted inside 150 ms", attempts)
		}
	})
}

// TestGoRetryBackoff pins M1.6 Ruling 5's numbers, which are the same in every
// SDK: base 100 ms, doubling, capped at 2 s, full jitter.
func TestGoRetryBackoff(t *testing.T) {
	// Full jitter means the wait is uniform over [0, ceiling] — so the ceiling
	// grows by doubling, stops at the cap, and the *smallest* sample out of two
	// hundred comes nowhere near it. Asserting on the smallest rather than on
	// "a zero appeared" is what makes this deterministic: the chance of no zero in
	// 200 draws from [0,200) is about a third.
	const samples = 200
	for attempt := range 12 {
		ceiling := BackoffCap(attempt)
		smallest := ceiling
		for range samples {
			got := Backoff(attempt, 0)
			if got < 0 || got > ceiling {
				t.Fatalf("attempt %d produced %v, outside [0, %v]", attempt, got, ceiling)
			}
			if got < smallest {
				smallest = got
			}
		}
		if smallest > ceiling/4 {
			t.Errorf("attempt %d never waited less than %v of its %v ceiling, so it is not full jitter",
				attempt, smallest, ceiling)
		}
	}
	// And the ceiling really is base × 2^attempt, capped at 2 s.
	want := []time.Duration{100, 200, 400, 800, 1600, 2000, 2000, 2000}
	for attempt, ceiling := range want {
		if got := BackoffCap(attempt); got != ceiling*time.Millisecond {
			t.Errorf("BackoffCap(%d) is %v, want %v", attempt, got, ceiling*time.Millisecond)
		}
	}
	// A server-sent `RetryInfo.retry_delay` replaces the computed backoff, up to
	// the 30 s ceiling. No proto carries `RetryInfo` yet (R2), so nothing reaches
	// this today; the numbers are pinned so the day one does, they are right.
	if got := Backoff(0, 5*time.Second); got != 5*time.Second {
		t.Errorf("a 5 s server delay became %v, want 5 s", got)
	}
	if got := Backoff(0, 10*time.Minute); got != MaxServerDelayMS*time.Millisecond {
		t.Errorf("a 10 minute server delay became %v, want the %v ceiling", got, MaxServerDelayMS*time.Millisecond)
	}
}

// BackoffCap is the ceiling of one backoff at an attempt: `min(cap, base × 2^attempt)`.
func BackoffCap(attempt int) time.Duration {
	ceiling := MaxDelayMS
	if attempt < 16 {
		if grown := BaseDelayMS << attempt; grown < ceiling {
			ceiling = grown
		}
	}
	return time.Duration(ceiling) * time.Millisecond
}

// newUnavailableError is a `connect.Error` with the retryable code, built the
// way the transport builds one so `ToLoamsError` sees the same shape it sees in
// production.
func newUnavailableError(message string) error {
	return connect.NewError(connect.CodeUnavailable, errors.New(message))
}
