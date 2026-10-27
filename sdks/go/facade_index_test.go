// go_facade_matches_index: Go's binding table (`gen/facade`, the Q604
// fallback, D730) checked against `sdks/fixtures/facade-index.json`, which the
// generator renders from the same annotations (SDKP P0a). Every module Go
// carries must have exactly the index's calls, RPC paths, retry classes,
// idempotency keys, pagination and streaming shape, so a transcription cannot
// drift from the protos silently. The app modules must all be there.

package loams

import (
	"encoding/json"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"testing"

	"loams.dev/go/gen/facade"
)

// FacadeMatchesIndex is the canonical test name.
const FacadeMatchesIndex = "go_facade_matches_index"

// knownDrift is the Go facade's drift from the index that predates SDKP, in
// the Live modules: `tables.Deploy` kept after Deploy moved to `live.admin`
// (LV1 Task 10), and six `live.admin` calls never transcribed. Listed so the
// gap is visible and any *new* drift fails; closing it is a Live task, not an
// app one.
var knownDrift = map[string]bool{
	"tables.Deploy":                 true,
	"live.admin.ActivateDeployment": true,
	"live.admin.Deploy":             true,
	"live.admin.GetDeployment":      true,
	"live.admin.GetIndexStatus":     true,
	"live.admin.ListDeployments":    true,
	"live.admin.TailLogs":           true,
}

type indexCall struct {
	Module              string `json:"module"`
	Call                string `json:"call"`
	RPC                 string `json:"rpc"`
	Retry               string `json:"retry"`
	IdempotencyKeyField string `json:"idempotencyKeyField"`
	Pagination          *struct {
		Items         string `json:"items"`
		NextPageToken string `json:"nextPageToken"`
	} `json:"pagination"`
	Stream bool `json:"stream"`
}

func pascalOf(snake string) string {
	parts := strings.Split(snake, "_")
	for i, p := range parts {
		if p != "" {
			parts[i] = strings.ToUpper(p[:1]) + p[1:]
		}
	}
	return strings.Join(parts, "")
}

func TestGoFacadeMatchesIndex(t *testing.T) {
	t.Run(FacadeMatchesIndex, func(t *testing.T) {
		raw, err := os.ReadFile(filepath.Join(fixturesDir, "facade-index.json"))
		if err != nil {
			t.Fatal(err)
		}
		var index struct {
			Calls []indexCall `json:"calls"`
		}
		if err := json.Unmarshal(raw, &index); err != nil {
			t.Fatal(err)
		}
		carried := map[string]bool{}
		for _, module := range facade.Modules {
			carried[module.Name] = true
		}
		for _, required := range []string{"namespaces", "collections", "documents", "search", "vector", "graph"} {
			if !carried[required] {
				t.Errorf("the app module %s is missing", required)
			}
		}
		want := map[string]indexCall{}
		for _, call := range index.Calls {
			if carried[call.Module] {
				want[call.Module+"."+pascalOf(call.Call)] = call
			}
		}
		var got []string
		for _, module := range facade.Modules {
			for _, call := range module.Calls {
				key := module.Name + "." + call.Name
				got = append(got, key)
				entry, ok := want[key]
				if !ok {
					if !knownDrift[key] {
						t.Errorf("%s is in the Go facade but not in the index", key)
					}
					continue
				}
				if call.RPC != entry.RPC {
					t.Errorf("%s: RPC %s, the index says %s", key, call.RPC, entry.RPC)
				}
				wantRetry := facade.RetryManual
				if entry.Retry == "safe" {
					wantRetry = facade.RetrySafe
				}
				if call.Retry != wantRetry {
					t.Errorf("%s: retry %v, the index says %s", key, call.Retry, entry.Retry)
				}
				if call.TakesIdempotencyKey != (entry.IdempotencyKeyField != "") {
					t.Errorf("%s: keyed %v, the index says %q", key, call.TakesIdempotencyKey, entry.IdempotencyKeyField)
				}
				if (call.Pagination == nil) != (entry.Pagination == nil) {
					t.Errorf("%s: pagination %v, the index says %v", key, call.Pagination, entry.Pagination)
				} else if call.Pagination != nil && call.Pagination.ItemsField != pascalOf(entry.Pagination.Items) {
					t.Errorf("%s: items %s, the index says %s", key, call.Pagination.ItemsField, entry.Pagination.Items)
				}
				if (call.Streaming == facade.Server) != entry.Stream {
					t.Errorf("%s: streaming %v, the index says %v", key, call.Streaming, entry.Stream)
				}
			}
		}
		var missing []string
		for key := range want {
			found := false
			for _, g := range got {
				if g == key {
					found = true
				}
			}
			if !found && !knownDrift[key] {
				missing = append(missing, key)
			}
		}
		sort.Strings(missing)
		if len(missing) > 0 {
			t.Errorf("the index has calls the Go facade lacks: %v", missing)
		}
	})
}
