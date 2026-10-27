// SDKP's `go_builders_match_corpus`.
//
// Every case in `sdks/fixtures/builders/` is driven through the public
// builders — NewSchema, Filter, NewSearch — from its neutral spec (the
// notation is in that directory's README), and the output is compared with the
// case's `expect` after both are decoded into the named message, so key order
// and number spelling do not matter and a field the message lacks does.

package loams

import (
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"reflect"
	"sort"
	"strings"
	"testing"

	"google.golang.org/protobuf/encoding/protojson"
	"google.golang.org/protobuf/proto"

	collectionv1 "loams.dev/go/gen/loams/collection/v1"
)

type corpusCall struct {
	Op   string `json:"op"`
	Args []any  `json:"args"`
}

type builderCase struct {
	Name    string          `json:"name"`
	Builder string          `json:"builder"`
	Message string          `json:"message"`
	Spec    json.RawMessage `json:"spec"`
	Expect  json.RawMessage `json:"expect"`
}

func loadBuilderCases(t *testing.T) []builderCase {
	t.Helper()
	dir := filepath.Join("..", "fixtures", "builders")
	entries, err := os.ReadDir(dir)
	if err != nil {
		t.Fatal(err)
	}
	var cases []builderCase
	for _, entry := range entries {
		if !strings.HasSuffix(entry.Name(), ".json") {
			continue
		}
		raw, err := os.ReadFile(filepath.Join(dir, entry.Name()))
		if err != nil {
			t.Fatal(err)
		}
		var c builderCase
		if err := json.Unmarshal(raw, &c); err != nil {
			t.Fatal(err)
		}
		cases = append(cases, c)
	}
	sort.Slice(cases, func(i, j int) bool { return cases[i].Name < cases[j].Name })
	return cases
}

// asCall is an argument that is itself a call: an object with an `op` key.
func asCall(value any) (corpusCall, bool) {
	object, ok := value.(map[string]any)
	if !ok {
		return corpusCall{}, false
	}
	op, ok := object["op"].(string)
	if !ok {
		return corpusCall{}, false
	}
	args, _ := object["args"].([]any)
	return corpusCall{Op: op, Args: args}, true
}

// options is a trailing options object (no `op`), or nil.
func options(args []any) map[string]any {
	if len(args) == 0 {
		return nil
	}
	object, ok := args[len(args)-1].(map[string]any)
	if !ok {
		return nil
	}
	if _, isCall := object["op"]; isCall {
		return nil
	}
	return object
}

func str(value any) string  { s, _ := value.(string); return s }
func num(value any) float64 { n, _ := value.(float64); return n }
func u32(value any) uint32  { return uint32(num(value)) }
func stringList(value any) []string {
	list, _ := value.([]any)
	out := make([]string, len(list))
	for i, item := range list {
		out[i] = str(item)
	}
	return out
}
func floats(value any) []float32 {
	list, _ := value.([]any)
	out := make([]float32, len(list))
	for i, item := range list {
		out[i] = float32(num(item))
	}
	return out
}

// evaluate is a nested filter, fusion or sort call.
func evaluate(call corpusCall) (JSONObject, error) {
	a := call.Args
	switch call.Op {
	case "rrf":
		if len(a) == 0 {
			return RRF(), nil
		}
		return RRF(u32(a[0])), nil
	case "dbsf":
		return DBSF(), nil
	case "weighted":
		return Weighted(floats(a)...), nil
	case "score":
		if len(a) == 0 {
			return Sort.Score(), nil
		}
		return Sort.Score(str(a[0])), nil
	case "pk":
		if len(a) == 0 {
			return Sort.PK(), nil
		}
		return Sort.PK(str(a[0])), nil
	case "field":
		return Sort.Field(str(a[0]), str(at(a, 1)), str(at(a, 2))), nil
	}
	return evaluateFilter(call)
}

func at(list []any, i int) any {
	if i < len(list) {
		return list[i]
	}
	return nil
}

func nestedFilters(args []any) ([]FilterJSON, error) {
	out := make([]FilterJSON, 0, len(args))
	for _, arg := range args {
		call, ok := asCall(arg)
		if !ok {
			return nil, fmt.Errorf("a nested filter is not a call: %v", arg)
		}
		filter, err := evaluate(call)
		if err != nil {
			return nil, err
		}
		out = append(out, filter)
	}
	return out, nil
}

func docID(value any) DocID {
	switch v := value.(type) {
	case float64:
		return UintID(uint64(v))
	case string:
		return StringID(v)
	case map[string]any:
		if uuid, ok := v["uuid"].(string); ok {
			return UUIDID(uuid)
		}
	}
	return StringID(fmt.Sprint(value))
}

func evaluateFilter(call corpusCall) (FilterJSON, error) {
	a := call.Args
	switch call.Op {
	case "all":
		return Filter.All(), nil
	case "none":
		return Filter.None(), nil
	case "eq":
		return Filter.Eq(str(a[0]), a[1]), nil
	case "in":
		values, _ := a[1].([]any)
		return Filter.In(str(a[0]), values...), nil
	case "range":
		o := options(a)
		return Filter.Range(str(a[0]), Bounds{Gt: o["gt"], Gte: o["gte"], Lt: o["lt"], Lte: o["lte"]}), nil
	case "exists":
		return Filter.Exists(str(a[0])), nil
	case "is_null":
		return Filter.IsNull(str(a[0])), nil
	case "is_empty":
		return Filter.IsEmpty(str(a[0])), nil
	case "prefix":
		return Filter.Prefix(str(a[0]), str(a[1])), nil
	case "wildcard":
		return Filter.Wildcard(str(a[0]), str(a[1])), nil
	case "match":
		o := options(a)
		return Filter.Match(str(a[0]), str(a[1]), MatchOptions{Operator: str(o["operator"]), Analyzer: str(o["analyzer"])}), nil
	case "phrase":
		return Filter.Phrase(str(a[0]), str(a[1]), u32(at(a, 2))), nil
	case "ids":
		ids := make([]DocID, len(a))
		for i, arg := range a {
			ids[i] = docID(arg)
		}
		return Filter.IDs(ids...), nil
	case "and", "or", "not", "filter":
		queries, err := nestedFilters(a)
		if err != nil {
			return nil, err
		}
		switch call.Op {
		case "and":
			return Filter.And(queries...), nil
		case "or":
			return Filter.Or(queries...), nil
		case "not":
			return Filter.Not(queries...), nil
		default:
			return Filter.Filter(queries...), nil
		}
	}
	return nil, fmt.Errorf("no filter verb %q", call.Op)
}

func buildSchema(calls []corpusCall) (JSONObject, error) {
	schema := NewSchema()
	for _, call := range calls {
		a := call.Args
		switch call.Op {
		case "text":
			schema.Text(str(a[0]))
		case "keyword":
			schema.Keyword(str(a[0]))
		case "i64":
			schema.I64(str(a[0]))
		case "f64":
			schema.F64(str(a[0]))
		case "bool":
			schema.Bool(str(a[0]))
		case "timestamp":
			schema.Timestamp(str(a[0]))
		case "vector":
			if distance := str(at(a, 2)); distance != "" {
				schema.Vector(str(a[0]), int(num(a[1])), distance)
			} else {
				schema.Vector(str(a[0]), int(num(a[1])))
			}
		case "sparse_vector":
			schema.SparseVector(str(a[0]))
		case "dynamic":
			schema.Dynamic(str(a[0]))
		case "max_fields":
			schema.MaxFields(int(num(a[0])))
		default:
			return nil, fmt.Errorf("no schema verb %q", call.Op)
		}
	}
	return schema.JSON(), nil
}

func buildSearch(spec struct {
	On    string       `json:"on"`
	Calls []corpusCall `json:"calls"`
}) (*collectionv1.SearchRequest, error) {
	s := NewSearch(spec.On)
	for _, call := range spec.Calls {
		a := call.Args
		o := options(a)
		switch call.Op {
		case "text":
			s = s.Text(str(a[0]), TextOptions{Fields: stringList(o["fields"]), K: u32(o["k"])})
		case "vector":
			s = s.Vector(str(a[0]), floats(a[1]), VectorOptions{
				K: u32(o["k"]), Nprobes: u32(o["nprobes"]), RefineFactor: u32(o["refine_factor"]),
				Ef: u32(o["ef"]), Exact: o["exact"] == true,
			})
		case "filter":
			nested, _ := asCall(a[0])
			filter, err := evaluate(nested)
			if err != nil {
				return nil, err
			}
			s = s.Filter(filter)
		case "fuse":
			nested, _ := asCall(a[0])
			fusion, err := evaluate(nested)
			if err != nil {
				return nil, err
			}
			s = s.Fuse(fusion)
		case "sort":
			keys, err := nestedFilters(a)
			if err != nil {
				return nil, err
			}
			s = s.Sort(keys...)
		case "highlight":
			s = s.Highlight(str(a[0]))
		case "select":
			s = s.Select(SelectOptions{Include: nilIfEmpty(stringList(o["include"])), Exclude: nilIfEmpty(stringList(o["exclude"]))})
		case "limit":
			s = s.Limit(u32(a[0]))
		case "offset":
			s = s.Offset(u32(a[0]))
		case "after":
			values, _ := a[0].([]any)
			s = s.After(values...)
		case "expand":
			s = s.Expand(str(a[0]), ExpandOptions{FromField: str(o["from_field"]), Hops: u32(o["hops"])})
		default:
			return nil, fmt.Errorf("no search verb %q", call.Op)
		}
	}
	return s.Build()
}

func nilIfEmpty(list []string) []string {
	if len(list) == 0 {
		return nil
	}
	return list
}

func sameMessage(t *testing.T, got, want proto.Message) {
	t.Helper()
	if !proto.Equal(got, want) {
		gotJSON, _ := protojson.Marshal(got)
		wantJSON, _ := protojson.Marshal(want)
		t.Errorf("got  %s\nwant %s", gotJSON, wantJSON)
	}
}

func Test_go_builders_match_corpus(t *testing.T) {
	cases := loadBuilderCases(t)
	names := map[string]bool{}
	for _, c := range cases {
		names[c.Name] = true
	}
	for _, required := range []string{
		"schema_notes", "filter_term", "filter_bool_nested", "filter_range_open", "filter_ids",
		"search_text_filter_highlight_select", "search_vector_ann_params", "search_hybrid_rrf",
		"search_hybrid_dbsf", "search_hybrid_weighted", "search_after_page", "search_expand",
	} {
		if !names[required] {
			t.Errorf("the corpus has no %s", required)
		}
	}
	for _, c := range cases {
		t.Run(c.Name, func(t *testing.T) {
			switch c.Builder {
			case "schema":
				var calls []corpusCall
				if err := json.Unmarshal(c.Spec, &calls); err != nil {
					t.Fatal(err)
				}
				built, err := buildSchema(calls)
				if err != nil {
					t.Fatal(err)
				}
				raw, _ := json.Marshal(built)
				var got, want any
				_ = json.Unmarshal(raw, &got)
				_ = json.Unmarshal(c.Expect, &want)
				if !reflect.DeepEqual(got, want) {
					t.Errorf("got  %s\nwant %s", raw, c.Expect)
				}
			case "filter":
				var call corpusCall
				if err := json.Unmarshal(c.Spec, &call); err != nil {
					t.Fatal(err)
				}
				built, err := evaluate(call)
				if err != nil {
					t.Fatal(err)
				}
				got, err := ToQuery(built)
				if err != nil {
					t.Fatal(err)
				}
				want := &collectionv1.Query{}
				if err := protojson.Unmarshal(c.Expect, want); err != nil {
					t.Fatal(err)
				}
				sameMessage(t, got, want)
			case "search":
				var spec struct {
					On    string       `json:"on"`
					Calls []corpusCall `json:"calls"`
				}
				if err := json.Unmarshal(c.Spec, &spec); err != nil {
					t.Fatal(err)
				}
				got, err := buildSearch(spec)
				if err != nil {
					t.Fatal(err)
				}
				want := &collectionv1.SearchRequest{}
				if err := protojson.Unmarshal(c.Expect, want); err != nil {
					t.Fatal(err)
				}
				sameMessage(t, got, want)
			default:
				t.Fatalf("unknown builder %q", c.Builder)
			}
		})
	}

	t.Run("a weighted fusion with the wrong number of weights is refused before any call", func(t *testing.T) {
		_, err := NewSearch("notes").Text("refund", TextOptions{Fields: []string{"body"}}).
			Vector("embedding", []float32{1, 0, 0, 0}).Fuse(Weighted(1)).Build()
		var invalid *InvalidArgumentError
		if !errors.As(err, &invalid) || invalid.Metadata["field"] != "fusion.weighted_sum.weights" {
			t.Errorf("got %v", err)
		}
	})
	t.Run("a verb leaves the builder it was called on unchanged", func(t *testing.T) {
		base := NewSearch("notes").Text("refund")
		limited := base.Limit(2)
		if _, ok := base.JSON()["limit"]; ok {
			t.Error("Limit changed its receiver")
		}
		if limited.JSON()["limit"] != uint32(2) {
			t.Error("Limit did not set the limit")
		}
	})
}
