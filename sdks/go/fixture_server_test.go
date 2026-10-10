// The conformance fixture server (design §44 §10.4, SDK1 Task 4).
//
// The corpus in `sdks/fixtures` is recorded from a real `loams dev`, and every
// SDK's suite replays it so thirteen clients can be compared against the same
// bytes. This file is how a Go test gets one.
//
// Three ways to get an endpoint, in the order they are tried:
//
//  1. `LOAMS_TEST_ENDPOINT` — a live `loams dev`. This short-circuits everything
//     else, which is how `sdks/conformance/run.sh` runs the same suite in CI
//     against a recording and on a developer machine against the real thing.
//  2. `node sdks/conformance/fixture-server.mjs` — the shared server. Preferred
//     when Node is on PATH, because then the Go suite really does run against
//     the same server the other twelve do.
//  3. An in-process replay of `sdks/fixtures/recorded` with the same matching
//     rules, for a machine with no Node. A Go module's `go test` should not need
//     a JavaScript runtime to run, and the corpus is the part that matters: the
//     recorded bytes are identical either way.
//
// What all three agree on is the corpus. `TestGoConformanceAllRequiredFixtures`
// reads `sdks/fixtures/index.json` and fails if a case is missing, so a suite
// cannot quietly stop covering something.

package loams

import (
	"bufio"
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

// paths to the shared corpus and server, relative to this package's directory.
// `go test` runs in the package directory, so these are stable; resolving them
// through `runtime.Caller` would survive a test binary run from elsewhere, which
// is not a case this suite has.
const (
	fixturesDir     = "../fixtures"
	fixtureServerJS = "../conformance/fixture-server.mjs"
)

// fixtureServer is a running endpoint and how to stop it.
type fixtureServer struct {
	// endpoint is the base URL the client is built with.
	endpoint string
	// live is true when this is a real `loams dev` rather than a replay, which
	// only changes what a skip is allowed to claim.
	live bool
	stop func()
}

// startFixtureServer brings up whichever of the three endpoints is available.
func startFixtureServer(t *testing.T) *fixtureServer {
	t.Helper()
	if live := strings.TrimRight(os.Getenv("LOAMS_TEST_ENDPOINT"), "/"); live != "" {
		return &fixtureServer{endpoint: live, live: true}
	}
	if _, err := exec.LookPath("node"); err == nil {
		if server, err := startNodeFixtureServer(t); err == nil {
			return server
		}
	}
	return startReplayServer(t)
}

// startNodeFixtureServer runs the shared fixture server and reads the URL it
// prints on stdout.
func startNodeFixtureServer(t *testing.T) (*fixtureServer, error) {
	t.Helper()
	// `exec.Command`, not `exec.CommandContext`: a CommandContext kills the process
	// when the context is done, and a context bounded to "how long until the URL
	// arrives" would take the fixture server with it the moment it had.
	command := exec.Command("node", fixtureServerJS, "--fixtures", fixturesDir, "--port", "0")
	stdout, err := command.StdoutPipe()
	if err != nil {
		return nil, err
	}
	command.Stderr = os.Stderr
	if err := command.Start(); err != nil {
		return nil, err
	}
	type result struct {
		URL string `json:"url"`
	}
	reported := make(chan result, 1)
	failed := make(chan error, 1)
	go func() {
		scanner := bufio.NewScanner(stdout)
		for scanner.Scan() {
			line := strings.TrimSpace(scanner.Text())
			if line == "" {
				continue
			}
			var parsed result
			if err := json.Unmarshal([]byte(line), &parsed); err != nil || parsed.URL == "" {
				continue
			}
			reported <- parsed
			return
		}
		failed <- fmt.Errorf("the fixture server printed no url")
	}()

	deadline := time.NewTimer(30 * time.Second)
	defer deadline.Stop()

	select {
	case parsed := <-reported:
		stop := func() {
			if command.Process != nil {
				_ = command.Process.Signal(os.Interrupt)
			}
			_ = command.Wait()
		}
		t.Cleanup(stop)
		return &fixtureServer{endpoint: parsed.URL, stop: stop}, nil
	case err := <-failed:
		_ = command.Process.Kill()
		return nil, err
	case <-deadline.C:
		_ = command.Process.Kill()
		return nil, errors.New("the fixture server printed no url within 30s")
	}
}

// recordedCase is one entry of `sdks/fixtures/recorded/*.json`.
type recordedCase struct {
	Name    string `json:"name"`
	Request struct {
		Method  string            `json:"method"`
		Path    string            `json:"path"`
		Headers map[string]string `json:"headers"`
		Body    json.RawMessage   `json:"body"`
		// BodyBase64 is the binary encoding of the same request; exactly one of
		// `Body` and `BodyBase64` is present.
		BodyBase64 string `json:"bodyBase64"`
	} `json:"request"`
	Response struct {
		Status     int               `json:"status"`
		Headers    map[string]string `json:"headers"`
		Body       json.RawMessage   `json:"body"`
		BodyBase64 string            `json:"bodyBase64"`
	} `json:"response"`
}

// fixtureKey is how a recorded case is matched: method, path and the *family* of
// the content type. The family rather than the exact type because a client's
// transport picks one encoding and the corpus carries all four; matching the
// exact string would be the same thing, but naming the families keeps the
// in-process replay and `fixture-server.mjs` in step.
func fixtureKey(method, path, contentType string) string {
	return method + " " + path + " " + contentFamily(contentType)
}

// contentFamily mirrors `family()` in `sdks/conformance/fixture-server.mjs`.
func contentFamily(contentType string) string {
	base, _, _ := strings.Cut(contentType, ";")
	base = strings.TrimSpace(base)
	switch {
	case base == "application/grpc-web+json":
		return "grpc_web_json"
	case strings.HasPrefix(base, "application/grpc-web"):
		return "grpc_web"
	case strings.HasPrefix(base, "application/connect"):
		return "connect"
	case base == "application/json":
		return "json"
	case base == "application/proto":
		return "proto"
	case base == "":
		return "none"
	default:
		return base
	}
}

// startReplayServer serves `sdks/fixtures/recorded` over `net/http`, with the
// same matching rules as `fixture-server.mjs`: keyed on method, path and content
// family, with the recorded request body checked, and a **404 naming the gap**
// for anything unrecorded rather than a silent success.
func startReplayServer(t *testing.T) *fixtureServer {
	t.Helper()
	recorded, err := filepath.Glob(filepath.Join(fixturesDir, "recorded", "*.json"))
	if err != nil || len(recorded) == 0 {
		t.Fatalf("no recorded fixtures under %s/recorded: %v", fixturesDir, err)
	}
	cases := map[string]recordedCase{}
	for _, path := range recorded {
		payload, readErr := os.ReadFile(path)
		if readErr != nil {
			t.Fatalf("reading %s: %v", path, readErr)
		}
		var entry recordedCase
		if err := json.Unmarshal(payload, &entry); err != nil {
			t.Fatalf("parsing %s: %v", path, err)
		}
		cases[fixtureKey(entry.Request.Method, entry.Request.Path, entry.Request.Headers["content-type"])] = entry
	}
	keys := make([]string, 0, len(cases))
	for key := range cases {
		keys = append(keys, key)
	}

	server := httptest.NewServer(http.HandlerFunc(func(writer http.ResponseWriter, request *http.Request) {
		key := fixtureKey(request.Method, request.URL.Path, request.Header.Get("Content-Type"))
		entry, found := cases[key]
		if !found {
			// Loudly. A suite that passed because everything answered 200 would
			// be worse than no suite.
			writer.Header().Set("Content-Type", "application/json")
			writer.WriteHeader(http.StatusNotFound)
			_ = json.NewEncoder(writer).Encode(map[string]any{
				"error":    "no recorded fixture for " + key,
				"recorded": keys,
			})
			return
		}
		sent, readErr := readAll(request)
		if readErr != nil {
			writer.WriteHeader(http.StatusBadRequest)
			return
		}
		// The request is checked as well as the response: a replay that ignored
		// what the client sent would pass an SDK that frames a gRPC-Web message
		// wrongly or posts the wrong payload.
		if expected := recordedBody(entry.Request.Body, entry.Request.BodyBase64); string(sent) != string(expected) {
			writer.Header().Set("Content-Type", "application/json")
			writer.WriteHeader(http.StatusBadRequest)
			_ = json.NewEncoder(writer).Encode(map[string]any{
				"error":    "the request does not match the recorded one for " + key,
				"expected": base64.StdEncoding.EncodeToString(expected),
				"sent":     base64.StdEncoding.EncodeToString(sent),
			})
			return
		}
		for name, value := range entry.Response.Headers {
			writer.Header().Set(name, value)
		}
		writer.WriteHeader(entry.Response.Status)
		_, _ = writer.Write(recordedBody(entry.Response.Body, entry.Response.BodyBase64))
	}))
	t.Cleanup(server.Close)
	return &fixtureServer{endpoint: server.URL, stop: server.Close}
}

// recordedBody is the JSON body a fixture carries, or the raw bytes of a
// `bodyBase64` one. Exactly one of the two is present in every recorded case.
func recordedBody(body json.RawMessage, bodyBase64 string) []byte {
	if len(body) > 0 && string(body) != "null" {
		return []byte(body)
	}
	decoded, err := base64.StdEncoding.DecodeString(bodyBase64)
	if err != nil {
		return nil
	}
	return decoded
}

func readAll(request *http.Request) ([]byte, error) {
	defer request.Body.Close()
	return io.ReadAll(request.Body)
}

// fixtureCorpus is `sdks/fixtures/index.json`.
type fixtureCorpus struct {
	About string `json:"about"`
	Cases []struct {
		Name        string  `json:"name"`
		About       string  `json:"about"`
		Path        string  `json:"path"`
		ContentType string  `json:"contentType"`
		Status      int     `json:"status"`
		Reason      *string `json:"reason"`
	} `json:"cases"`
}

// readCorpus reads the corpus index, failing the test if it cannot be read: a
// suite that cannot see the corpus is not a suite.
func readCorpus(t *testing.T) fixtureCorpus {
	t.Helper()
	payload, err := os.ReadFile(filepath.Join(fixturesDir, "index.json"))
	if err != nil {
		t.Fatalf("reading the fixture corpus index: %v", err)
	}
	var corpus fixtureCorpus
	if err := json.Unmarshal(payload, &corpus); err != nil {
		t.Fatalf("parsing the fixture corpus index: %v", err)
	}
	return corpus
}

// newTestClient builds a client against a fixture endpoint. It is
// unauthenticated, which is what `GetInstance` needs anyway and what keeps a
// bearer out of the test suite entirely.
func newTestClient(t *testing.T, endpoint string) *Client {
	t.Helper()
	client, err := New(Options{
		Endpoint:   endpoint,
		HTTPClient: &http.Client{Timeout: 20 * time.Second},
	})
	if err != nil {
		t.Fatalf("building a client for %s: %v", endpoint, err)
	}
	t.Cleanup(func() { _ = client.Close() })
	return client
}
