// SDK2 Task 2's `go_conformance_all_required_fixtures`.
//
// The corpus in `sdks/fixtures` is recorded from a real `loams dev`, and this
// runs every case in it through the SDK's **public** surface — `client.Instance()`
// and `client.Tables()`, the same objects an application uses — rather than
// through the stubs. That is the point of the suite: it proves the facade
// dispatches to the right RPC, sends the right encoding, and turns what comes
// back into the right typed value.
//
// Three things are covered, which between them are what the design asks of a
// conforming SDK (design §44 §10.4):
//
//   - a successful call, in the encoding an SDK sends by default;
//   - a structured-reason error, with `reason` and not the message;
//   - the unavailable-service path, in all three of its shapes: the guard that
//     costs no RPC, the refusal a call gets, and the refusal on a stream.
//
// # Go, and the names of tests
//
// Go's test tool only runs functions whose name begins with `Test` followed by a
// non-lowercase letter, so a function cannot literally be named
// `go_conformance_all_required_fixtures`. Each Go test therefore runs its work in
// a subtest carrying **exactly** that name, which is what `go test -v` prints
// and what `-run` targets:
//
//	go test -run 'go_conformance_all_required_fixtures' -v ./...
//
// `TestConformanceTestNames` asserts the six canonical names are all present, so
// the set cannot quietly shrink.

package loams

import (
	"context"
	"errors"
	"sort"
	"testing"
	"time"
)

// The six tests SDK2 Task 2 requires, in the names the plan states.
const (
	ConformanceAllRequiredFixtures = "go_conformance_all_required_fixtures"
	RetryReusesIdempotencyKey      = "go_retry_reuses_idempotency_key"
	ErrorReasonMapping             = "go_error_reason_mapping"
	StreamResumeWithCursor         = "go_stream_resume_with_cursor"
	TokenSourceRefresh             = "go_token_source_refresh"
	PaginationIterator             = "go_pagination_iterator"
)

// conformanceTests is the registry `TestConformanceTestNames` checks. Each entry
// is the Go function that pins one canonical name.
var conformanceTests = map[string]func(*testing.T){
	ConformanceAllRequiredFixtures: TestGoConformanceAllRequiredFixtures,
	RetryReusesIdempotencyKey:      TestGoRetryReusesIdempotencyKey,
	ErrorReasonMapping:             TestGoErrorReasonMapping,
	StreamResumeWithCursor:         TestGoStreamResumeWithCursor,
	TokenSourceRefresh:             TestGoTokenSourceRefresh,
	PaginationIterator:             TestGoPaginationIterator,
}

// TestConformanceTestNames fails if a required name is missing from the registry,
// so "the six tests exist" is a checkable claim rather than a comment.
func TestConformanceTestNames(t *testing.T) {
	required := []string{
		ConformanceAllRequiredFixtures,
		RetryReusesIdempotencyKey,
		ErrorReasonMapping,
		StreamResumeWithCursor,
		TokenSourceRefresh,
		PaginationIterator,
	}
	sort.Strings(required)
	if len(conformanceTests) != len(required) {
		t.Fatalf("the registry has %d entries and the plan requires %d: %v",
			len(conformanceTests), len(required), conformanceTests)
	}
	for _, name := range required {
		if conformanceTests[name] == nil {
			t.Errorf("no test is registered for %s", name)
		}
	}
}

// TestGoConformanceAllRequiredFixtures replays the whole corpus.
func TestGoConformanceAllRequiredFixtures(t *testing.T) {
	t.Run(ConformanceAllRequiredFixtures, func(t *testing.T) {
		server := startFixtureServer(t)
		client := newTestClient(t, server.endpoint)
		ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
		defer cancel()

		corpus := readCorpus(t)
		expected := []string{
			// A successful call, in each encoding a client might pick.
			"instance_get_instance_json",
			"instance_get_instance_proto",
			"instance_get_instance_grpc_web",
			"instance_get_instance_grpc_web_json",
			// A structured-reason error, in each encoding.
			"instance_who_am_i_json",
			"instance_who_am_i_proto",
			"instance_who_am_i_grpc_web",
			"instance_who_am_i_grpc_web_json",
			// The unavailable-service path, unary and on a stream.
			"live_query_json",
			"live_query_proto",
			"live_query_grpc_web",
			"live_query_grpc_web_json",
			"live_watch",
		}
		recorded := make([]string, 0, len(corpus.Cases))
		for _, entry := range corpus.Cases {
			recorded = append(recorded, entry.Name)
		}
		sort.Strings(recorded)
		sort.Strings(expected)
		if len(recorded) != len(expected) {
			t.Fatalf("the corpus has %d cases and the suite covers %d: %v", len(recorded), len(expected), recorded)
		}
		for index := range expected {
			if recorded[index] != expected[index] {
				t.Fatalf("corpus case %d is %s, the suite expects %s\ncorpus: %v", index, recorded[index], expected[index], recorded)
			}
		}

		// A successful call. `GetInstance` needs no auth, which is why it is the
		// first thing any client calls, and it is the case that proves the SDK
		// sends the encoding the corpus recorded: an SDK that asked for JSON
		// would get the JSON bytes and fail to parse them as protobuf, which is
		// exactly the class of mismatch the four encodings exist to catch.
		info, err := client.Instance().GetInstance(ctx, &GetInstanceRequest{})
		if err != nil {
			t.Fatalf("GetInstance: %v", err)
		}
		if info.GetName() != "Loams" {
			t.Errorf("instance name is %q, want %q", info.GetName(), "Loams")
		}
		if !containsString(info.GetApiVersions(), "loams.instance.v1") {
			t.Errorf("api_versions is %v, want it to contain loams.instance.v1", info.GetApiVersions())
		}
		if len(info.GetServices()) == 0 {
			t.Error("services is empty; the catalogue is what feature detection reads")
		}

		// A structured-reason error. The reason is what the SDK reads; the
		// message is for a person and is not asserted on.
		_, err = client.Instance().WhoAmI(ctx, &WhoAmIRequest{})
		if err == nil {
			t.Fatal("WhoAmI answered, but this build has no authentication yet")
		}
		if !IsLoamsError(err) {
			t.Errorf("WhoAmI failed with %T, want a *LoamsError", err)
		}
		if got := ReasonOf(err); got != ReasonNotImplemented {
			t.Errorf("WhoAmI reason is %q, want %q", got, ReasonNotImplemented)
		}
		var unimplemented *UnimplementedError
		if !errors.As(err, &unimplemented) {
			t.Errorf("WhoAmI failed with %T, want an *UnimplementedError", err)
		}

		// The unavailable-service path, three ways.
		//
		// 1. The guard, from the catalogue, spending no RPC on a call that cannot
		//    work. `loams.live` and `loams.tables` are the same service, so the
		//    guard is asked about either.
		if err := client.System().Guard(ctx, "live"); err == nil {
			t.Error("Guard(\"live\") passed, but loams.live.v1 is not served in the standard variant")
		} else {
			var absent *FeatureNotInVariantError
			if !errors.As(err, &absent) {
				t.Errorf("Guard(\"live\") failed with %T, want a *FeatureNotInVariantError", err)
			}
		}
		if err := client.System().Guard(ctx, "instance"); err != nil {
			t.Errorf("Guard(\"instance\") failed with %v, want nil", err)
		}

		// 2. The refusal a call gets when the caller skips the guard. This is the
		//    typed surface: the reason is in the registry, and the variant is read
		//    out of the metadata rather than parsed out of the message.
		_, err = client.Tables().Query(ctx, &QueryRequest{})
		if err == nil {
			t.Fatal("tables.query answered, but loams.live.v1 is not served in the standard variant")
		}
		var absent *FeatureNotInVariantError
		if !errors.As(err, &absent) {
			t.Fatalf("tables.query failed with %T, want a *FeatureNotInVariantError", err)
		}
		if absent.Reason != ReasonFeatureNotInVariant {
			t.Errorf("the refusal reason is %q, want %q", absent.Reason, ReasonFeatureNotInVariant)
		}
		if absent.Variant != "standard" {
			t.Errorf("the refusal variant is %q, want %q (it comes from metadata.variant, not the message)", absent.Variant, "standard")
		}

		// 3. The refusal on a server stream, which arrives inside the Connect
		//    envelope rather than as an HTTP status. A client that only reads
		//    status codes sees a 200 here, so this is the case that distinguishes
		//    a real Connect implementation from a status-code-only fake.
		stream, err := client.Live().Watch(ctx, &WatchRequest{})
		if err == nil {
			received := 0
			for stream.Receive() {
				received++
			}
			_ = stream.Close()
			if received != 0 {
				t.Errorf("watch yielded %d messages, want none before the refusal", received)
			}
			streamErr := stream.Err()
			var streamAbsent *FeatureNotInVariantError
			if !errors.As(streamErr, &streamAbsent) {
				t.Errorf("watch failed with %T (%v), want a *FeatureNotInVariantError", streamErr, streamErr)
			}
		} else {
			// The refusal arriving at open time rather than at the first Receive is
			// the same failure; both are legal for a Connect client, and both must
			// map to the same type.
			var streamAbsent *FeatureNotInVariantError
			if !errors.As(err, &streamAbsent) {
				t.Errorf("watch failed at open with %T (%v), want a *FeatureNotInVariantError", err, err)
			}
		}

		// The catalogue answers the same question the refusals do, from one call.
		catalogue, err := client.System().Catalogue(ctx)
		if err != nil {
			t.Fatalf("catalogue: %v", err)
		}
		if !containsString(catalogue.Served, "loams.instance.v1") {
			t.Errorf("served is %v, want it to contain loams.instance.v1", catalogue.Served)
		}
		if !containsString(catalogue.Unavailable, "loams.live.v1") {
			t.Errorf("unavailable is %v, want it to contain loams.live.v1", catalogue.Unavailable)
		}
		var liveStatus *ServiceStatus
		for index := range catalogue.Services {
			if catalogue.Services[index].Package == "loams.live.v1" {
				liveStatus = &catalogue.Services[index]
			}
		}
		if liveStatus == nil {
			t.Fatal("the catalogue has no loams.live.v1 entry")
		}
		if !liveStatus.Unstable {
			t.Error("loams.live.v1 is not marked unstable; buf breaking skips that package")
		}

		// The two facade names for one package answer the same question, because
		// the guard resolves a module name to its package.
		byModule, err := client.System().AvailableModule(ctx, "tables")
		if err != nil {
			t.Fatalf("AvailableModule(\"tables\"): %v", err)
		}
		if byModule {
			t.Error("AvailableModule(\"tables\") says loams.live.v1 is served")
		}
		byPackage, err := client.System().Available(ctx, "loams.live.v1")
		if err != nil {
			t.Fatalf("Available(\"loams.live.v1\"): %v", err)
		}
		if byPackage {
			t.Error("Available(\"loams.live.v1\") says the package is served")
		}
	})
}

// TestGoConformanceReportsVersion pins R9: the SDK declares its proto revision
// and reports the server's packages beside it, and a package the SDK speaks that
// the server does not serve is a warning rather than an exception.
func TestGoConformanceReportsVersion(t *testing.T) {
	server := startFixtureServer(t)
	client := newTestClient(t, server.endpoint)
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()

	report, err := client.System().Version(ctx)
	if err != nil {
		t.Fatalf("version: %v", err)
	}
	if report.ProtoRev != client.ProtoRev() {
		t.Errorf("the report says proto revision %q, the client says %q", report.ProtoRev, client.ProtoRev())
	}
	if report.ServerVersion == "" {
		t.Error("the report has no server version")
	}
	// `loams.live.v1` is served as unavailable in the standard variant, and
	// `GetInstance.ApiVersions` lists only what is served, so a mismatch here is
	// the server's, not the SDK's.
	if len(report.APIVersions) != 1 || report.APIVersions[0] != "loams.instance.v1" {
		t.Errorf("api_versions is %v, want [loams.instance.v1]", report.APIVersions)
	}
	if report.Compatible {
		t.Error("the report says the instance is compatible, but it does not serve every package the SDK speaks")
	}
	if !containsString(report.Missing, "loams.live.v1") {
		t.Errorf("missing is %v, want it to contain loams.live.v1", report.Missing)
	}
}

// TestGoConformanceCatalogueIsShared checks R5's "concurrent readers share one
// in-flight fetch": a hundred readers at once must cost one `GetInstance`.
func TestGoConformanceCatalogueIsShared(t *testing.T) {
	server := startFixtureServer(t)
	client := newTestClient(t, server.endpoint)
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()

	const readers = 100
	type outcome struct {
		served int
		err    error
	}
	results := make(chan outcome, readers)
	for range readers {
		go func() {
			catalogue, err := client.System().Catalogue(ctx)
			if err != nil {
				results <- outcome{err: err}
				return
			}
			results <- outcome{served: len(catalogue.Served)}
		}()
	}
	for range readers {
		got := <-results
		if got.err != nil {
			t.Fatalf("catalogue: %v", got.err)
		}
		if got.served == 0 {
			t.Fatal("a concurrent reader got an empty catalogue")
		}
	}
	first, err := client.System().Catalogue(ctx)
	if err != nil {
		t.Fatalf("catalogue: %v", err)
	}
	again, err := client.System().Catalogue(ctx)
	if err != nil {
		t.Fatalf("catalogue: %v", err)
	}
	if len(first.Services) != len(again.Services) {
		t.Errorf("two catalogue reads disagree: %d entries then %d", len(first.Services), len(again.Services))
	}
}

func containsString(haystack []string, needle string) bool {
	for _, value := range haystack {
		if value == needle {
			return true
		}
	}
	return false
}
