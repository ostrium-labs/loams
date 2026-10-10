// SDK2 Task 2's `go_conformance_all_required_fixtures`.
//
// The corpus in `sdks/fixtures` is recorded from a real `loams dev` and
// `loams-apps-mock`, and this runs every required fixture through the SDK.
// All 28 required fixtures are exercised through the Connect runtime and the
// typed error hierarchy, meeting the 100% bar of design §44 §10.4 (D617).

package loams

import (
	"bytes"
	"context"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"net/http"
	"os"
	"path/filepath"
	"sort"
	"strconv"
	"strings"
	"testing"
	"time"

	connect "connectrpc.com/connect"
	"google.golang.org/protobuf/encoding/protojson"
	"google.golang.org/protobuf/proto"

	approvalsv1 "loams.dev/go/gen/loams/approvals/v1"
	"loams.dev/go/gen/loams/approvals/v1/approvalsv1connect"
	devicesv1 "loams.dev/go/gen/loams/devices/v1"
	"loams.dev/go/gen/loams/devices/v1/devicesv1connect"
	instancev1 "loams.dev/go/gen/loams/instance/v1"
	"loams.dev/go/gen/loams/instance/v1/instancev1connect"
	livev1 "loams.dev/go/gen/loams/live/v1"
	"loams.dev/go/gen/loams/live/v1/livev1connect"
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

// The 28 required fixtures of design §44 §10.4, named literally so
// check-languages.mjs can discover them.
var requiredConformanceFixtures = []string{
	"instance_get_instance_grpc_web",
	"instance_get_instance_grpc_web_json",
	"instance_get_instance_json",
	"instance_get_instance_proto",
	"instance_who_am_i_grpc_web",
	"instance_who_am_i_grpc_web_json",
	"instance_who_am_i_json",
	"instance_who_am_i_proto",
	"live_query_grpc_web",
	"live_query_grpc_web_json",
	"live_query_json",
	"live_query_proto",
	"live_watch",
	"mock_error_approval_already_decided",
	"mock_error_approval_expired",
	"mock_error_approval_stale_revision",
	"mock_error_encodings",
	"mock_error_not_implemented",
	"mock_error_reason_required",
	"mock_error_requester_cannot_approve",
	"mock_error_step_up_required",
	"mock_state_idempotent_decide",
	"mock_state_stream_heartbeat",
	"mock_state_stream_resume",
	"mock_state_stream_resume_remove",
	"mock_state_stream_snapshot_reset",
	"mock_status_get_instance",
	"mock_status_unauthenticated",
}

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

// Canonical entrypoints matching the exact names required by run-test.sh and required.mjs
func Test_go_conformance_all_required_fixtures(t *testing.T) { TestGoConformanceAllRequiredFixtures(t) }
func Test_go_retry_reuses_idempotency_key(t *testing.T)      { TestGoRetryReusesIdempotencyKey(t) }
func Test_go_error_reason_mapping(t *testing.T)             { TestGoErrorReasonMapping(t) }
func Test_go_stream_resume_with_cursor(t *testing.T)         { TestGoStreamResumeWithCursor(t) }
func Test_go_token_source_refresh(t *testing.T)             { TestGoTokenSourceRefresh(t) }
func Test_go_pagination_iterator(t *testing.T)             { TestGoPaginationIterator(t) }

type manifestJSON struct {
	Fixtures []struct {
		Name     string `json:"name"`
		Required bool   `json:"required"`
		File     string `json:"file"`
	} `json:"fixtures"`
}

type recordedStepJSON struct {
	Request struct {
		Method     string            `json:"method"`
		Path       string            `json:"path"`
		Headers    map[string]string `json:"headers"`
		Body       any               `json:"body"`
		BodyBase64 string            `json:"bodyBase64"`
	} `json:"request"`
	Response struct {
		Status    int `json:"status"`
		Truncated bool `json:"truncated"`
	} `json:"response"`
	Expect map[string]any `json:"expect"`
}

type recordedFixtureJSON struct {
	Name  string             `json:"name"`
	Steps []recordedStepJSON `json:"steps"`
}

func decodeRecordedRequest(path string, reqBytes []byte, isJSON bool, isFramed bool) (proto.Message, error) {
	raw := reqBytes
	if isFramed && len(raw) >= 5 {
		raw = raw[5:]
	}
	var msg proto.Message
	switch path {
	case "/loams.instance.v1.InstanceService/GetInstance":
		msg = &instancev1.GetInstanceRequest{}
	case "/loams.instance.v1.InstanceService/WhoAmI":
		msg = &instancev1.WhoAmIRequest{}
	case "/loams.live.v1.LiveService/Query":
		msg = &livev1.QueryRequest{}
	case "/loams.live.v1.LiveService/Watch":
		msg = &livev1.WatchRequest{}
	case "/loams.approvals.v1.ApprovalService/DecideApproval":
		msg = &approvalsv1.DecideApprovalRequest{}
	case "/loams.approvals.v1.ApprovalService/WatchApprovals":
		msg = &approvalsv1.WatchApprovalsRequest{}
	case "/loams.approvals.v1.ApprovalService/ListApprovals":
		msg = &approvalsv1.ListApprovalsRequest{}
	case "/loams.devices.v1.DeviceService/SendTestNotification":
		msg = &devicesv1.SendTestNotificationRequest{}
	default:
		return nil, fmt.Errorf("unknown RPC: %s", path)
	}

	if isJSON {
		if len(raw) > 0 {
			if err := (protojson.UnmarshalOptions{DiscardUnknown: true}).Unmarshal(raw, msg); err != nil {
				return nil, err
			}
		}
	} else {
		if len(raw) > 0 {
			if err := proto.Unmarshal(raw, msg); err != nil {
				return nil, err
			}
		}
	}
	return msg, nil
}

type stepResult struct {
	answer []byte
	err    error
	frames int
}

func replayStep(ctx context.Context, httpClient connect.HTTPClient, baseURL string, path string, reqMsg proto.Message, headers map[string]string, isGRPCWeb bool, isJSON bool) stepResult {
	var opts []connect.ClientOption
	if isGRPCWeb {
		opts = append(opts, connect.WithGRPCWeb())
	}
	if isJSON {
		opts = append(opts, connect.WithProtoJSON())
	}

	switch path {
	case "/loams.instance.v1.InstanceService/GetInstance":
		c := instancev1connect.NewInstanceServiceClient(httpClient, baseURL, opts...)
		req := connect.NewRequest(reqMsg.(*instancev1.GetInstanceRequest))
		for k, v := range headers {
			req.Header().Set(k, v)
		}
		resp, err := c.GetInstance(ctx, req)
		if err != nil {
			return stepResult{err: err}
		}
		b, _ := proto.Marshal(resp.Msg)
		return stepResult{answer: b, frames: 1}

	case "/loams.instance.v1.InstanceService/WhoAmI":
		c := instancev1connect.NewInstanceServiceClient(httpClient, baseURL, opts...)
		req := connect.NewRequest(reqMsg.(*instancev1.WhoAmIRequest))
		for k, v := range headers {
			req.Header().Set(k, v)
		}
		resp, err := c.WhoAmI(ctx, req)
		if err != nil {
			return stepResult{err: err}
		}
		b, _ := proto.Marshal(resp.Msg)
		return stepResult{answer: b, frames: 1}

	case "/loams.live.v1.LiveService/Query":
		c := livev1connect.NewLiveServiceClient(httpClient, baseURL, opts...)
		req := connect.NewRequest(reqMsg.(*livev1.QueryRequest))
		for k, v := range headers {
			req.Header().Set(k, v)
		}
		resp, err := c.Query(ctx, req)
		if err != nil {
			return stepResult{err: err}
		}
		b, _ := proto.Marshal(resp.Msg)
		return stepResult{answer: b, frames: 1}

	case "/loams.live.v1.LiveService/Watch":
		c := livev1connect.NewLiveServiceClient(httpClient, baseURL, opts...)
		req := connect.NewRequest(reqMsg.(*livev1.WatchRequest))
		for k, v := range headers {
			req.Header().Set(k, v)
		}
		stream, err := c.Watch(ctx, req)
		if err != nil {
			return stepResult{err: err}
		}
		defer stream.Close()
		var count int
		var ans []byte
		for stream.Receive() {
			count++
			b, _ := proto.Marshal(stream.Msg())
			ans = append(ans, b...)
		}
		return stepResult{answer: ans, err: stream.Err(), frames: count}

	case "/loams.approvals.v1.ApprovalService/DecideApproval":
		c := approvalsv1connect.NewApprovalServiceClient(httpClient, baseURL, opts...)
		req := connect.NewRequest(reqMsg.(*approvalsv1.DecideApprovalRequest))
		for k, v := range headers {
			req.Header().Set(k, v)
		}
		resp, err := c.DecideApproval(ctx, req)
		if err != nil {
			return stepResult{err: err}
		}
		b, _ := proto.Marshal(resp.Msg)
		return stepResult{answer: b, frames: 1}

	case "/loams.approvals.v1.ApprovalService/WatchApprovals":
		c := approvalsv1connect.NewApprovalServiceClient(httpClient, baseURL, opts...)
		req := connect.NewRequest(reqMsg.(*approvalsv1.WatchApprovalsRequest))
		for k, v := range headers {
			req.Header().Set(k, v)
		}
		stream, err := c.WatchApprovals(ctx, req)
		if err != nil {
			return stepResult{err: err}
		}
		defer stream.Close()
		var count int
		var ans []byte
		for stream.Receive() {
			count++
			b, _ := proto.Marshal(stream.Msg())
			ans = append(ans, b...)
		}
		return stepResult{answer: ans, err: stream.Err(), frames: count}

	case "/loams.approvals.v1.ApprovalService/ListApprovals":
		c := approvalsv1connect.NewApprovalServiceClient(httpClient, baseURL, opts...)
		req := connect.NewRequest(reqMsg.(*approvalsv1.ListApprovalsRequest))
		for k, v := range headers {
			req.Header().Set(k, v)
		}
		resp, err := c.ListApprovals(ctx, req)
		if err != nil {
			return stepResult{err: err}
		}
		b, _ := proto.Marshal(resp.Msg)
		return stepResult{answer: b, frames: 1}

	case "/loams.devices.v1.DeviceService/SendTestNotification":
		c := devicesv1connect.NewDeviceServiceClient(httpClient, baseURL, opts...)
		req := connect.NewRequest(reqMsg.(*devicesv1.SendTestNotificationRequest))
		for k, v := range headers {
			req.Header().Set(k, v)
		}
		resp, err := c.SendTestNotification(ctx, req)
		if err != nil {
			return stepResult{err: err}
		}
		b, _ := proto.Marshal(resp.Msg)
		return stepResult{answer: b, frames: 1}
	}
	return stepResult{err: fmt.Errorf("unknown path %s", path)}
}

func writeConformanceReport(endpoint string, ran []string) error {
	resultsDir := filepath.Join(fixturesDir, "results")
	if err := os.MkdirAll(resultsDir, 0o755); err != nil {
		return err
	}
	report := map[string]any{
		"about":     "What this SDK's suite ran.",
		"language":  "go",
		"transport": "connect",
		"live":      false,
		"endpoint":  endpoint,
		"tests": []string{
			ConformanceAllRequiredFixtures,
			RetryReusesIdempotencyKey,
			ErrorReasonMapping,
			StreamResumeWithCursor,
			TokenSourceRefresh,
			PaginationIterator,
		},
		"ran":     ran,
		"skipped": []any{},
	}
	data, err := json.MarshalIndent(report, "", "  ")
	if err != nil {
		return err
	}
	return os.WriteFile(filepath.Join(resultsDir, "go.json"), append(data, '\n'), 0o644)
}

// TestGoConformanceAllRequiredFixtures replays the whole corpus.
func TestGoConformanceAllRequiredFixtures(t *testing.T) {
	t.Run(ConformanceAllRequiredFixtures, func(t *testing.T) {
		server := startFixtureServer(t)
		client := newTestClient(t, server.endpoint)
		ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
		defer cancel()

		manifestData, err := os.ReadFile(filepath.Join(fixturesDir, "manifest.json"))
		if err != nil {
			t.Fatalf("reading manifest.json: %v", err)
		}
		var manifest manifestJSON
		if err := json.Unmarshal(manifestData, &manifest); err != nil {
			t.Fatalf("unmarshaling manifest.json: %v", err)
		}

		httpClient := &http.Client{Timeout: 10 * time.Second}
		var ran []string
		var failures []string

		for _, fix := range manifest.Fixtures {
			if !fix.Required {
				continue
			}

			fixData, err := os.ReadFile(filepath.Join(fixturesDir, fix.File))
			if err != nil {
				failures = append(failures, fmt.Sprintf("%s: reading fixture file: %v", fix.Name, err))
				continue
			}

			var rec recordedFixtureJSON
			if err := json.Unmarshal(fixData, &rec); err != nil {
				failures = append(failures, fmt.Sprintf("%s: parsing fixture JSON: %v", fix.Name, err))
				continue
			}
			if len(rec.Steps) == 0 {
				var single recordedStepJSON
				if err := json.Unmarshal(fixData, &single); err == nil {
					rec.Steps = []recordedStepJSON{single}
				}
			}

			fixtureFailed := false
			var answers [][]byte

			for idx, step := range rec.Steps {
				rpcPath := step.Request.Path
				ct := strings.ToLower(step.Request.Headers["content-type"])
				isGRPCWeb := strings.Contains(ct, "grpc-web")
				isJSON := strings.HasSuffix(ct, "json")
				isStream := strings.Contains(rpcPath, "Watch")
				isFramed := isGRPCWeb || (strings.Contains(ct, "connect") && isStream)

				var rawBody []byte
				if step.Request.BodyBase64 != "" {
					rawBody, _ = base64.StdEncoding.DecodeString(step.Request.BodyBase64)
				} else if s, ok := step.Request.Body.(string); ok {
					rawBody = []byte(s)
				} else if step.Request.Body != nil {
					rawBody, _ = json.Marshal(step.Request.Body)
				}

				reqMsg, err := decodeRecordedRequest(rpcPath, rawBody, isJSON, isFramed)
				if err != nil {
					failures = append(failures, fmt.Sprintf("%s step %d: decode request: %v", fix.Name, idx, err))
					fixtureFailed = true
					break
				}

				headers := map[string]string{
					"loams-fixture-name": fix.Name,
					"loams-fixture-step": strconv.Itoa(idx),
				}
				if auth, ok := step.Request.Headers["authorization"]; ok {
					headers["authorization"] = auth
				}

				res := replayStep(ctx, httpClient, server.endpoint, rpcPath, reqMsg, headers, isGRPCWeb, isJSON)
				answers = append(answers, res.answer)

				expect := step.Expect
				wantReason, hasReason := expect["reason"]
				if hasReason && wantReason != nil {
					wantStr, _ := wantReason.(string)
					loamsErr := ToLoamsError(res.err, rpcPath)
					gotReason := ReasonOf(loamsErr)
					if string(gotReason) != wantStr {
						failures = append(failures, fmt.Sprintf("%s step %d: expected reason %s, got %s (err: %v)", fix.Name, idx, wantStr, gotReason, res.err))
						fixtureFailed = true
					}
				} else if hasReason && wantReason == nil {
					if res.err == nil {
						failures = append(failures, fmt.Sprintf("%s step %d: expected unauthenticated refusal, but got success", fix.Name, idx))
						fixtureFailed = true
					}
				} else {
					if res.err != nil {
						if isStream && step.Response.Truncated {
							// Stream truncation is expected
						} else {
							failures = append(failures, fmt.Sprintf("%s step %d: unexpected error: %v", fix.Name, idx, res.err))
							fixtureFailed = true
						}
					}
				}

				if framesVal, ok := expect["frames"]; ok {
					wantFrames := int(framesVal.(float64))
					if res.frames != wantFrames {
						failures = append(failures, fmt.Sprintf("%s step %d: expected %d frames, got %d", fix.Name, idx, wantFrames, res.frames))
						fixtureFailed = true
					}
				}

				if idStepVal, ok := expect["identicalToStep"]; ok {
					targetStep := int(idStepVal.(float64))
					if targetStep < len(answers)-1 {
						earlier := answers[targetStep]
						if !bytes.Equal(earlier, answers[len(answers)-1]) {
							failures = append(failures, fmt.Sprintf("%s step %d: identicalToStep %d mismatch", fix.Name, idx, targetStep))
							fixtureFailed = true
						}
					}
				}

				if fixtureFailed {
					break
				}
			}

			if !fixtureFailed {
				ran = append(ran, fix.Name)
			}
		}

		if len(failures) > 0 {
			t.Fatalf("Conformance failures (%d):\n  - %s", len(failures), strings.Join(failures, "\n  - "))
		}

		if err := writeConformanceReport(server.endpoint, ran); err != nil {
			t.Fatalf("writing conformance report: %v", err)
		}

		if len(ran) != len(requiredConformanceFixtures) {
			t.Fatalf("replayed %d fixtures, want %d", len(ran), len(requiredConformanceFixtures))
		}

		// Public surface assertions (R5, R8)
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

		if err := client.System().Guard(ctx, "live"); err == nil {
			t.Error("Guard(\"live\") passed, but loams.live.v1 is not served in the standard variant")
		}
		if err := client.System().Guard(ctx, "instance"); err != nil {
			t.Errorf("Guard(\"instance\") failed with %v, want nil", err)
		}

		_, err = client.Tables().Query(ctx, &QueryRequest{})
		if err == nil {
			t.Fatal("tables.query answered, but loams.live.v1 is not served in the standard variant")
		}

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
