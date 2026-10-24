// corpus_replays_go (DX0 Task 4): every Go history in the corpus replays
// with the pinned Go SDK's replayer and zero nondeterminism errors.
package replay

import (
	"os"
	"path/filepath"
	"regexp"
	"testing"

	"go.temporal.io/sdk/client"
	"go.temporal.io/sdk/worker"
	"go.temporal.io/sdk/workflow"

	"github.com/ostrium-labs/loams/conformance/compute/temporal/corpus/workers/go/scenarios"
)

var runSuffix = regexp.MustCompile(`\.run[0-9]+$`)

func TestCorpusReplaysGo(t *testing.T) {
	dir := os.Getenv("CORPUS_HISTORIES")
	if dir == "" {
		dir = "../../histories/go"
	}
	files, err := filepath.Glob(filepath.Join(dir, "*.json"))
	if err != nil {
		t.Fatal(err)
	}
	if len(files) == 0 {
		t.Fatalf("no histories in %s", dir)
	}
	for _, f := range files {
		name := runSuffix.ReplaceAllString(filepath.Base(f[:len(f)-len(".json")]), "")
		t.Run(filepath.Base(f), func(t *testing.T) {
			r := worker.NewWorkflowReplayer()
			for _, s := range scenarios.All {
				r.RegisterWorkflowWithOptions(s.Workflow, workflow.RegisterOptions{Name: s.Name})
			}
			for n, fn := range scenarios.Children {
				r.RegisterWorkflowWithOptions(fn, workflow.RegisterOptions{Name: n})
			}
			fh, err := os.Open(f)
			if err != nil {
				t.Fatal(err)
			}
			defer fh.Close()
			h, err := client.HistoryFromJSON(fh, client.HistoryJSONOptions{})
			if err != nil {
				t.Fatalf("%s: %v", f, err)
			}
			// The workflow ID the generator used: child IDs derive from it.
			opts := worker.ReplayWorkflowHistoryOptions{
				OriginalExecution: workflow.Execution{ID: "corpus-go-" + name},
			}
			if err := r.ReplayWorkflowHistoryWithOptions(nil, h, opts); err != nil {
				t.Fatalf("%s (%s) does not replay: %v", f, name, err)
			}
		})
	}
}
